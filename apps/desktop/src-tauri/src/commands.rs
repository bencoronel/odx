use odx_core::{Capability, Core, EnvironmentDraft, WriteMode};
use serde::Serialize;
use serde_json::{Value, json};
use std::sync::Arc;
use tauri::State;
use zeroize::Zeroizing;

type Services<'a> = State<'a, Arc<crate::services::Services>>;

/// Private desktop IPC error. External API/MCP error envelopes remain unchanged.
#[derive(Debug, Clone, Serialize)]
pub struct AppError {
    pub code: String,
    #[serde(skip_serializing_if = "serde_json::Value::is_null")]
    pub params: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<&'static str>,
}

impl AppError {
    fn new(code: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            params: json!({}),
            detail: None,
            outcome: None,
        }
    }

    fn tool(error: odx_core::ToolError) -> Self {
        let ui_code = error.ui_code;
        let detail = matches!(
            error.code,
            "upstream_error" | "upstream_network" | "upstream_timeout"
        )
        .then_some(error.message);
        Self {
            code: ui_code.unwrap_or(error.code).into(),
            params: if ui_code.is_some() {
                error.ui_params
            } else {
                json!({})
            },
            detail,
            outcome: Some(match error.outcome {
                odx_core::output::Outcome::NotSent => "not_sent",
                odx_core::output::Outcome::Failed => "failed",
                odx_core::output::Outcome::Uncertain => "uncertain",
            }),
        }
    }
}

impl From<odx_core::error::OdxError> for AppError {
    fn from(error: odx_core::error::OdxError) -> Self {
        let outcome = match &error {
            odx_core::error::OdxError::Uncertain => Some("uncertain"),
            odx_core::error::OdxError::Remote(_)
            | odx_core::error::OdxError::Network { .. }
            | odx_core::error::OdxError::HttpStatus { .. } => Some("failed"),
            _ => Some("not_sent"),
        };
        Self {
            code: error.code().into(),
            params: error.ui_params(),
            detail: error.safe_detail().map(str::to_owned),
            outcome,
        }
    }
}

impl From<std::io::Error> for AppError {
    fn from(_: std::io::Error) -> Self {
        Self::new("system.operation_failed")
    }
}
impl From<String> for AppError {
    fn from(_: String) -> Self {
        Self::new("app.operation_failed")
    }
}
impl From<&str> for AppError {
    fn from(_: &str) -> Self {
        Self::new("app.operation_failed")
    }
}

