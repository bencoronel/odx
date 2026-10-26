#[macro_use]
mod command_list;
mod commands;
mod desktop;
mod error;
mod files;
mod services;
use std::sync::Arc;
use tauri::Manager;

/// Dev builds use their own socket so their connector never reaches a release app, and vice versa.
pub const SOCKET_DIR: &str = if cfg!(debug_assertions) {
    "odx-dev"
} else {
    "odx"
};
/// Dev builds show their own name and register with agent CLIs under their own MCP name, so they run beside a release app.
pub const APP_NAME: &str = if cfg!(debug_assertions) {
    "Odx Dev"
} else {
    "Odx"
};
pub const MCP_NAME: &str = if cfg!(debug_assertions) {
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

macro_rules! register {
    ($($command:ident),* $(,)?) => {
        tauri::generate_handler![$(commands::$command),*]
    };
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
        context.config_mut().product_name = Some(APP_NAME.into());
        // What the window reads with getName().
        context.package_info_mut().name = APP_NAME.into();
        for window in &mut context.config_mut().app.windows {
            window.title = APP_NAME.into();
        }
        context.set_default_window_icon(Some(tauri::include_image!("icons/icon-dev.png")));
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
            // A table window closed before loading must not leave its data behind.
            tauri::WindowEvent::Destroyed if window.label().starts_with("table-") => {
                desktop::take_table(window.app_handle(), window.label());
            }
            tauri::WindowEvent::Moved(_) | tauri::WindowEvent::Resized(_) => {
                desktop::remember_main_bounds(window)
            }
            _ => {}
        })
        .setup(move |app| {
            desktop::place_main(app.handle());
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
            std::fs::create_dir_all(data_dir.as_path())?;
            let core = odx_core::Core::new(path)?;
            let services = Arc::new(services::Services::default());
            {
                let services = services.clone();
                let core = core.clone();
                tauri::async_runtime::spawn(async move { services.start_socket(core).await });
            }
            app.manage(services);
            desktop::notify_approvals(app.handle().clone(), core.clone(), core.approval_notices());
            let (agent_requests, agent_queue) = tokio::sync::mpsc::unbounded_channel();
            core.set_desktop(agent_requests);
            desktop::serve_agents(app.handle().clone(), agent_queue);
            app.manage(core);
            Ok(())
        })
        .invoke_handler(for_each_command!(register))
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
