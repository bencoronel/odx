mod commands;
mod desktop;
mod services;
use std::sync::Arc;
use tauri::Manager;

/// Dev builds use their own socket so their connector never reaches a release app, and vice versa.
pub const SOCKET_DIR: &str = if cfg!(debug_assertions) {
    "odx-dev"
} else {
    "odx"
};

/// `Odx mcp-stdio` runs the connector without opening a window or vault.
fn mcp_stdio() -> ! {
    use odx_core::transports::{connector, socket};
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("connector runtime");
    let code = runtime.block_on(async {
        let stdin = tokio::io::BufReader::new(tokio::io::stdin());
        let path = match socket::default_socket_path(SOCKET_DIR) {
            Ok(path) => path,
            Err(e) => {
                eprintln!("odx mcp-stdio: {e}");
                connector::report(stdin, tokio::io::stdout(), &e).await;
                return 1;
            }
        };
        match connector::run(&path, stdin, tokio::io::stdout()).await {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("odx mcp-stdio: {e}");
                1
            }
        }
    });
    std::process::exit(code)
}

/// Notify new requests. If notifications fail, show availability in the window; requests remain in Approvals and the status bar.
fn notify_approvals(
    app: tauri::AppHandle,
    mut notices: tokio::sync::broadcast::Receiver<odx_core::ApprovalNotice>,
) {
    use tauri::Emitter;
    tauri::async_runtime::spawn(async move {
        loop {
            let notice = match notices.recv().await {
                Ok(notice) => notice,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            };
            let _ = app.emit("approvals-changed", &notice.approval_id);
            let who = notice.client_label.as_deref().unwrap_or("A client");
            desktop::notify(
                &app,
                "Pending approval",
                format!(
                    "{who} requests {} on {} · {}",
                    notice.method, notice.model, notice.environment_name
                ),
            );
            if let Some(window) = app.get_webview_window("main") {
                let _ =
                    window.request_user_attention(Some(tauri::UserAttentionType::Informational));
            }
        }
    });
}

fn main() {
    if let Err(error) = odx_core::security::harden_process() {
        eprintln!("odx: could not enable process protection: {error}");
        std::process::exit(1);
    }
    if desktop::launch_mode(std::env::args()) == desktop::Launch::Connector {
        mcp_stdio();
    }
    let desktop = Arc::new(desktop::Desktop::default());
    let mut context = tauri::generate_context!();
    if cfg!(debug_assertions) {
        // Separate identifier: own app data (vault, outputs, webview storage) and single-instance name.
        context.config_mut().identifier.push_str(".dev");
    }
    tauri::Builder::default()
        // Check single-instance state before opening a window, vault or socket; focus the existing instance.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            desktop::show_main(app)
        }))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(desktop.clone())
        .on_window_event(|window, event| match event {
            tauri::WindowEvent::CloseRequested { api, .. } => {
                desktop::on_close_requested(window, api)
            }
            tauri::WindowEvent::Destroyed if window.label() == "main" => {
                desktop::on_destroyed(window.app_handle())
            }
            tauri::WindowEvent::Moved(_) | tauri::WindowEvent::Resized(_) => {
                desktop::remember_main_bounds(window)
            }
            _ => {}
        })
        .setup(move |app| {
            desktop::configure_main(app.handle());
            desktop::setup_tray(app, &desktop);
            desktop::probe_notifications(app.handle().clone());
            desktop::lock_on_sleep(app.handle().clone());
            desktop::lock_on_screen_lock(app.handle().clone());
            let data_dir = if cfg!(debug_assertions) {
                match std::env::var_os("ODX_TEST_DATA_DIR") {
                    Some(value) => {
                        let dir = std::path::PathBuf::from(value);
                        if !dir.is_absolute() {
                            return Err(std::io::Error::new(
                                std::io::ErrorKind::InvalidInput,
                                "ODX_TEST_DATA_DIR must be absolute",
                            )
                            .into());
                        }
                        dir
                    }
                    None => app.path().app_data_dir()?,
                }
            } else {
                app.path().app_data_dir()?
            };
            let path = data_dir.join("vault.sqlite3");
            std::fs::create_dir_all(path.parent().expect("app data path"))?;
            let core = odx_core::Core::new(path).expect("core setup");
            let services = Arc::new(services::Services::default());
            {
                let services = services.clone();
                let core = core.clone();
                tauri::async_runtime::spawn(async move { services.start_socket(core).await });
            }
            app.manage(services);
            notify_approvals(app.handle().clone(), core.approval_notices());
            app.manage(core);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::desktop_ready,
            commands::desktop_restore,
            commands::desktop_suspend,
            commands::desktop_cancel_suspend,
            commands::context_get,
            commands::context_set,
            commands::headers_get,
            commands::headers_set,
            commands::proxies_get,
            commands::proxy_save,
            commands::proxy_delete,
            commands::proxy_test,
            commands::proxy_default_set,
            commands::environment_proxy_get,
            commands::environment_proxy_set,
            commands::vault_status,
            commands::vault_create,
            commands::vault_unlock,
            commands::vault_change_password,
            commands::vault_lock,
            commands::vault_reset,
            commands::service_status,
            commands::activity_list,
            commands::shell_call,
            commands::shell_history_list,
            commands::shell_history_add,
            commands::shell_history_clear,
            commands::shell_export,
            commands::shell_sessions_load,
            commands::shell_sessions_save,
            commands::environments_list,
            commands::environment_save,
            commands::environment_update,
            commands::environment_clone,
            commands::environment_delete,
            commands::environment_write_mode,
            commands::environment_check,
            commands::environment_check_draft,
            commands::environment_databases,
            commands::environment_tags_set,
            commands::environment_favicon_page,
            commands::environment_favicon_image,
            commands::environment_open_url,
            commands::updates_open_releases,
            commands::clients_list,
            commands::client_update,
            commands::approvals_list,
            commands::approval_decide,
            commands::rules_list,
            commands::rule_save,
            commands::rule_delete,
            commands::desktop_status,
            commands::desktop_restart,
            commands::settings_get,
            commands::settings_save
        ])
        .build(context)
        .expect("Odx failed")
        .run(|app, event| {
            if let tauri::RunEvent::ExitRequested { api, code, .. } = event
                && desktop::hold_exit(app, code)
            {
                api.prevent_exit();
            }
        });
}