#[tauri::command]
pub async fn vault_status(core: State<'_, Arc<Core>>) -> Result<&'static str, AppError> {
    Ok(core.status().await)
}
#[tauri::command]
pub async fn vault_create(password: String, core: State<'_, Arc<Core>>) -> Result<(), AppError> {
    let password = Zeroizing::new(password);
    let result = core.create(&password).await.map_err(AppError::from);
    drop(password);
    result
}
#[tauri::command]
pub async fn vault_unlock(password: String, core: State<'_, Arc<Core>>) -> Result<(), AppError> {
    let password = Zeroizing::new(password);
    let result = core.unlock(&password).await.map_err(AppError::from);
    drop(password);
    result
}
#[tauri::command]
pub async fn vault_change_password(
    current_password: String,
    new_password: String,
    core: State<'_, Arc<Core>>,
) -> Result<(), AppError> {
    let current_password = Zeroizing::new(current_password);
    let new_password = Zeroizing::new(new_password);
    core.change_password(&current_password, &new_password)
        .await
        .map_err(|_| AppError::new("app.password_change_failed"))
}
#[tauri::command]
pub async fn vault_reset(core: State<'_, Arc<Core>>) -> Result<(), AppError> {
    core.reset().await.map_err(AppError::from)
}
#[tauri::command]
pub async fn vault_lock(app: tauri::AppHandle, core: State<'_, Arc<Core>>) -> Result<(), AppError> {
    crate::desktop::clear_window_state(&app);
    core.lock().await;
    Ok(())
}
#[tauri::command]
pub async fn activity_list(
    cursor: Option<odx_core::ActivityCursor>,
    filter: odx_core::ActivityFilter,
    core: State<'_, Arc<Core>>,
) -> Result<odx_core::ActivityPage, AppError> {
    core.activity(cursor, filter).await.map_err(AppError::from)
}
/// Desktop-only admin Shell with coded errors.
#[tauri::command]
pub async fn shell_call(
    call: odx_core::ShellCall,
    core: State<'_, Arc<Core>>,
) -> Result<odx_core::ShellResult, AppError> {
    core.shell_call(call).await.map_err(AppError::tool)
}
#[tauri::command]
pub async fn shell_history_list(
    core: State<'_, Arc<Core>>,
) -> Result<Vec<odx_core::HistoryEntry>, AppError> {
    core.shell_history().await.map_err(AppError::from)
}
#[tauri::command]
pub async fn shell_history_add(
    command: String,
    environment: Option<String>,
    core: State<'_, Arc<Core>>,
) -> Result<(), AppError> {
    core.shell_history_add(&command, environment)
        .await
        .map_err(AppError::from)
}
#[tauri::command]
pub async fn shell_history_clear(core: State<'_, Arc<Core>>) -> Result<(), AppError> {
    core.shell_history_clear().await.map_err(AppError::from)
}
/// Shell tabs saved in the vault; `None` before the first save.
#[tauri::command]
pub async fn shell_sessions_load(core: State<'_, Arc<Core>>) -> Result<Option<String>, AppError> {
    core.shell_sessions().await.map_err(AppError::from)
}
#[tauri::command]
pub async fn shell_sessions_save(data: String, core: State<'_, Arc<Core>>) -> Result<(), AppError> {
    core.shell_sessions_save(&data)
        .await
        .map_err(AppError::from)
}
/// Open the saved environment URL by ID. Arbitrary URLs and credentials are excluded.
#[tauri::command]
pub async fn environment_open_url(
    id: String,
    record: Option<RecordRef>,
    core: State<'_, Arc<Core>>,
) -> Result<(), AppError> {
    let url = core
        .environments()
        .await
        .map_err(AppError::from)?
        .into_iter()
        .find(|e| e.id == id)
        .map(|e| e.url)
        .ok_or_else(|| AppError::new("app.environment_not_found"))?;
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(AppError::new("app.invalid_input"));
    }
    let url = match record {
        Some(r) => {
            record_url(&url, &r.model, r.id).ok_or_else(|| AppError::new("app.invalid_input"))?
        }
        None => url,
    };
    xdg_open(&url)
}
/// Release page for installs the updater cannot replace (.deb, .rpm, development builds).
#[tauri::command]
pub fn updates_open_releases() -> Result<(), AppError> {
    xdg_open("https://github.com/bencoronel/odx/releases")
}
fn xdg_open(url: &str) -> Result<(), AppError> {
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
    model: String,
    id: u64,
}

/// The legacy `/web#` form URL works in Odoo 16 to 19 (newer versions redirect it).
fn record_url(base: &str, model: &str, id: u64) -> Option<String> {
    let valid = !model.is_empty()
        && model.len() <= 128
        && !model.starts_with('.')
        && !model.ends_with('.')
        && model
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.');
    (valid && id > 0).then(|| {
        format!(
            "{}/web#id={id}&model={model}&view_type=form",
            base.trim_end_matches('/')
        )
    })
}

const MAX_EXPORT: usize = 16 * 1024 * 1024;

