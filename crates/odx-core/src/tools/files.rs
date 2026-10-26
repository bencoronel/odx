//! Managed download/to_file output under <profile>/outputs. The server chooses paths and names. Write exclusive temporary files without following links; publish via non-overwriting hard links, never partial files. Retention: first of 24 h, 512 MiB or 4096 files; 32 MiB per file. Business data is readable by the local user (0600), contains no credentials and is not master-password encrypted.
use crate::{
    clients::{Capability, ClientIdentity},
    execution::{Core, Upstream},
    output::{Outcome, ToolError, ToolOutput, not_found},
    tools::args::{Args, common, is_identifier, kw, model, record_id},
};
use chrono::{DateTime, Utc};
use serde_json::{Map, Value, json};
use std::{
    fs,
    io::{self, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

const MIB: u64 = 1024 * 1024;
/// Maximum decoded download size.
pub(crate) const MAX_DOWNLOAD_DECODED: usize = 32 * MIB as usize;
/// Maximum base64 text before decoding.
pub(crate) const MAX_DOWNLOAD_ENCODED: usize = 48 * MIB as usize;
/// Binary response limit: base64 plus 1 MiB for filename, MIME type and protocol overhead. Other calls remain at 16 MiB.
const MAX_DOWNLOAD_RESPONSE: usize = MAX_DOWNLOAD_ENCODED + MIB as usize;

#[derive(Debug, Clone, Copy)]
pub(crate) struct Quota {
    pub(crate) per_file: u64,
    pub(crate) total: u64,
    pub(crate) max_age: Duration,
    pub(crate) max_files: usize,
}
pub(crate) const QUOTA: Quota = Quota {
    per_file: 32 * MIB,
    total: 512 * MIB,
    max_age: Duration::from_secs(24 * 3600),
    max_files: 4096,
};
/// Configured retention quota, capped at 24 h and 512 MiB.
pub(crate) fn output_quota(settings: crate::config::Settings) -> Quota {
    Quota {
        total: (u64::from(settings.output_mib) * MIB).min(QUOTA.total),
        max_age: Duration::from_secs(u64::from(settings.output_hours) * 3600).min(QUOTA.max_age),
        ..QUOTA
    }
}
/// Maximum directory entries scanned per cleanup.
const MAX_SCAN: usize = 100_000;

/// Serialize the fetched scope: TSV tables, XML views, JSON calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutputKind {
    Tsv,
    Xml,
    Json,
}

#[derive(Debug)]
pub(crate) struct Stored {
    pub(crate) id: String,
    pub(crate) path: PathBuf,
    pub(crate) bytes: u64,
    pub(crate) expires_at: DateTime<Utc>,
    pub(crate) removed_files: usize,
    pub(crate) removed_bytes: u64,
}

#[derive(Debug)]
pub(crate) enum StoreError {
    TooLarge(u64),
    Io(io::Error),
}
impl From<io::Error> for StoreError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}
impl From<StoreError> for ToolError {
    fn from(error: StoreError) -> Self {
        match error {
            StoreError::TooLarge(bytes) => ToolError::new(
                "output_too_large",
                format!(
                    "file would use {bytes} bytes, exceeding the {} per-file limit; nothing saved",
                    QUOTA.per_file
                ),
                Outcome::Failed,
            ),
            StoreError::Io(error) => ToolError::new(
                "output_storage_error",
                format!("could not save output file; no partial file remains: {error}"),
                Outcome::Failed,
            ),
        }
    }
}

/// Serialize writes and cleanup to enforce quotas without races.
static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Managed name: <uuid>[-<hint>][.<ext>] or temporary .<uuid>.part.
fn managed(name: &str) -> bool {
    let core = name
        .strip_prefix('.')
        .and_then(|n| n.strip_suffix(".part"))
        .unwrap_or(name);
    core.get(..36)
        .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok())
        && matches!(core.as_bytes().get(36), None | Some(b'-' | b'.'))
}

