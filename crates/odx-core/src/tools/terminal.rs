//! Terminal tools: an agent reads the desktop Terminal's sessions and asks the app to run a command or show a tab.
//! Reads come from the sessions the app saves in the vault; running and showing go to the app through
//! [`DesktopRequest`], since the Terminal (its tabs, parser and confirmations) lives in the window.
use crate::{
    clients::{Capability, ClientIdentity},
    execution::Core,
    output::{Outcome, ToolError, ToolOutput},
    tools::args::{Args, common},
};
use serde_json::{Map, Value, json};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

const MAX_COMMAND: usize = 4096;
const SHOW_TIMEOUT: Duration = Duration::from_secs(15);

/// What the core asks of the desktop window. The app answers on `reply`; dropping it reports the window as gone.
#[derive(Debug)]
pub enum DesktopRequest {
    /// Type `command` in `terminal` (or a session of `env_id`) as `client_label`, run it, and reply with its output.
    /// `allow_writes`: the client may request writes; the user still confirms each one in the Terminal.
    Run {
        terminal: Option<String>,
        env_id: String,
        command: String,
        client_id: String,
        /// Token revision the command was requested under; shell_call checks it is still current.
        client_revision: u64,
        client_label: String,
        allow_writes: bool,
        reply: oneshot::Sender<Result<Value, String>>,
    },
    /// Bring the window forward on `terminal`.
    Show {
        terminal: String,
        reply: oneshot::Sender<Result<Value, String>>,
    },
}

/// A saved Terminal tab, as the app stores it (`ShellPage.tsx` `SavedSessions`); only what the tools need.
struct SavedTab {
    id: String,
    name: String,
    env_id: String,
    env_name: String,
    entries: Vec<Value>,
}

