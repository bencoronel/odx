//! Desktop services: MCP socket listens while locked; the HTTP API is disabled.
use odx_core::{
    Core,
    transports::{http, socket},
};
use serde::Serialize;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Default)]
struct State {
    socket_path: Option<String>,
    socket_error: Option<String>,
}

#[derive(Default)]
pub struct Services {
    state: Mutex<State>,
}

#[derive(Serialize)]
pub struct Service {
    state: &'static str,
    address: Option<String>,
    error: Option<String>,
}

#[derive(Serialize)]
pub struct ServiceStatus {
    api: Service,
    mcp: Service,
    /// MCP connector launch configuration.
    connector_command: String,
    connector_args: [&'static str; 1],
    api_url: String,
    /// Currently connected MCP clients.
    connections: Vec<odx_core::McpConnectionInfo>,
}

/// Stable executable path; use the AppImage when packaged.
fn connector_command() -> String {
    std::env::var("APPIMAGE")
        .ok()
        .filter(|p| !p.is_empty())
        .or_else(|| {
            std::env::current_exe()
                .ok()
                .map(|p| p.display().to_string())
        })
        .unwrap_or_else(|| "odx".into())
}

impl Services {
    pub async fn start_socket(&self, core: Arc<Core>) {
        let result = match socket::default_socket_path(crate::SOCKET_DIR) {
            Ok(path) => socket::bind(&path).await.map(|listener| (path, listener)),
            Err(e) => Err(e),
        };
        let mut state = self.state.lock().await;
        match result {
            Ok((path, listener)) => {
                state.socket_path = Some(path.display().to_string());
                state.socket_error = None;
                tauri::async_runtime::spawn(socket::serve(core, listener));
            }
            Err(e) => {
                eprintln!("odx: MCP socket unavailable: {e}");
                state.socket_error = Some("service.mcp_socket_failed".into());
            }
        }
    }

    pub async fn status(&self, core: &Core) -> ServiceStatus {
        let connections = core.mcp_connections().await;
        let state = self.state.lock().await;
        let api_address = format!("127.0.0.1:{}", http::DEFAULT_PORT);
        ServiceStatus {
            api: Service {
                state: "stopped",
                address: None,
                error: None,
            },
            mcp: Service {
                state: if state.socket_error.is_some() {
                    "error"
                } else if state.socket_path.is_some() {
                    "listening"
                } else {
                    "stopped"
                },
                address: state.socket_path.clone(),
                error: state.socket_error.clone(),
            },
            connector_command: connector_command(),
            connector_args: ["mcp-stdio"],
            api_url: format!("http://{api_address}/api/v1"),
            connections,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Desktop never starts HTTP, including after creating or unlocking the vault.
    #[test]
    fn api_stays_stopped_across_vault_lifecycle() {
        tauri::async_runtime::block_on(async {
            let dir = std::env::temp_dir().join(format!(
                "odx-services-{}-{:?}",
                std::process::id(),
                std::time::SystemTime::now()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            let core = Core::new(dir.join("vault.sqlite3")).unwrap();
            let services = Services::default();
            for action in ["absent", "create", "lock", "unlock"] {
                match action {
                    "create" => core.create("fictional master password").await.unwrap(),
                    "lock" => core.lock().await,
                    "unlock" => core.unlock("fictional master password").await.unwrap(),
                    _ => {}
                }
                let status = services.status(&core).await;
                assert_eq!(status.api.state, "stopped", "after {action}");
                assert!(status.api.address.is_none());
                assert!(status.api.error.is_none());
            }
            core.lock().await;
            drop(core);
            std::fs::remove_dir_all(&dir).unwrap();
        });
    }
}
