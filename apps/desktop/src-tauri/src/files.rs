//! OS file and browser helpers for the desktop commands: open URLs, Odoo record links, Terminal exports.
use crate::error::AppError;

pub fn xdg_open(url: &str) -> Result<(), AppError> {
    let mut child = std::process::Command::new("xdg-open")
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|_| AppError::new("system.browser_open_failed"))?;
    // Reap the child asynchronously to avoid zombies.
    tauri::async_runtime::spawn_blocking(move || child.wait());
    Ok(())
}
/// A record to open in the Odoo web client.
#[derive(Debug, serde::Deserialize)]
pub struct RecordRef {
    pub(crate) model: String,
    pub(crate) id: u64,
}

pub(crate) const MAX_EXPORT: usize = 16 * 1024 * 1024;

/// Plain file names only (no paths), with a known extension.
pub(crate) fn export_name(name: &str) -> Option<(&str, &str)> {
    let (stem, ext) = name.rsplit_once('.')?;
    let valid = !stem.is_empty()
        && stem.len() <= 100
        && !stem.starts_with('.')
        && stem
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    (valid && matches!(ext, "csv" | "json")).then_some((stem, ext))
}

/// XDG download directory from user-dirs.dirs, else ~/Downloads, else home.
pub(crate) fn downloads_dir(
    home: &std::path::Path,
    config: &std::path::Path,
) -> std::path::PathBuf {
    let configured = std::fs::read_to_string(config.join("user-dirs.dirs"))
        .ok()
        .and_then(|text| {
            text.lines().find_map(|line| {
                let value = line
                    .trim()
                    .strip_prefix("XDG_DOWNLOAD_DIR=")?
                    .trim_matches('"');
                Some(match value.strip_prefix("$HOME") {
                    Some(rest) => home.join(rest.trim_start_matches('/')),
                    None => std::path::PathBuf::from(value),
                })
            })
        });
    [configured, Some(home.join("Downloads"))]
        .into_iter()
        .flatten()
        .find(|dir| dir.is_dir())
        .unwrap_or_else(|| home.to_owned())
}

/// Create `stem.ext`, or `stem-1.ext`… when taken; never overwrite.
pub(crate) fn write_new(
    dir: &std::path::Path,
    stem: &str,
    ext: &str,
    content: &[u8],
) -> std::io::Result<std::path::PathBuf> {
    use std::io::Write;
    for n in 0..100 {
        let file = if n == 0 {
            format!("{stem}.{ext}")
        } else {
            format!("{stem}-{n}.{ext}")
        };
        let path = dir.join(file);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut f) => return f.write_all(content).map(|()| path),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::ErrorKind::AlreadyExists.into())
}

#[cfg(test)]
mod shell_file_tests {
    use super::*;

    /// Temporary home removed when the test ends.
    struct Home(std::path::PathBuf);
    impl Home {
        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }
    impl Drop for Home {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn exports_go_to_the_xdg_download_dir_and_never_overwrite() {
        let root = std::env::temp_dir().join(format!("odx-export-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let home = Home(root);
        let config = home.path().join(".config");
        std::fs::create_dir_all(home.path().join("Descargas")).unwrap();
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(
            config.join("user-dirs.dirs"),
            "XDG_DESKTOP_DIR=\"$HOME/Escritorio\"\nXDG_DOWNLOAD_DIR=\"$HOME/Descargas\"\n",
        )
        .unwrap();
        let dir = downloads_dir(home.path(), &config);
        assert_eq!(dir, home.path().join("Descargas"));
        let first = write_new(&dir, "odx", "csv", b"one").unwrap();
        let second = write_new(&dir, "odx", "csv", b"two").unwrap();
        assert_eq!(first, dir.join("odx.csv"));
        assert_eq!(second, dir.join("odx-1.csv"));
        assert_eq!(std::fs::read(&first).unwrap(), b"one");
        // A configured folder that does not exist falls back to the home directory.
        std::fs::remove_file(&first).unwrap();
        std::fs::remove_file(&second).unwrap();
        std::fs::remove_dir(home.path().join("Descargas")).unwrap();
        assert_eq!(downloads_dir(home.path(), &config), home.path());
    }

    #[test]
    fn export_names_are_plain_files_with_known_extensions() {
        assert_eq!(
            export_name("odx-res.partner-1.csv"),
            Some(("odx-res.partner-1", "csv"))
        );
        for name in [
            "../x.csv",
            "a/b.json",
            ".hidden.csv",
            "x.exe",
            "x",
            "x.csv/",
        ] {
            assert!(export_name(name).is_none(), "{name}");
        }
    }
}
