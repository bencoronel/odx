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
/// Open the saved environment URL by ID. Arbitrary URLs and credentials are excluded.
#[tauri::command]
pub async fn environment_open_url(id: String, core: State<'_, Arc<Core>>) -> Result<(), AppError> {
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
    let mut child = std::process::Command::new("xdg-open")
        .arg(&url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|_| AppError::new("system.browser_open_failed"))?;
    // Reap the child asynchronously to avoid zombies.
    tauri::async_runtime::spawn_blocking(move || child.wait());
    Ok(())
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
    core: State<'_, Arc<Core>>,
) -> Result<Value, AppError> {
    core.decide(&id, approve).await.map_err(AppError::from)
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

/// Global proxy with credential placeholders.
#[tauri::command]
pub async fn proxy_get(
    core: State<'_, Arc<Core>>,
) -> Result<Option<odx_core::proxy::ProxyView>, AppError> {
    core.proxy().await.map_err(AppError::from)
}
#[tauri::command]
pub async fn proxy_set(
    proxy: Option<odx_core::proxy::ProxyInput>,
    core: State<'_, Arc<Core>>,
) -> Result<(), AppError> {
    core.set_proxy(proxy).await.map_err(AppError::from)
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
pub fn desktop_cancel_suspend(app: tauri::AppHandle, ticket: u64) {
    crate::desktop::cancel_suspend(&app, ticket);
}

#[tauri::command]
pub fn desktop_ready(app: tauri::AppHandle) -> Option<u64> {
    crate::desktop::pending_suspend(&app)
}