/// Safe filename hint: [A-Za-z0-9._-], no separators/leading dot, at most 64 characters.
pub(crate) fn safe_name(hint: &str) -> String {
    let clean: String = hint
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let clean = clean.trim_start_matches(['.', '-']);
    if clean.len() <= 64 {
        return clean.to_owned();
    }
    // Shorten the stem, not the extension: invoice_…_final.pdf stays a .pdf (all ASCII here).
    let ext = clean
        .rfind('.')
        .map(|dot| &clean[dot..])
        .filter(|ext| ext.len() <= 10)
        .unwrap_or("");
    format!("{}{ext}", &clean[..64 - ext.len()])
}

fn ensure_dir(dir: &Path) -> io::Result<()> {
    match fs::symlink_metadata(dir) {
        Ok(meta) => {
            if !meta.file_type().is_dir() {
                return Err(io::Error::other(
                    "managed output path is not a directory (possible symlink)",
                ));
            }
            // SAFETY: geteuid has no preconditions or side effects.
            if meta.uid() != unsafe { libc::geteuid() } {
                return Err(io::Error::other("output directory belongs to another user"));
            }
            if meta.mode() & 0o077 != 0 {
                fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
            }
            Ok(())
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            fs::DirBuilder::new().mode(0o700).create(dir)
        }
        Err(e) => Err(e),
    }
}

fn remove_quiet(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// Remove expired files and oldest files needed for incoming bytes. With None, enforce existing quota only. Touch only managed regular files; never follow links.
fn sweep(
    dir: &Path,
    quota: &Quota,
    now: SystemTime,
    incoming: Option<u64>,
) -> io::Result<(usize, u64)> {
    let mut kept: Vec<(SystemTime, u64, PathBuf)> = Vec::new();
    let (mut removed, mut removed_bytes) = (0, 0);
    for entry in fs::read_dir(dir)?.take(MAX_SCAN) {
        let entry = entry?;
        let name = entry.file_name();
        if !name.to_str().is_some_and(managed) {
            continue;
        }
        // DirEntry::metadata does not follow symlinks.
        let meta = entry.metadata()?;
        if !meta.file_type().is_file() {
            continue;
        }
        let modified = meta.modified()?;
        if now.duration_since(modified).unwrap_or_default() >= quota.max_age {
            remove_quiet(&entry.path())?;
            removed += 1;
            removed_bytes += meta.len();
            continue;
        }
        kept.push((modified, meta.len(), entry.path()));
    }
    kept.sort();
    let mut total: u64 = kept.iter().map(|k| k.1).sum();
    let mut count = kept.len();
    for (_, len, path) in &kept {
        if total + incoming.unwrap_or(0) <= quota.total
            && count + usize::from(incoming.is_some()) <= quota.max_files
        {
            break;
        }
        remove_quiet(path)?;
        total -= len;
        count -= 1;
        removed += 1;
        removed_bytes += len;
    }
    Ok((removed, removed_bytes))
}

/// Write body to a new file in dir; sanitize the server-chosen hint and use the format extension.
pub(crate) fn store(
    dir: &Path,
    hint: &str,
    ext: Option<&str>,
    body: &[u8],
    quota: &Quota,
    now: SystemTime,
) -> Result<Stored, StoreError> {
    let bytes = body.len() as u64;
    if bytes > quota.per_file {
        return Err(StoreError::TooLarge(bytes));
    }
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    ensure_dir(dir)?;
    let (removed_files, removed_bytes) = sweep(dir, quota, now, Some(bytes))?;
    let id = uuid::Uuid::new_v4().to_string();
    let mut name = id.clone();
    let hint = safe_name(hint);
    if !hint.is_empty() {
        name.push('-');
        name.push_str(&hint);
    }
    if let Some(ext) = ext {
        name.push('.');
        name.push_str(ext);
    }
    let path = dir.join(&name);
    let part = dir.join(format!(".{id}.part"));
    let written = (|| -> io::Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&part)?;
        file.write_all(body)?;
        file.sync_all()?;
        // Hard-link publication fails if the destination exists.
        fs::hard_link(&part, &path)
    })();
    // Remove temporary files on success and partial failure.
    let cleaned = remove_quiet(&part);
    written?;
    cleaned?;
    if let Ok(d) = fs::File::open(dir) {
        let _ = d.sync_all();
    }
    let expires_at = DateTime::<Utc>::from(now + quota.max_age);
    Ok(Stored {
        id,
        path,
        bytes,
        expires_at,
        removed_files,
        removed_bytes,
    })
}