fn saved_tabs(data: &str) -> (Vec<SavedTab>, Option<String>) {
    let Ok(saved) = serde_json::from_str::<Value>(data) else {
        return (Vec::new(), None);
    };
    let active = saved
        .get("activeId")
        .and_then(Value::as_str)
        .map(String::from);
    let tabs = saved
        .get("tabs")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|tab| {
            let session = tab.get("session")?;
            // Tabs saved before sessions had a fixed environment carry it only on the session.
            let env_id = tab
                .get("envId")
                .or_else(|| session.pointer("/env/id"))?
                .as_str()?
                .to_owned();
            let env_name = tab
                .get("envName")
                .or_else(|| session.pointer("/env/name"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            let custom = tab.get("defaultName").and_then(Value::as_bool) == Some(false);
            let name = tab
                .get("name")
                .and_then(Value::as_str)
                .filter(|n| custom && !n.is_empty())
                .unwrap_or(&env_name)
                .to_owned();
            Some(SavedTab {
                id: tab.get("id")?.as_str()?.to_owned(),
                name,
                env_id,
                env_name,
                entries: session
                    .get("entries")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default(),
            })
        })
        .collect();
    (tabs, active)
}

/// Commands and what they printed, oldest first: each typed command opens a block.
fn command_blocks(entries: &[Value]) -> Vec<Value> {
    let mut blocks: Vec<Map<String, Value>> = Vec::new();
    for entry in entries {
        let kind = entry.get("kind").and_then(Value::as_str).unwrap_or("");
        if kind == "input" {
            let mut block = Map::new();
            block.insert(
                "command".into(),
                entry.get("text").cloned().unwrap_or(Value::Null),
            );
            if let Some(agent) = entry.get("agent").filter(|a| a.is_string()) {
                block.insert("agent".into(), agent.clone());
            }
            block.insert("output".into(), json!([]));
            blocks.push(block);
            continue;
        }
        let Some(block) = blocks.last_mut() else {
            continue;
        };
        let item = match kind {
            "table" => {
                let table = entry.get("table").cloned().unwrap_or(Value::Null);
                json!({ "columns": table.get("columns"), "rows": table.get("rows"), "model": table.get("model") })
            }
            // The app keeps errors as codes; the code and Odoo's detail are what an agent can act on.
            _ => match entry.get("appError") {
                Some(error) => {
                    json!({ "kind": "error", "text": error.get("detail").or_else(|| error.get("code")) })
                }
                None => json!({ "kind": kind, "text": entry.get("text") }),
            },
        };
        if let Some(Value::Array(output)) = block.get_mut("output") {
            output.push(item);
        }
    }
    blocks.into_iter().map(Value::Object).collect()
}

impl Core {
    /// Connect the desktop window; without it, `terminal_run` and `show` report the app as unavailable.
    pub fn set_desktop(&self, requests: mpsc::UnboundedSender<DesktopRequest>) {
        if let Ok(mut desktop) = self.desktop.lock() {
            *desktop = Some(requests);
        }
    }

    fn send_desktop(&self, request: DesktopRequest) -> Result<(), ToolError> {
        self.desktop
            .lock()
            .ok()
            .and_then(|desktop| desktop.as_ref().map(|tx| tx.send(request).is_ok()))
            .filter(|sent| *sent)
            .map(drop)
            .ok_or_else(|| {
                ToolError::new(
                    "unavailable",
                    "the Odx desktop app is not running",
                    Outcome::NotSent,
                )
            })
    }

    /// Saved tabs on environments this client may use; other tabs do not exist for it.
    async fn client_tabs(
        &self,
        client: &ClientIdentity,
    ) -> Result<(Vec<SavedTab>, Option<String>), ToolError> {
        // Terminal tabs show Odoo records: reading them needs odoo:read, like any read tool.
        if !client.capabilities.contains(&Capability::OdooRead) {
            return Err(ToolError::new(
                "forbidden",
                "client lacks odoo:read capability",
                Outcome::NotSent,
            ));
        }
        let data = self
            .shell_sessions()
            .await
            .map_err(ToolError::from)?
            .unwrap_or_default();
        let (tabs, active) = saved_tabs(&data);
        Ok((
            tabs.into_iter()
                .filter(|tab| client.environment_ids.contains(&tab.env_id))
                .collect(),
            active,
        ))
    }

    async fn client_tab(
        &self,
        client: &ClientIdentity,
        terminal: &str,
    ) -> Result<SavedTab, ToolError> {
        self.client_tabs(client)
            .await?
            .0
            .into_iter()
            .find(|tab| tab.id == terminal)
            .ok_or_else(|| {
                ToolError::new(
                    "not_found",
                    format!("no terminal {terminal}; list them with terminals"),
                    Outcome::NotSent,
                )
            })
    }

    pub(crate) async fn tool_terminals(
        &self,
        client: &ClientIdentity,
        arguments: Value,
    ) -> Result<ToolOutput, ToolError> {
        let mut args = Args::for_tool("terminals", arguments)?;
        let (format, trunc) = common(&mut args)?;
        let (tabs, active) = self.client_tabs(client).await?;
        let data = tabs
            .iter()
            .map(|tab| {
                let blocks = command_blocks(&tab.entries);
                json!({
                    "terminal": tab.id,
                    "name": tab.name,
                    "env": tab.env_name,
                    "active": active.as_deref() == Some(tab.id.as_str()),
                    "commands": blocks.len(),
                    "last_command": blocks.last().and_then(|b| b.get("command")).cloned().unwrap_or(Value::Null),
                })
            })
            .collect();
        Ok(ToolOutput {
            data: Value::Array(data),
            meta: Map::new(),
            format,
            trunc,
        })
    }

    pub(crate) async fn tool_terminal_read(
        &self,
        client: &ClientIdentity,
        arguments: Value,
    ) -> Result<ToolOutput, ToolError> {
        let mut args = Args::for_tool("terminal_read", arguments)?;
        let (format, trunc) = common(&mut args)?;
        let terminal = args.str("terminal")?;
        let last = args.int("last", 5, 1, 50)? as usize;
        let tab = self.client_tab(client, &terminal).await?;
        let blocks = command_blocks(&tab.entries);
        // Newest first: when the reply is cut to the size limit, the command just run survives.
        let shown: Vec<Value> = blocks[blocks.len().saturating_sub(last)..]
            .iter()
            .rev()
            .cloned()
            .collect();
        let mut meta = Map::new();
        meta.insert("terminal".into(), json!(tab.id));
        meta.insert("env".into(), json!(tab.env_name));
        meta.insert("commands".into(), json!(blocks.len()));
        Ok(ToolOutput {
            data: Value::Array(shown),
            meta,
            format,
            trunc,
        })
    }

    pub(crate) async fn tool_terminal_run(
        &self,
        client: &ClientIdentity,
        arguments: Value,
    ) -> Result<ToolOutput, ToolError> {
        let mut args = Args::for_tool("terminal_run", arguments)?;
        let (format, trunc) = common(&mut args)?;
        let command = args.str("command")?;
        if command.trim().is_empty() || command.len() > MAX_COMMAND {
            return Err(ToolError::invalid(format!(
                "command must be 1 to {MAX_COMMAND} bytes"
            )));
        }
        let timeout = Duration::from_secs(args.int("timeout", 120, 1, 600)? as u64);
        let terminal = args.str_opt("terminal")?;
        let env_id = match (&terminal, args.str_opt("env")?) {
            (Some(terminal), None) => self.client_tab(client, terminal).await?.env_id,
            (None, Some(env)) => {
                // Only the environment id is needed: no protocol probe, so a down Odoo fails in the Terminal, not here.
                self.configured_target(client, &env, Capability::OdooRead)
                    .await?
                    .env
                    .id
            }
            _ => return Err(ToolError::invalid("give either terminal or env")),
        };
        let client_label = {
            let s = self.inner.lock().await;
            let v = s.vault()?;
            v.data()
                .clients
                .iter()
                .find(|g| g.id == client.client_id)
                .map(|g| g.label.clone())
                .unwrap_or_default()
        };
        let (reply, answer) = oneshot::channel();
        self.send_desktop(DesktopRequest::Run {
            terminal,
            env_id,
            command,
            client_id: client.client_id.clone(),
            client_revision: client.revision,
            client_label,
            allow_writes: client.capabilities.contains(&Capability::OdooWriteRequest),
            reply,
        })?;
        // A write waits for the user's confirmation in the Terminal, so the wait can be long.
        let data = match tokio::time::timeout(timeout, answer).await {
            Err(_) => {
                return Err(ToolError::new(
                    "timeout",
                    "the Terminal did not finish in time; the command may still be running, see terminal_read",
                    Outcome::Uncertain,
                ));
            }
            Ok(Err(_)) => {
                return Err(ToolError::new(
                    "unavailable",
                    "the Odx window closed before answering",
                    Outcome::Uncertain,
                ));
            }
            Ok(Ok(Err(message))) => {
                return Err(ToolError::new("terminal", message, Outcome::NotSent));
            }
            Ok(Ok(Ok(data))) => data,
        };
        Ok(ToolOutput {
            data,
            meta: Map::new(),
            format,
            trunc,
        })
    }

    pub(crate) async fn tool_show(
        &self,
        client: &ClientIdentity,
        arguments: Value,
    ) -> Result<ToolOutput, ToolError> {
        let mut args = Args::for_tool("show", arguments)?;
        let terminal = args.str("terminal")?;
        self.client_tab(client, &terminal).await?;
        let (reply, answer) = oneshot::channel();
        self.send_desktop(DesktopRequest::Show {
            terminal: terminal.clone(),
            reply,
        })?;
        match tokio::time::timeout(SHOW_TIMEOUT, answer).await {
            Ok(Ok(Ok(_))) => Ok(ToolOutput {
                data: json!({ "shown": { "terminal": terminal } }),
                meta: Map::new(),
                format: crate::output::Format::Compact,
                trunc: 200,
            }),
            Ok(Ok(Err(message))) => Err(ToolError::new("terminal", message, Outcome::NotSent)),
            _ => Err(ToolError::new(
                "unavailable",
                "the Odx window did not come forward",
                Outcome::NotSent,
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_saved_tabs_and_groups_commands_with_their_output() {
        let data = json!({ "version": 1, "activeId": "t2", "tabs": [
            { "id": "t1", "name": "", "defaultName": true, "envId": "A", "envName": "Prod", "session": { "entries": [
                { "id": 1, "kind": "info", "text": "Odx Terminal" },
                { "id": 2, "kind": "input", "prompt": "ben>", "text": "count res.partner", "command": true },
                { "id": 3, "kind": "text", "text": "42" },
                { "id": 4, "kind": "input", "prompt": "ben>", "text": "read res.partner 9", "command": true, "agent": "Claude" },
                { "id": 5, "kind": "error", "text": "", "appError": { "code": "upstream.remote", "detail": "Record does not exist" } },
                { "id": 6, "kind": "table", "expanded": false, "table": { "columns": ["id"], "rows": [[1]], "model": "res.partner" } },
            ] } },
            // Saved before sessions had a fixed environment: the environment is on the session.
            { "id": "t2", "name": "Ventas", "defaultName": false, "session": { "env": { "id": "B", "name": "Staging" }, "entries": [] } },
            { "id": "t3", "name": "", "defaultName": true, "session": { "entries": [] } },
        ] }).to_string();
        let (tabs, active) = saved_tabs(&data);
        assert_eq!(active.as_deref(), Some("t2"));
        assert_eq!(
            tabs.iter()
                .map(|t| (t.id.as_str(), t.name.as_str(), t.env_id.as_str()))
                .collect::<Vec<_>>(),
            [("t1", "Prod", "A"), ("t2", "Ventas", "B")]
        );
        let blocks = command_blocks(&tabs[0].entries);
        assert_eq!(blocks.len(), 2);
        assert_eq!(
            blocks[0],
            json!({ "command": "count res.partner", "output": [{ "kind": "text", "text": "42" }] })
        );
        assert_eq!(blocks[1]["agent"], "Claude");
        assert_eq!(
            blocks[1]["output"][0],
            json!({ "kind": "error", "text": "Record does not exist" })
        );
        assert_eq!(blocks[1]["output"][1]["rows"], json!([[1]]));
        assert!(saved_tabs("not json").0.is_empty());
    }
}
