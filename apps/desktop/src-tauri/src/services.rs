//! Desktop services: MCP socket listens while locked; the HTTP API is disabled.
use odx_core::{
    Core,
    transports::{http, socket},
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};
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
    mcp_name: &'static str,
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

/// Agent CLIs Odx can register itself with, as in the MCP page snippets.
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Agent {
    Claude,
    Codex,
}

/// Whether an agent CLI is installed and points `odx` at this executable.
#[derive(Serialize, PartialEq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum AgentSetup {
    NoCli,
    Missing,
    Added,
    /// Registered with another executable path, e.g. an AppImage renamed or moved since.
    Outdated,
}

const AGENT_TIMEOUT: Duration = Duration::from_secs(20);

impl Agent {
    fn program(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
        }
    }

    /// PATH, plus ~/.local/bin: desktop launchers often miss it and Claude Code installs there.
    fn cli(self) -> Option<PathBuf> {
        let home = std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/bin"));
        std::env::var_os("PATH")
            .map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
            .unwrap_or_default()
            .into_iter()
            .chain(home)
            .map(|dir| dir.join(self.program()))
            .find(|path| path.is_file())
    }

    fn add_args(self, command: &str) -> Vec<String> {
        let mut args: Vec<String> = match self {
            Agent::Claude => [
                "mcp",
                "add",
                "--transport",
                "stdio",
                "--scope",
                "user",
                crate::MCP_NAME,
            ]
            .map(String::from)
            .to_vec(),
            // Codex starts MCP servers with a reduced environment; the connector needs XDG_RUNTIME_DIR to find the app socket.
            Agent::Codex => {
                let mut args = ["mcp", "add", crate::MCP_NAME].map(String::from).to_vec();
                if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
                    args.extend(["--env".into(), format!("XDG_RUNTIME_DIR={dir}")]);
                }
                args
            }
        };
        args.extend(["--".into(), command.into(), "mcp-stdio".into()]);
        args
    }

    fn remove_args(self) -> &'static [&'static str] {
        match self {
            Agent::Claude => &["mcp", "remove", "--scope", "user", crate::MCP_NAME],
            Agent::Codex => &["mcp", "remove", crate::MCP_NAME],
        }
    }
}

/// Run without a shell; a CLI that hangs is killed after `AGENT_TIMEOUT`.
fn run(cli: &PathBuf, args: &[impl AsRef<std::ffi::OsStr>]) -> std::io::Result<Output> {
    let mut child = Command::new(cli)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let start = Instant::now();
    while child.try_wait()?.is_none() {
        if start.elapsed() > AGENT_TIMEOUT {
            let _ = child.kill();
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    child.wait_with_output()
}

/// Blocking: runs `<cli> mcp get <MCP_NAME>`.
pub fn agent_setup(agent: Agent) -> AgentSetup {
    let Some(cli) = agent.cli() else {
        return AgentSetup::NoCli;
    };
    match run(&cli, &["mcp", "get", crate::MCP_NAME]) {
        Ok(out) if out.status.success() => {
            setup_state(&String::from_utf8_lossy(&out.stdout), &connector_command())
        }
        _ => AgentSetup::Missing,
    }
}

fn setup_state(described: &str, command: &str) -> AgentSetup {
    if described.contains(command) {
        AgentSetup::Added
    } else {
        AgentSetup::Outdated
    }
}

/// Blocking: register `MCP_NAME` with the agent, replacing an entry that points elsewhere. Errors carry the CLI's own message.
pub fn agent_install(agent: Agent) -> Result<AgentSetup, String> {
    let cli = agent
        .cli()
        .ok_or_else(|| format!("{} not found", agent.program()))?;
    if agent_setup(agent) == AgentSetup::Outdated {
        let _ = run(&cli, agent.remove_args());
    }
    let out = run(&cli, &agent.add_args(&connector_command())).map_err(|e| e.to_string())?;
    if !out.status.success() {
        let message = String::from_utf8_lossy(if out.stderr.is_empty() {
            &out.stdout
        } else {
            &out.stderr
        });
        return Err(message.trim().chars().take(500).collect());
    }
    Ok(agent_setup(agent))
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
            mcp_name: crate::MCP_NAME,
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

    #[test]
    fn agent_commands_and_state() {
        let codex = Agent::Codex.add_args("/opt/Odx.AppImage");
        assert_eq!(codex[..3], ["mcp", "add", crate::MCP_NAME]);
        assert_eq!(
            codex[codex.len() - 3..],
            ["--", "/opt/Odx.AppImage", "mcp-stdio"]
        );
        let claude = Agent::Claude.add_args("/opt/Odx.AppImage");
        assert!(claude.windows(2).any(|w| w == ["--scope", "user"]));
        assert_eq!(
            setup_state(
                "Command: /opt/Odx.AppImage\nArgs: mcp-stdio",
                "/opt/Odx.AppImage"
            ),
            AgentSetup::Added
        );
        assert_eq!(
            setup_state("Command: /old/Odx_0.1.AppImage", "/opt/Odx.AppImage"),
            AgentSetup::Outdated
        );
    }

    /// Desktop never starts HTTP, including after creating or unlocking the vault.
    #[test]
    fn api_stays_stopped_across_vault_lifecycle() {
        let dir = std::env::temp_dir().join(format!(
            "odx-services-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        // The global Tauri runtime outlives the test process's Rust cleanup. Own this runtime so
        // unlock's blocking SQLCipher pruning finishes before native libraries exit or files are removed.
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
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
        });
        drop(runtime);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