/// Cleanup on unlock removes expired/excess files. Do not create absent directories; reject unowned ones as on writes.
pub(crate) fn sweep_outputs(
    dir: &Path,
    quota: &Quota,
    now: SystemTime,
) -> io::Result<(usize, u64)> {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if fs::symlink_metadata(dir).is_err_and(|e| e.kind() == io::ErrorKind::NotFound) {
        return Ok((0, 0));
    }
    ensure_dir(dir)?;
    sweep(dir, quota, now, None)
}

const SWEEP_EVERY: Duration = Duration::from_secs(600);
/// Output directories with a periodic sweeper and their latest quota.
static SWEEPERS: std::sync::Mutex<std::collections::BTreeMap<PathBuf, Quota>> =
    std::sync::Mutex::new(std::collections::BTreeMap::new());

/// One periodic sweep per directory removes expired files while idle; it stops once the directory is empty or gone.
fn keep_sweeping(dir: PathBuf, quota: Quota) {
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return;
    };
    let mut sweepers = SWEEPERS.lock().unwrap_or_else(|e| e.into_inner());
    if sweepers.insert(dir.clone(), quota).is_some() {
        return;
    }
    handle.spawn(async move {
        loop {
            tokio::time::sleep(SWEEP_EVERY).await;
            let dir = dir.clone();
            let done = tokio::task::spawn_blocking(move || {
                let quota = SWEEPERS.lock().unwrap_or_else(|e| e.into_inner())[&dir];
                let _ = sweep_outputs(&dir, &quota, SystemTime::now());
                // Check and deregister under the registry lock so a concurrent store restarts the sweeper.
                let mut sweepers = SWEEPERS.lock().unwrap_or_else(|e| e.into_inner());
                let idle = fs::read_dir(&dir).map_or(true, |mut e| e.next().is_none());
                if idle {
                    sweepers.remove(&dir);
                }
                idle
            })
            .await;
            if done.unwrap_or(true) {
                return;
            }
        }
    });
}