/// Plain file names only (no paths), with a known extension.
fn export_name(name: &str) -> Option<(&str, &str)> {
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
fn downloads_dir(home: &std::path::Path, config: &std::path::Path) -> std::path::PathBuf {
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
fn write_new(
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

/// Save shell output to the downloads folder without overwriting; returns the path.
#[tauri::command]
pub async fn shell_export(name: String, content: String) -> Result<String, AppError> {
    let (stem, ext) = export_name(&name).ok_or_else(|| AppError::new("app.invalid_input"))?;
    if content.len() > MAX_EXPORT {
        return Err(AppError::new("app.invalid_input"));
    }
    let home = std::path::PathBuf::from(
        std::env::var_os("HOME").ok_or_else(|| AppError::new("system.operation_failed"))?,
    );
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| home.join(".config"));
    let path = write_new(
        &downloads_dir(&home, &config),
        stem,
        ext,
        content.as_bytes(),
    )
    .map_err(|_| AppError::new("system.operation_failed"))?;
    Ok(path.display().to_string())
}

#[tauri::command]
pub async fn service_status(
    services: Services<'_>,
    core: State<'_, Arc<Core>>,
) -> Result<crate::services::ServiceStatus, AppError> {
    Ok(services.status(&core).await)
}
#[tauri::command]
pub async fn environments_list(
    core: State<'_, Arc<Core>>,
) -> Result<Vec<odx_core::EnvironmentView>, AppError> {
    core.environments().await.map_err(AppError::from)
}
#[tauri::command]
pub async fn environment_save(
    draft: EnvironmentDraft,
    core: State<'_, Arc<Core>>,
) -> Result<odx_core::EnvironmentView, AppError> {
    core.save_environment(draft).await.map_err(AppError::from)
}
#[tauri::command]
pub async fn environment_update(
    id: String,
    draft: EnvironmentDraft,
    core: State<'_, Arc<Core>>,
) -> Result<odx_core::EnvironmentView, AppError> {
    core.update_environment(&id, draft)
        .await
        .map_err(AppError::from)
}
#[tauri::command]
pub async fn environment_clone(
    id: String,
    new_name: String,
    core: State<'_, Arc<Core>>,
) -> Result<odx_core::EnvironmentView, AppError> {
    core.clone_environment(&id, &new_name)
        .await
        .map_err(AppError::from)
}
#[tauri::command]
pub async fn environment_delete(
    id: String,
    confirmed: bool,
    core: State<'_, Arc<Core>>,
) -> Result<(), AppError> {
    core.delete_environment(&id, confirmed)
        .await
        .map_err(AppError::from)
}
#[tauri::command]
pub async fn environment_check(
    id: String,
    core: State<'_, Arc<Core>>,
) -> Result<odx_core::HealthReport, AppError> {
    core.check_environment(&id).await.map_err(AppError::from)
}
#[tauri::command]
pub async fn environment_check_draft(
    draft: EnvironmentDraft,
    proxy: Option<odx_core::proxy::EnvironmentProxyInput>,
    core: State<'_, Arc<Core>>,
) -> Result<odx_core::HealthReport, AppError> {
    let proxy = proxy.unwrap_or(odx_core::proxy::EnvironmentProxyInput::Inherit);
    core.check_draft_with_proxy(draft, proxy)
        .await
        .map_err(AppError::from)
}
#[tauri::command]
pub async fn environment_databases(
    url: String,
    core: State<'_, Arc<Core>>,
) -> Result<Vec<String>, AppError> {
    core.list_databases(&url).await.map_err(AppError::from)
}
#[tauri::command]
pub async fn environment_tags_set(
    id: String,
    tags: Vec<String>,
    core: State<'_, Arc<Core>>,
) -> Result<odx_core::EnvironmentView, AppError> {
    core.set_environment_tags(&id, &tags)
        .await
        .map_err(AppError::from)
}
#[tauri::command]
pub async fn clients_list(
    core: State<'_, Arc<Core>>,
) -> Result<Vec<odx_core::ClientView>, AppError> {
    core.clients().await.map_err(AppError::from)
}
#[tauri::command]
pub async fn client_update(
    id: String,
    label: String,
    // `None` includes all current and future environments.
    environment_ids: Option<Vec<String>>,
    capabilities: Vec<Capability>,
    core: State<'_, Arc<Core>>,
) -> Result<odx_core::ClientView, AppError> {
    let scope = environment_ids.map_or(odx_core::ClientScope::All, Into::into);
    core.update_client(&id, &label, scope, capabilities)
        .await
        .map_err(AppError::from)
}
#[tauri::command]
pub async fn environment_write_mode(
    id: String,
    mode: String,
    minutes: Option<u32>,
    core: State<'_, Arc<Core>>,
) -> Result<(), AppError> {
    if mode == "temporary" {
        return core
            .set_temporary_write(
                &id,
                minutes.ok_or_else(|| AppError::new("app.invalid_input"))?,
            )
            .await
            .map_err(AppError::from);
    }
    let m = match mode.as_str() {
        "read_only" => WriteMode::ReadOnly,
        "approval" => WriteMode::Approval,
        "permanent" => WriteMode::Permanent,
        _ => return Err(AppError::new("app.invalid_input")),
    };
    core.set_mode(&id, m).await.map_err(AppError::from)
}
#[tauri::command]
pub async fn approvals_list(
    core: State<'_, Arc<Core>>,
) -> Result<Vec<odx_core::PendingApproval>, AppError> {
    core.approvals().await.map_err(AppError::from)
}
#[tauri::command]
pub async fn approval_decide(
    id: String,
    approve: bool,
    reason: Option<String>,
    core: State<'_, Arc<Core>>,
) -> Result<Value, AppError> {
    if approve {
        core.decide(&id, true).await
    } else {
        core.reject(&id, reason).await
    }
    .map_err(AppError::from)
}
#[tauri::command]
pub async fn rules_list(
    environment_id: Option<String>,
    core: State<'_, Arc<Core>>,
) -> Result<Vec<odx_core::RuleView>, AppError> {
    core.rules(environment_id.as_deref())
        .await
        .map_err(AppError::from)
}
#[tauri::command]
pub async fn rule_save(
    draft: odx_core::RuleDraft,
    core: State<'_, Arc<Core>>,
) -> Result<odx_core::RuleView, AppError> {
    core.save_rule(draft).await.map_err(AppError::from)
}
#[tauri::command]
pub async fn rule_delete(id: String, core: State<'_, Arc<Core>>) -> Result<(), AppError> {
    core.delete_rule(&id).await.map_err(AppError::from)
}

#[tauri::command]
pub async fn headers_get(
    environment_id: Option<String>,
    core: State<'_, Arc<Core>>,
) -> Result<odx_core::headers::HeaderSettings, AppError> {
    core.header_settings(environment_id.as_deref())
        .await
        .map_err(AppError::from)
}
#[tauri::command]
pub async fn headers_set(
    environment_id: Option<String>,
    headers: Vec<odx_core::headers::HeaderEntry>,
    core: State<'_, Arc<Core>>,
) -> Result<(), AppError> {
    core.set_headers(environment_id.as_deref(), headers)
        .await
        .map_err(AppError::from)
}

/// Named proxies with credential placeholders.
#[tauri::command]
pub async fn proxies_get(
    core: State<'_, Arc<Core>>,
) -> Result<odx_core::proxy::ProxiesView, AppError> {
    core.proxies().await.map_err(AppError::from)
}
#[tauri::command]
pub async fn proxy_save(
    id: Option<String>,
    proxy: odx_core::proxy::ProxyInput,
    core: State<'_, Arc<Core>>,
) -> Result<String, AppError> {
    core.save_proxy(id, proxy).await.map_err(AppError::from)
}
/// Reach an environment URL through an unsaved proxy.
#[tauri::command]
pub async fn proxy_test(
    id: Option<String>,
    proxy: odx_core::proxy::ProxyInput,
    url: String,
    core: State<'_, Arc<Core>>,
) -> Result<odx_core::proxy::ProxyTestReport, AppError> {
    core.test_proxy(id, proxy, &url)
        .await
        .map_err(AppError::from)
}
#[tauri::command]
pub async fn tag_colors_get(
    core: State<'_, Arc<Core>>,
) -> Result<std::collections::BTreeMap<String, String>, AppError> {
    core.tag_colors().await.map_err(AppError::from)
}
#[tauri::command]
pub async fn tag_color_set(
    tag: String,
    color: Option<String>,
    core: State<'_, Arc<Core>>,
) -> Result<(), AppError> {
    core.set_tag_color(&tag, color.as_deref())
        .await
        .map_err(AppError::from)
}
#[tauri::command]
pub async fn proxy_delete(id: String, core: State<'_, Arc<Core>>) -> Result<(), AppError> {
    core.delete_proxy(&id).await.map_err(AppError::from)
}
#[tauri::command]
pub async fn proxy_default_set(
    id: Option<String>,
    core: State<'_, Arc<Core>>,
) -> Result<(), AppError> {
    core.set_default_proxy(id).await.map_err(AppError::from)
}
#[tauri::command]
pub async fn environment_proxy_get(
    id: String,
    core: State<'_, Arc<Core>>,
) -> Result<odx_core::proxy::EnvironmentProxyView, AppError> {
    core.environment_proxy(&id).await.map_err(AppError::from)
}
#[tauri::command]
pub async fn environment_proxy_set(
    id: String,
    proxy: odx_core::proxy::EnvironmentProxyInput,
    core: State<'_, Arc<Core>>,
) -> Result<(), AppError> {
    core.set_environment_proxy(&id, proxy)
        .await
        .map_err(AppError::from)
}

#[tauri::command]
pub async fn context_get(
    environment_id: Option<String>,
    core: State<'_, Arc<Core>>,
) -> Result<odx_core::context::ContextSettings, AppError> {
    core.context_settings(environment_id.as_deref())
        .await
        .map_err(AppError::from)
}
#[tauri::command]
pub async fn context_set(
    environment_id: Option<String>,
    entries: Vec<odx_core::context::ContextEntry>,
    core: State<'_, Arc<Core>>,
) -> Result<(), AppError> {
    core.set_context(environment_id.as_deref(), entries)
        .await
        .map_err(AppError::from)
}

#[tauri::command]
pub async fn environment_favicon_image(
    id: String,
    url: String,
    core: State<'_, Arc<Core>>,
) -> Result<odx_core::favicon::FaviconImage, AppError> {
    core.favicon_image(&id, &url).await.map_err(AppError::from)
}

#[tauri::command]
pub async fn environment_favicon_page(
    id: String,
    core: State<'_, Arc<Core>>,
) -> Result<odx_core::favicon::FaviconPage, AppError> {
    core.favicon_page(&id).await.map_err(AppError::from)
}
/// Install-and-restart path for updates: lock and drain like Quit, then relaunch.
#[tauri::command]
pub fn desktop_restart(app: tauri::AppHandle) {
    crate::desktop::quit(&app, true);
}
/// Expose tray and notification availability to the window.
#[tauri::command]
pub fn desktop_status(
    desktop: State<'_, Arc<crate::desktop::Desktop>>,
) -> crate::desktop::DesktopStatus {
    desktop.status()
}
/// Settings are editable only from the window, never API/MCP clients.
#[tauri::command]
pub async fn settings_get(core: State<'_, Arc<Core>>) -> Result<odx_core::SettingsView, AppError> {
    core.settings().await.map_err(AppError::from)
}
#[tauri::command]
pub async fn settings_save(
    settings: odx_core::Settings,
    core: State<'_, Arc<Core>>,
) -> Result<odx_core::SettingsView, AppError> {
    core.save_settings(settings).await.map_err(AppError::from)
}

#[cfg(test)]
mod shell_file_tests {
    use super::*;

    #[test]
    fn record_urls_accept_only_model_names_and_positive_ids() {
        assert_eq!(
            record_url("https://odoo.test/", "res.partner", 7).as_deref(),
            Some("https://odoo.test/web#id=7&model=res.partner&view_type=form")
        );
        for model in ["", "Res.Partner", "res.partner&x=1", ".res", "res/../x"] {
            assert!(
                record_url("https://odoo.test", model, 7).is_none(),
                "{model}"
            );
        }
        assert!(record_url("https://odoo.test", "res.partner", 0).is_none());
    }

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

#[cfg(test)]
mod error_contract_tests {
    use super::*;
    use odx_core::error::OdxError;

    #[test]
    fn invalid_input_does_not_leak_the_original_value() {
        let error = AppError::from(OdxError::Invalid("api_key=secret-value".into()));
        assert_eq!(error.code, "app.invalid_input");
        assert!(error.detail.is_none());
        assert!(
            !serde_json::to_string(&error)
                .unwrap()
                .contains("secret-value")
        );
    }

    #[test]
    fn validation_errors_keep_stable_codes_and_safe_params_for_the_desktop() {
        let error = AppError::from(OdxError::validation_with_params(
            "validation.settings.out_of_range",
            json!({ "setting": "global_concurrency", "min": 1, "max": 64, "api_key": "secret" }),
            "legacy validation message containing api_key=secret",
        ));
        assert_eq!(error.code, "validation.settings.out_of_range");
        assert_eq!(
            error.params,
            json!({ "setting": "global_concurrency", "min": 1, "max": 64 })
        );
        assert!(error.detail.is_none());
        let serialized = serde_json::to_string(&error).unwrap();
        assert!(!serialized.contains("legacy validation message"));
        assert!(!serialized.contains("secret"));
    }

    #[test]
    fn upstream_details_and_uncertain_outcomes_keep_their_contract() {
        let remote = AppError::from(OdxError::Remote("access denied; key=[REDACTED]".into()));
        assert_eq!(remote.code, "upstream.remote");
        assert_eq!(
            remote.detail.as_deref(),
            Some("access denied; key=[REDACTED]")
        );

        let uncertain = AppError::from(OdxError::Uncertain);
        assert_eq!(uncertain.code, "upstream.uncertain_result");
        assert_eq!(uncertain.outcome, Some("uncertain"));
        assert!(uncertain.detail.is_none());
    }

    #[test]
    fn tool_uncertain_result_does_not_include_backend_text() {
        let error = AppError::tool(odx_core::ToolError::new(
            "uncertain_result",
            "localized by the UI",
            odx_core::Outcome::Uncertain,
        ));
        assert_eq!(error.code, "uncertain_result");
        assert_eq!(error.outcome, Some("uncertain"));
        assert!(error.detail.is_none());
    }
}

#[tauri::command]
pub async fn desktop_restore(
    app: tauri::AppHandle,
    core: State<'_, Arc<Core>>,
) -> Result<Option<String>, AppError> {
    Ok(crate::desktop::restore_window_state(
        &app,
        core.status().await == "unlocked",
    ))
}
#[tauri::command]
pub async fn desktop_suspend(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    core: State<'_, Arc<Core>>,
    ticket: u64,
    snapshot: String,
) -> Result<bool, AppError> {
    crate::desktop::suspend_main(
        &app,
        &window,
        ticket,
        snapshot,
        core.status().await == "unlocked",
    )
    .map_err(AppError::from)
}
#[tauri::command]
pub fn desktop_flushed(app: tauri::AppHandle) {
    crate::desktop::flushed(&app);
}
#[tauri::command]
pub fn desktop_cancel_suspend(app: tauri::AppHandle, ticket: u64) {
    crate::desktop::cancel_suspend(&app, ticket);
}

#[tauri::command]
pub async fn agent_setup(agent: crate::services::Agent) -> crate::services::AgentSetup {
    tauri::async_runtime::spawn_blocking(move || crate::services::agent_setup(agent))
        .await
        .unwrap_or(crate::services::AgentSetup::NoCli)
}

/// Runs the agent's own `mcp add`; the error detail is the CLI's message, e.g. a name already taken in another scope.
#[tauri::command]
pub async fn agent_install(
    agent: crate::services::Agent,
) -> Result<crate::services::AgentSetup, AppError> {
    tauri::async_runtime::spawn_blocking(move || crate::services::agent_install(agent))
        .await
        .map_err(|_| AppError::new("system.operation_failed"))?
        .map_err(|detail| AppError {
            detail: Some(detail),
            ..AppError::new("system.agent_install_failed")
        })
}

#[tauri::command]
pub fn desktop_set_language(app: tauri::AppHandle, language: String) {
    crate::desktop::set_language(&app, &language);
}

#[tauri::command]
pub fn window_keep_above_supported() -> bool {
    crate::desktop::keep_above_supported()
}

/// Bring the calling window to the front after it shows itself (first paint of a new WebView).
#[tauri::command]
pub async fn window_present(window: tauri::WebviewWindow) {
    let _ = tauri::async_runtime::spawn_blocking(move || crate::desktop::activate(&window)).await;
}

/// Keep the calling window above the others (or not).
#[tauri::command]
pub async fn window_keep_above(window: tauri::WebviewWindow, above: bool) -> Result<(), AppError> {
    tauri::async_runtime::spawn_blocking(move || crate::desktop::keep_above(&window, above))
        .await
        .map_err(|_| AppError::new("system.operation_failed"))?
        .map_err(|detail| AppError {
            detail: Some(detail),
            ..AppError::new("system.operation_failed")
        })
}

#[tauri::command]
pub fn table_window_open(
    app: tauri::AppHandle,
    table: serde_json::Value,
    title: String,
) -> Result<(), AppError> {
    crate::desktop::open_table_window(&app, table, &title)
        .map_err(|_| AppError::new("system.operation_failed"))
}

/// The table opened in the calling window; `None` once taken or for any other window.
#[tauri::command]
pub fn table_window_take(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
) -> Option<serde_json::Value> {
    crate::desktop::take_table(&app, window.label())
}

/// Agent requests waiting for the window (run a Terminal command, show a tab).
#[tauri::command]
pub fn agent_take(app: tauri::AppHandle) -> Vec<serde_json::Value> {
    crate::desktop::take_agent_requests(&app)
}

#[tauri::command]
pub fn agent_reply(
    app: tauri::AppHandle,
    id: u64,
    result: Option<serde_json::Value>,
    error: Option<String>,
) {
    crate::desktop::answer_agent(&app, id, result, error);
}

#[tauri::command]
pub fn desktop_take_approval(app: tauri::AppHandle) -> Option<String> {
    crate::desktop::take_open_approval(&app)
}

#[tauri::command]
pub fn desktop_ready(app: tauri::AppHandle) -> Option<u64> {
    crate::desktop::pending_suspend(&app)
}
