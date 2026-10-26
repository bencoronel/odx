use odx_core::{Capability, Core, EnvironmentDraft, WriteMode};
use serde_json::Value;
use std::sync::Arc;
use tauri::State;
use zeroize::Zeroizing;

pub use crate::error::AppError;
use crate::files::{MAX_EXPORT, RecordRef, downloads_dir, export_name, write_new, xdg_open};

type Services<'a> = State<'a, Arc<crate::services::Services>>;

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
pub async fn vault_lock(app: tauri::AppHandle) -> Result<(), AppError> {
    // The same path as sleep and screen lock: one lock at a time, after the window saves its Terminal.
    crate::desktop::lock(&app).await;
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
#[tauri::command]
pub async fn activity_detail(
    request_id: String,
    core: State<'_, Arc<Core>>,
) -> Result<Option<odx_core::audit::AuditDetails>, AppError> {
    core.activity_details(request_id)
        .await
        .map_err(AppError::from)
}
/// Desktop-only admin Shell with coded errors.
#[tauri::command]
pub async fn shell_call(
    call: odx_core::ShellCall,
    window: tauri::Window,
    core: State<'_, Arc<Core>>,
) -> Result<odx_core::ShellResult, AppError> {
    // Table windows only repeat a read to add a column: never a confirmed write or an agent's identity.
    if window.label() != "main" && (call.confirmed || call.client_id.is_some()) {
        return Err(AppError::tool(odx_core::ToolError::invalid(
            "only the main window can confirm writes or act for a client",
        )));
    }
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
    let record = record.as_ref().map(|r| (r.model.as_str(), r.id));
    xdg_open(&core.web_url(&id, record).await?)
}
/// Release page for installs the updater cannot replace (.deb, .rpm, development builds).
#[tauri::command]
pub fn updates_open_releases() -> Result<(), AppError> {
    xdg_open("https://github.com/bencoronel/odx/releases")
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
pub fn mcp_disconnect(core: State<'_, Arc<Core>>, id: u64) -> bool {
    core.close_connection(id)
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
    mode: WriteMode,
    minutes: Option<u32>,
    core: State<'_, Arc<Core>>,
) -> Result<(), AppError> {
    if mode == WriteMode::Temporary {
        let minutes = minutes.ok_or_else(|| AppError::new("app.invalid_input"))?;
        return core
            .set_temporary_write(&id, minutes)
            .await
            .map_err(AppError::from);
    }
    core.set_mode(&id, mode).await.map_err(AppError::from)
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

#[tauri::command]
pub async fn desktop_restore(
    app: tauri::AppHandle,
    core: State<'_, Arc<Core>>,
) -> Result<Option<String>, AppError> {
    Ok(crate::desktop::restore_window_state(
        &app,
        crate::desktop::unlocked(&core).await,
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
        crate::desktop::unlocked(&core).await,
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
pub fn desktop_set_texts(app: tauri::AppHandle, texts: crate::desktop::DesktopTexts) {
    crate::desktop::set_texts(&app, texts);
}

#[tauri::command]
pub fn window_keep_above_supported() -> bool {
    crate::desktop::keep_above_supported()
}

/// Show the calling window and bring it to the front after the first paint of a new WebView.
#[tauri::command]
pub async fn window_present(window: tauri::WebviewWindow) {
    let _ = tauri::async_runtime::spawn_blocking(move || crate::desktop::present(&window)).await;
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
pub fn agent_take(app: tauri::AppHandle) -> Vec<crate::desktop::AgentRequest> {
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