impl Core {
    /// Clean expired/excess output in a blocking background thread without waiting for another write.
    pub(crate) fn sweep_outputs_in_background(&self, settings: crate::config::Settings) {
        let dir = self.outputs_dir();
        let quota = output_quota(settings);
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn_blocking(move || sweep_outputs(&dir, &quota, SystemTime::now()));
        }
        keep_sweeping(self.outputs_dir(), quota);
    }

    /// Store body in the managed directory; return its descriptor and cleanup warnings.
    pub(crate) async fn store_output(
        &self,
        body: Vec<u8>,
        hint: String,
        ext: Option<&'static str>,
    ) -> Result<(Map<String, Value>, Option<String>), ToolError> {
        let dir = self.outputs_dir();
        let settings = self
            .inner
            .lock()
            .await
            .vault
            .as_ref()
            .map(|v| v.data().settings)
            .unwrap_or_default();
        let quota = output_quota(settings);
        let stored = tokio::task::spawn_blocking(move || {
            store(&dir, &hint, ext, &body, &quota, SystemTime::now())
        })
        .await
        .map_err(|_| StoreError::Io(io::Error::other("file write task interrupted")))??;
        keep_sweeping(self.outputs_dir(), quota);
        let mut descriptor = Map::new();
        descriptor.insert("file_id".into(), json!(stored.id));
        descriptor.insert("path".into(), json!(stored.path.to_string_lossy()));
        descriptor.insert("bytes".into(), json!(stored.bytes));
        descriptor.insert("expires_at".into(), json!(stored.expires_at.to_rfc3339()));
        let cleanup = (stored.removed_files > 0).then(|| {
            format!(
                "removed {} output files ({} bytes) due to expiry or quota",
                stored.removed_files, stored.removed_bytes
            )
        });
        Ok((descriptor, cleanup))
    }

    /// to_file stores exactly the fetched page and replaces data with its descriptor; metadata preserves pagination and metrics.
    pub(crate) async fn output_to_file(
        &self,
        mut out: ToolOutput,
        kind: OutputKind,
        label: &str,
    ) -> Result<ToolOutput, ToolError> {
        let (body, rows, ext) = match kind {
            OutputKind::Tsv => {
                let rows = out.data.as_array().cloned().unwrap_or_default();
                let rows: Vec<Value> = rows
                    .into_iter()
                    .map(|r| {
                        if r.is_object() {
                            r
                        } else {
                            json!({"value": r})
                        }
                    })
                    .collect();
                (
                    crate::output::tsv(&rows).into_bytes(),
                    Some(rows.len()),
                    "tsv",
                )
            }
            OutputKind::Xml => (
                out.data.as_str().unwrap_or_default().as_bytes().to_vec(),
                None,
                "xml",
            ),
            OutputKind::Json => (
                serde_json::to_vec_pretty(&out.data).map_err(|_| {
                    ToolError::new(
                        "output_storage_error",
                        "could not serialize result",
                        Outcome::Failed,
                    )
                })?,
                None,
                "json",
            ),
        };
        let (mut descriptor, cleanup) = self.store_output(body, label.into(), Some(ext)).await?;
        if let Some(rows) = rows {
            descriptor.insert("rows".into(), json!(rows));
        }
        out.data = Value::Object(descriptor);
        if let Some(message) = cleanup {
            out.warn("output_cleanup", message);
        }
        Ok(out)
    }

    pub(crate) async fn tool_download(
        &self,
        client: &ClientIdentity,
        arguments: Value,
    ) -> Result<ToolOutput, ToolError> {
        let mut args = Args::for_tool("download", arguments)?;
        let (format, trunc) = common(&mut args)?;
        let env_name = args.str("env")?;
        let mut context = args.object("context")?;
        let model = model(&mut args)?;
        let id = record_id(&mut args)?;
        let field = args.str_opt("field")?.unwrap_or_else(|| "datas".into());
        if !is_identifier(&field, false) {
            return Err(ToolError::invalid(format!("invalid field: {field}")));
        }
        let target = self.target(client, &env_name, Capability::OdooRead).await?;
        // Disable bin_size so Odoo returns content instead of textual size.
        context.insert("bin_size".into(), json!(false));
        let context = Value::Object(context);
        let mut up = Upstream::start();
        let described = self
            .read_call(
                client,
                &target,
                &model,
                "fields_get",
                json!([]),
                kw(&[
                    ("allfields", json!([field])),
                    ("attributes", json!(["type"])),
                ]),
                &context,
                &mut up,
            )
            .await?;
        match described[&field]["type"].as_str() {
            Some("binary") => {}
            Some(kind) => {
                return Err(ToolError::invalid(format!(
                    "{model}.{field} has type {kind}, expected binary"
                )));
            }
            None => {
                return Err(not_found(format!(
                    "field {model}.{field} is missing or inaccessible"
                )));
            }
        }
        let mut wanted = vec![field.clone()];
        if model == "ir.attachment" {
            wanted.extend(
                ["name", "mimetype"]
                    .into_iter()
                    .filter(|f| *f != field)
                    .map(String::from),
            );
        }
        let _download = self.limits.download_permit().await?;
        let rows = self
            .read_call_limited(
                client,
                &target,
                &model,
                "read",
                |_| json!([[id]]),
                kw(&[("fields", json!(wanted))]),
                &context,
                &mut up,
                MAX_DOWNLOAD_RESPONSE,
            )
            .await
            .map_err(|mut e| {
                if e.code == "output_too_large" {
                    e.message = "binary exceeds download limits (48 MiB encoded, 32 MiB decoded); no file created"
                        .into();
                }
                e
            })?;
        let mut rows = rows;
        let Some(row) = rows.get_mut(0) else {
            return Err(not_found(format!(
                "record {model}({id}) is missing or inaccessible"
            )));
        };
        let encoded = match row[&field].take() {
            Value::String(s) if !s.is_empty() => s,
            _ => {
                return Err(not_found(format!(
                    "{model}({id}).{field} is empty; no file created"
                )));
            }
        };
        // Up to 48 MiB of base64: decode off the async workers.
        let decoded = tokio::task::spawn_blocking(move || decode(&encoded).map_err(Box::new))
            .await
            .map_err(|_| StoreError::Io(io::Error::other("decode task interrupted")))?
            .map_err(|e| *e)?;
        let hint = row
            .get("name")
            .and_then(Value::as_str)
            .filter(|n| !n.is_empty() && field != "name")
            .map(String::from)
            .unwrap_or_else(|| format!("{model}-{id}-{field}"));
        let (mut descriptor, cleanup) = self.store_output(decoded, hint, None).await?;
        for extra in ["name", "mimetype"] {
            if extra != field
                && let Some(v) = row.get(extra)
            {
                descriptor.insert(extra.into(), v.clone());
            }
        }
        let mut meta = Map::new();
        up.write_meta(&mut meta);
        let mut out = ToolOutput {
            data: Value::Object(descriptor),
            meta,
            format,
            trunc,
        };
        if let Some(message) = cleanup {
            out.warn("output_cleanup", message);
        }
        Ok(out)
    }
}

/// Decode Odoo base64 with encoded and decoded size limits.
pub(crate) fn decode(encoded: &str) -> Result<Vec<u8>, ToolError> {
    if encoded.len() > MAX_DOWNLOAD_ENCODED {
        return Err(ToolError::new(
            "output_too_large",
            format!(
                "encoded binary uses {} bytes, exceeding {MAX_DOWNLOAD_ENCODED}",
                encoded.len()
            ),
            Outcome::Failed,
        ));
    }
    libsodium_rs::ensure_init().map_err(|_| {
        ToolError::new(
            "service_unavailable",
            "libsodium unavailable",
            Outcome::Failed,
        )
    })?;
    let decoded =
        libsodium_rs::utils::base642bin(encoded, libsodium_rs::utils::BASE64_VARIANT_ORIGINAL)
            .map_err(|_| {
                ToolError::new(
                    "upstream_error",
                    "Odoo returned invalid base64",
                    Outcome::Failed,
                )
            })?;
    if decoded.len() > MAX_DOWNLOAD_DECODED {
        return Err(ToolError::new(
            "output_too_large",
            format!(
                "binary uses {} bytes, exceeding {MAX_DOWNLOAD_DECODED}",
                decoded.len()
            ),
            Outcome::Failed,
        ));
    }
    Ok(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quota(total: u64, max_files: usize) -> Quota {
        Quota {
            per_file: 64,
            total,
            max_age: Duration::from_secs(3600),
            max_files,
        }
    }
    fn names(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        v.sort();
        v
    }

    #[test]
    fn names_are_chosen_by_the_server_and_sanitised() {
        assert_eq!(safe_name("../../etc/passwd"), "_.._etc_passwd");
        assert_eq!(safe_name(".hidden"), "hidden");
        assert_eq!(safe_name("invoice ñ 1.pdf"), "invoice___1.pdf");
        assert_eq!(safe_name(&"a".repeat(200)).len(), 64);
        let long = safe_name(&format!("Factura_{}_definitiva.pdf", "x".repeat(80)));
        assert!(long.len() == 64 && long.ends_with(".pdf"), "{long}");
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("outputs");
        let s = store(
            &dir,
            "../x/y",
            Some("tsv"),
            b"a\tb\n",
            &QUOTA,
            SystemTime::now(),
        )
        .unwrap();
        assert_eq!(s.path.parent().unwrap(), dir);
        let name = s.path.file_name().unwrap().to_str().unwrap();
        assert!(
            name.starts_with(&s.id) && name.ends_with("-_x_y.tsv"),
            "{name}"
        );
        assert_eq!(fs::read(&s.path).unwrap(), b"a\tb\n");
        let mode = fs::metadata(&s.path).unwrap().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(fs::metadata(&dir).unwrap().mode() & 0o777, 0o700);
        assert_eq!(names(&dir).len(), 1, "no temporary files remain");
    }

    #[test]
    fn per_file_limit_writes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("outputs");
        let err = store(
            &dir,
            "x",
            None,
            &[0; 65],
            &quota(1024, 10),
            SystemTime::now(),
        );
        assert!(matches!(err, Err(StoreError::TooLarge(65))));
        assert!(!dir.exists() || names(&dir).is_empty());
    }

    #[test]
    fn quota_removes_oldest_and_expired_and_reports_it() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("outputs");
        let q = quota(100, 3);
        let t0 = SystemTime::now();
        let touch = |path: &Path, at: SystemTime| {
            let f = fs::File::options().write(true).open(path).unwrap();
            f.set_modified(at).unwrap();
        };
        let first = store(&dir, "a", None, &[1; 40], &q, t0).unwrap();
        touch(&first.path, t0 - Duration::from_secs(10));
        let second = store(&dir, "b", None, &[2; 40], &q, t0).unwrap();
        touch(&second.path, t0 - Duration::from_secs(5));
        assert_eq!(second.removed_files, 0);
        // 40 + 40 + 40 > 100: remove only the oldest file.
        let third = store(&dir, "c", None, &[3; 40], &q, t0).unwrap();
        touch(&third.path, t0);
        assert_eq!((third.removed_files, third.removed_bytes), (1, 40));
        assert!(!first.path.exists() && second.path.exists() && third.path.exists());
        // After one hour, remove all earlier expired files.
        let later = t0 + Duration::from_secs(3600);
        let fourth = store(&dir, "d", None, &[4; 10], &q, later).unwrap();
        assert_eq!(fourth.removed_files, 2);
        assert_eq!(names(&dir).len(), 1);
        // File count limit.
        let q1 = quota(1000, 1);
        let fifth = store(&dir, "e", None, &[5; 1], &q1, later).unwrap();
        assert_eq!(fifth.removed_files, 1);
        assert!(!fourth.path.exists());
    }

    #[test]
    fn sweep_without_new_file_keeps_what_fits_and_ignores_a_missing_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("outputs");
        assert_eq!(
            sweep_outputs(&dir, &QUOTA, SystemTime::now()).unwrap(),
            (0, 0)
        );
        assert!(!dir.exists(), "does not create the directory");
        let t0 = SystemTime::now();
        let q = quota(80, 2);
        let a = store(&dir, "a", None, &[1; 40], &q, t0).unwrap();
        let b = store(&dir, "b", None, &[2; 40], &q, t0).unwrap();
        // Preserve files exactly at byte and count limits.
        assert_eq!(sweep_outputs(&dir, &q, t0).unwrap(), (0, 0));
        assert!(a.path.exists() && b.path.exists());
        // Over 512 MiB (40 bytes here): remove the oldest file.
        let q = quota(40, 2);
        fs::File::options()
            .write(true)
            .open(&a.path)
            .unwrap()
            .set_modified(t0 - Duration::from_secs(5))
            .unwrap();
        assert_eq!(sweep_outputs(&dir, &q, t0).unwrap(), (1, 40));
        assert!(!a.path.exists() && b.path.exists());
        // Remove expired files without another write.
        let later = t0 + Duration::from_secs(2 * 3600);
        assert_eq!(sweep_outputs(&dir, &q, later).unwrap(), (1, 40));
        assert!(names(&dir).is_empty());
    }

    #[test]
    fn foreign_entries_and_symlinks_are_never_followed_or_deleted() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("outputs");
        fs::create_dir(&dir).unwrap();
        let victim = tmp.path().join("victim");
        fs::write(&victim, b"keep").unwrap();
        // Neither follow nor delete managed symlinks or unrelated files.
        let planted = dir.join(format!("{}-x.tsv", uuid::Uuid::new_v4()));
        std::os::unix::fs::symlink(&victim, &planted).unwrap();
        fs::write(dir.join("notes.txt"), b"mine").unwrap();
        let q = quota(10, 1);
        store(&dir, "a", None, &[1; 8], &q, SystemTime::now()).unwrap();
        store(&dir, "b", None, &[1; 8], &q, SystemTime::now()).unwrap();
        assert_eq!(fs::read(&victim).unwrap(), b"keep");
        assert!(
            fs::symlink_metadata(&planted)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(dir.join("notes.txt").exists());
    }

    #[test]
    fn a_symlinked_output_directory_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let elsewhere = tmp.path().join("elsewhere");
        fs::create_dir(&elsewhere).unwrap();
        let dir = tmp.path().join("outputs");
        std::os::unix::fs::symlink(&elsewhere, &dir).unwrap();
        let err = store(&dir, "a", None, b"x", &QUOTA, SystemTime::now());
        assert!(matches!(err, Err(StoreError::Io(_))));
        assert!(names(&elsewhere).is_empty());
    }

    #[test]
    fn a_failed_write_leaves_no_partial_file() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("outputs");
        fs::DirBuilder::new().mode(0o500).create(&dir).unwrap();
        let err = store(&dir, "a", None, b"x", &QUOTA, SystemTime::now());
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(matches!(err, Err(StoreError::Io(_))));
        assert!(names(&dir).is_empty());
    }

    #[test]
    fn download_limits_apply_before_and_after_decoding() {
        assert_eq!(
            decode(&"A".repeat(MAX_DOWNLOAD_ENCODED + 4))
                .unwrap_err()
                .code,
            "output_too_large"
        );
        // 33 MiB decoded fits 48 MiB encoded but exceeds the 32 MiB decoded limit.
        let big = vec![0u8; MAX_DOWNLOAD_DECODED + 1];
        let encoded =
            libsodium_rs::utils::bin2base64(&big, libsodium_rs::utils::BASE64_VARIANT_ORIGINAL);
        assert!(encoded.len() <= MAX_DOWNLOAD_ENCODED);
        assert_eq!(decode(&encoded).unwrap_err().code, "output_too_large");
        assert_eq!(decode("no*base64").unwrap_err().code, "upstream_error");
        assert_eq!(decode("aGk=").unwrap(), b"hi");
    }

    #[tokio::test(start_paused = true)]
    async fn periodic_sweep_removes_expired_files_then_stops() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("outputs");
        let q = Quota {
            max_age: Duration::ZERO,
            ..QUOTA
        };
        let stored = store(&dir, "a", None, b"x", &q, SystemTime::now()).unwrap();
        keep_sweeping(dir.clone(), q);
        let running = || SWEEPERS.lock().unwrap().contains_key(&dir);
        assert!(running());
        for _ in 0..10 {
            if !running() {
                break;
            }
            tokio::time::sleep(SWEEP_EVERY).await;
        }
        assert!(!stored.path.exists());
        assert!(!running(), "an empty directory ends the sweeper");
    }
}
