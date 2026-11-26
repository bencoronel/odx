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
    /// Open a native interactive table window with arbitrary/computed columns and rows,
    /// or print it directly into `terminal` when specified.
    ShowTable {
        terminal: Option<String>,
        title: String,
        columns: Vec<String>,
        rows: Vec<Vec<Value>>,
        model: Option<String>,
        footer: Option<String>,
        client_label: String,
        reply: oneshot::Sender<Result<Value, String>>,
    },
    /// Open a validated Odoo URL in the user's default web browser.
    OpenUrl {
        url: String,
        reply: oneshot::Sender<Result<Value, String>>,
    },
    /// Write a UTF-8 CSV/JSON export to the user's Downloads folder without overwriting.
    ExportFile {
        name: String,
        content: String,
        reply: oneshot::Sender<Result<Value, String>>,
    },
    /// Notify the desktop UI that saved views changed (e.g. after `view_save`).
    ViewsChanged,
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

    pub(crate) async fn tool_show_table(
        &self,
        client: &ClientIdentity,
        arguments: Value,
    ) -> Result<ToolOutput, ToolError> {
        if !client.capabilities.contains(&Capability::OdooRead) {
            return Err(ToolError::new(
                "forbidden",
                "client lacks odoo:read capability",
                Outcome::NotSent,
            ));
        }
        let mut args = Args::for_tool("show_table", arguments)?;
        let title = args.str("title")?;
        if title.trim().is_empty() || title.chars().count() > 200 {
            return Err(ToolError::invalid("title must be 1..200 characters"));
        }
        let terminal = args.str_opt("terminal")?.filter(|t| !t.trim().is_empty());
        if let Some(ref tab_id) = terminal {
            self.client_tab(client, tab_id).await?;
        }
        let model = args.str_opt("model")?.filter(|m| !m.trim().is_empty());
        let footer = args.str_opt("footer")?.filter(|f| !f.trim().is_empty());
        let raw_columns = args.raw("columns");
        let raw_rows = args
            .raw("rows")
            .ok_or_else(|| ToolError::invalid("missing required argument: rows"))?;
        let (columns, rows) = normalize_table_input(raw_columns, raw_rows)?;
        let row_count = rows.len();
        let col_count = columns.len();
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
        self.send_desktop(DesktopRequest::ShowTable {
            terminal: terminal.clone(),
            title: title.clone(),
            columns,
            rows,
            model,
            footer,
            client_label,
            reply,
        })?;
        match tokio::time::timeout(SHOW_TIMEOUT, answer).await {
            Ok(Ok(Ok(_))) => Ok(ToolOutput {
                data: json!({ "opened": true, "terminal": terminal, "title": title, "columns": col_count, "rows": row_count }),
                meta: Map::new(),
                format: crate::output::Format::Compact,
                trunc: 200,
            }),
            Ok(Ok(Err(message))) => Err(ToolError::new("terminal", message, Outcome::NotSent)),
            _ => Err(ToolError::new(
                "unavailable",
                "the Odx window did not open the table",
                Outcome::NotSent,
            )),
        }
    }

    pub(crate) async fn tool_open_record(
        &self,
        client: &ClientIdentity,
        arguments: Value,
    ) -> Result<ToolOutput, ToolError> {
        let mut args = Args::for_tool("open_record", arguments)?;
        let env_name = args.str("env")?;
        let target = self
            .configured_target(client, &env_name, Capability::OdooRead)
            .await?;
        let model = args.str_opt("model")?;
        let id = match args.raw("id") {
            None => None,
            Some(v) => {
                let n = v
                    .as_u64()
                    .filter(|n| *n > 0)
                    .ok_or_else(|| ToolError::invalid("id must be a positive integer"))?;
                Some(n)
            }
        };
        let record = match (&model, id) {
            (Some(m), Some(i)) => {
                if !crate::tools::args::valid_model(m) {
                    return Err(ToolError::invalid(format!("invalid model: {m}")));
                }
                Some((m.as_str(), i))
            }
            (None, None) => None,
            _ => {
                return Err(ToolError::invalid(
                    "provide both model and id, or neither to open the environment home",
                ));
            }
        };
        let url = self
            .web_url(&target.env.id, record)
            .await
            .map_err(ToolError::from)?;
        let (reply, answer) = oneshot::channel();
        self.send_desktop(DesktopRequest::OpenUrl {
            url: url.clone(),
            reply,
        })?;
        match tokio::time::timeout(SHOW_TIMEOUT, answer).await {
            Ok(Ok(Ok(_))) => Ok(ToolOutput {
                data: json!({ "opened": true, "env": target.env.name, "url": url }),
                meta: Map::new(),
                format: crate::output::Format::Compact,
                trunc: 200,
            }),
            Ok(Ok(Err(message))) => Err(ToolError::new("desktop", message, Outcome::NotSent)),
            _ => Err(ToolError::new(
                "unavailable",
                "the Odx desktop app did not open the browser",
                Outcome::NotSent,
            )),
        }
    }

    pub(crate) async fn tool_export_csv(
        &self,
        client: &ClientIdentity,
        arguments: Value,
    ) -> Result<ToolOutput, ToolError> {
        if !client.capabilities.contains(&Capability::OdooRead) {
            return Err(ToolError::new(
                "forbidden",
                "client lacks odoo:read capability",
                Outcome::NotSent,
            ));
        }
        let mut args = Args::for_tool("export_csv", arguments)?;
        let raw_name = args.str("filename")?;
        let filename = sanitize_csv_filename(&raw_name)?;
        let delimiter = match args.str_opt("delimiter")?.as_deref() {
            None | Some(",") => ',',
            Some(";") => ';',
            Some(_) => return Err(ToolError::invalid("delimiter must be ',' or ';'")),
        };
        let raw_columns = args.raw("columns");
        let raw_rows = args
            .raw("rows")
            .ok_or_else(|| ToolError::invalid("missing required argument: rows"))?;
        let (columns, rows) = normalize_table_input(raw_columns, raw_rows)?;
        let csv = encode_csv(&columns, &rows, delimiter);
        let row_count = rows.len();
        let col_count = columns.len();
        let (reply, answer) = oneshot::channel();
        self.send_desktop(DesktopRequest::ExportFile {
            name: filename,
            content: csv,
            reply,
        })?;
        match tokio::time::timeout(SHOW_TIMEOUT, answer).await {
            Ok(Ok(Ok(result))) => Ok(ToolOutput {
                data: json!({
                    "saved": true,
                    "path": result.get("path").cloned().unwrap_or(Value::Null),
                    "columns": col_count,
                    "rows": row_count,
                    "delimiter": delimiter.to_string(),
                }),
                meta: Map::new(),
                format: crate::output::Format::Compact,
                trunc: 200,
            }),
            Ok(Ok(Err(message))) => Err(ToolError::new("export", message, Outcome::NotSent)),
            _ => Err(ToolError::new(
                "unavailable",
                "the Odx desktop app did not complete the export",
                Outcome::NotSent,
            )),
        }
    }

    pub(crate) async fn tool_views(
        &self,
        client: &ClientIdentity,
        arguments: Value,
    ) -> Result<ToolOutput, ToolError> {
        if !client.capabilities.contains(&Capability::OdooRead) {
            return Err(ToolError::new(
                "forbidden",
                "client lacks odoo:read capability",
                Outcome::NotSent,
            ));
        }
        let mut args = Args::for_tool("views", arguments)?;
        let (format, trunc) = common(&mut args)?;
        let views = self.views().await.map_err(ToolError::from)?;
        let data: Vec<Value> = views
            .into_iter()
            .map(|v| {
                let schedule = v.schedule.map(|s| {
                    let runs: Vec<_> = s
                        .last_runs
                        .into_iter()
                        .filter(|r| client.environment_ids.contains(&r.environment_id))
                        .collect();
                    json!({
                        "enabled": s.enabled,
                        "interval_minutes": s.interval_minutes,
                        "alert_on": s.alert_on,
                        "last_run_at": s.last_run_at,
                        "last_runs": runs,
                    })
                });
                json!({
                    "id": v.id,
                    "name": v.name,
                    "description": v.description,
                    "model": v.model,
                    "domain": v.domain,
                    "fields": v.fields,
                    "order": v.order,
                    "limit": v.limit,
                    "schedule": schedule,
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

    pub(crate) async fn tool_view_run(
        &self,
        client: &ClientIdentity,
        arguments: Value,
    ) -> Result<ToolOutput, ToolError> {
        let mut args = Args::for_tool("view_run", arguments)?;
        let (format, trunc) = common(&mut args)?;
        let env_name = args.str("env")?;
        let view_ref = args.str("view")?;
        let target = self
            .configured_target(client, &env_name, Capability::OdooRead)
            .await?;
        let views = self.views().await.map_err(ToolError::from)?;
        let found = views
            .iter()
            .find(|v| v.id == view_ref || v.name.eq_ignore_ascii_case(&view_ref))
            .ok_or_else(|| {
                ToolError::invalid(format!(
                    "no saved view {view_ref}; list available views with views"
                ))
            })?;
        let result = self
            .run_view(&found.id, &target.env.id)
            .await
            .map_err(ToolError::from)?;
        let mut meta = Map::new();
        meta.insert("view".into(), json!(found.name));
        meta.insert("environment".into(), json!(result.environment_name));
        meta.insert("total_count".into(), json!(result.total_count));
        meta.insert("duration_ms".into(), json!(result.duration_ms));
        Ok(ToolOutput {
            data: Value::Array(result.rows),
            meta,
            format,
            trunc,
        })
    }

    pub(crate) async fn tool_view_save(
        &self,
        client: &ClientIdentity,
        arguments: Value,
    ) -> Result<ToolOutput, ToolError> {
        if !client.capabilities.contains(&Capability::OdooRead) {
            return Err(ToolError::new(
                "forbidden",
                "client lacks odoo:read capability",
                Outcome::NotSent,
            ));
        }
        let mut args = Args::for_tool("view_save", arguments)?;
        let id = args.str_opt("id")?;
        let name = args.str("name")?;
        let model = crate::tools::args::model(&mut args)?;
        let description = args.text("description")?.unwrap_or_default();
        let domain = args.array("domain")?.unwrap_or_default();
        let fields = match args.array("fields")? {
            Some(items) => {
                let mut out = Vec::with_capacity(items.len());
                for item in items {
                    let s = item
                        .as_str()
                        .ok_or_else(|| ToolError::invalid("fields must be an array of strings"))?;
                    out.push(s.to_owned());
                }
                out
            }
            None => Vec::new(),
        };
        let order = args.text("order")?.filter(|s| !s.trim().is_empty());
        let limit = args.int("limit", 50, 1, 500)? as u32;
        // Preserve any existing schedule when updating an existing view by id.
        let existing_schedule = if let Some(ref view_id) = id {
            self.views()
                .await
                .ok()
                .and_then(|list| list.into_iter().find(|v| v.id == *view_id))
                .and_then(|v| v.schedule)
                .map(|s| crate::views::ViewScheduleDraft {
                    enabled: s.enabled,
                    environment_ids: s.environment_ids,
                    interval_minutes: s.interval_minutes,
                    alert_on: s.alert_on,
                })
        } else {
            None
        };
        let saved = self
            .save_view(crate::views::SavedViewDraft {
                id,
                name,
                description,
                model,
                domain: Value::Array(domain),
                fields,
                order,
                limit,
                schedule: existing_schedule,
            })
            .await
            .map_err(ToolError::from)?;
        let _ = self.send_desktop(DesktopRequest::ViewsChanged);
        Ok(ToolOutput {
            data: json!({ "saved": true, "id": saved.id, "name": saved.name, "model": saved.model }),
            meta: Map::new(),
            format: crate::output::Format::Compact,
            trunc: 200,
        })
    }
}

fn sanitize_csv_filename(raw: &str) -> Result<String, ToolError> {
    let trimmed = raw.trim();
    let stem = trimmed.strip_suffix(".csv").unwrap_or(trimmed);
    if stem.is_empty()
        || stem.len() > 100
        || stem.starts_with('.')
        || !stem
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Err(ToolError::invalid(
            "filename must be a plain name (letters, digits, '-', '_', '.') without paths",
        ));
    }
    Ok(format!("{stem}.csv"))
}

fn csv_cell_value(v: &Value) -> (String, bool) {
    match v {
        Value::Null => (String::new(), false),
        Value::Bool(b) => (b.to_string(), false),
        Value::Number(n) => (n.to_string(), true),
        Value::String(s) => (s.clone(), false),
        Value::Array(arr) if arr.len() == 2 && arr[0].is_number() && arr[1].is_string() => (
            format!("{} ({})", arr[1].as_str().unwrap_or(""), arr[0]),
            false,
        ),
        other => (other.to_string(), false),
    }
}

fn escape_csv_cell(text: &str, delimiter: char, is_number: bool) -> String {
    if is_number {
        return text.to_owned();
    }
    let safe = if text.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        format!("'{text}")
    } else {
        text.to_owned()
    };
    if safe.contains(delimiter) || safe.contains(['"', '\n', '\r']) {
        format!("\"{}\"", safe.replace('"', "\"\""))
    } else {
        safe
    }
}

pub(crate) fn encode_csv(columns: &[String], rows: &[Vec<Value>], delimiter: char) -> String {
    let mut out = String::from("\u{FEFF}");
    let sep = delimiter.to_string();
    let header: Vec<String> = columns
        .iter()
        .map(|c| escape_csv_cell(c, delimiter, false))
        .collect();
    out.push_str(&header.join(&sep));
    for row in rows {
        out.push_str("\r\n");
        let cells: Vec<String> = row
            .iter()
            .map(|v| {
                let (text, is_num) = csv_cell_value(v);
                escape_csv_cell(&text, delimiter, is_num)
            })
            .collect();
        out.push_str(&cells.join(&sep));
    }
    out
}

const MAX_TABLE_COLUMNS: usize = 60;
const MAX_TABLE_ROWS: usize = 10_000;

/// Normalize custom table input: supports either array-of-arrays (with explicit `columns`)
/// or array-of-objects (with optional `columns` — inferred in first-seen key order if omitted).
pub(crate) fn normalize_table_input(
    raw_columns: Option<Value>,
    raw_rows: Value,
) -> Result<(Vec<String>, Vec<Vec<Value>>), ToolError> {
    let raw_rows = raw_rows
        .as_array()
        .ok_or_else(|| ToolError::invalid("rows must be an array"))?;
    if raw_rows.len() > MAX_TABLE_ROWS {
        return Err(ToolError::invalid(format!(
            "rows exceeds maximum of {MAX_TABLE_ROWS}"
        )));
    }
    let mut columns: Vec<String> = match raw_columns {
        Some(Value::Array(items)) => {
            let mut cols = Vec::with_capacity(items.len());
            for item in items {
                let name = item
                    .as_str()
                    .ok_or_else(|| ToolError::invalid("columns must be an array of strings"))?
                    .trim();
                if name.is_empty() {
                    return Err(ToolError::invalid("column names cannot be empty"));
                }
                cols.push(name.to_owned());
            }
            cols
        }
        Some(Value::Null) | None => Vec::new(),
        _ => return Err(ToolError::invalid("columns must be an array of strings")),
    };
    if columns.is_empty() {
        for row in raw_rows {
            if let Some(obj) = row.as_object() {
                for key in obj.keys() {
                    if !columns.contains(key) {
                        columns.push(key.clone());
                    }
                }
            }
        }
    }
    if columns.is_empty() {
        return Err(ToolError::invalid(
            "columns is required when rows are arrays or empty",
        ));
    }
    if columns.len() > MAX_TABLE_COLUMNS {
        return Err(ToolError::invalid(format!(
            "columns exceeds maximum of {MAX_TABLE_COLUMNS}"
        )));
    }
    let mut rows = Vec::with_capacity(raw_rows.len());
    for row in raw_rows {
        match row {
            Value::Array(cells) => {
                let mut padded = Vec::with_capacity(columns.len());
                for i in 0..columns.len() {
                    padded.push(cells.get(i).cloned().unwrap_or(Value::Null));
                }
                rows.push(padded);
            }
            Value::Object(map) => {
                let mut cells = Vec::with_capacity(columns.len());
                for col in &columns {
                    cells.push(map.get(col).cloned().unwrap_or(Value::Null));
                }
                rows.push(cells);
            }
            _ => {
                return Err(ToolError::invalid(
                    "each row must be an array of cell values or an object keyed by column",
                ));
            }
        }
    }
    Ok((columns, rows))
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

    #[test]
    fn normalizes_custom_table_rows_from_arrays_and_objects() {
        let (cols, rows) = normalize_table_input(
            Some(json!(["id_prestashop", "trabajo", "detalle"])),
            json!([[872, 72070, "Duplicado"], [901, 72071]]),
        )
        .unwrap();
        assert_eq!(cols, ["id_prestashop", "trabajo", "detalle"]);
        assert_eq!(rows[0], vec![json!(872), json!(72070), json!("Duplicado")]);
        assert_eq!(rows[1], vec![json!(901), json!(72071), Value::Null]);

        let (cols2, rows2) = normalize_table_input(
            None,
            json!([
                {"id_prestashop": 872, "trabajo": 72070, "detalle": "Choca con plantilla A"},
                {"id_prestashop": 901, "trabajo": 72071}
            ]),
        )
        .unwrap();
        assert!(cols2.contains(&"id_prestashop".to_string()));
        assert_eq!(rows2.len(), 2);
        assert!(normalize_table_input(None, json!([])).is_err());
    }

    #[test]
    fn encodes_csv_with_bom_formula_protection_and_excel_semicolon() {
        assert_eq!(
            sanitize_csv_filename("errores_hoy").unwrap(),
            "errores_hoy.csv"
        );
        assert_eq!(sanitize_csv_filename("report.csv").unwrap(), "report.csv");
        assert!(sanitize_csv_filename("../escape.csv").is_err());
        let csv = encode_csv(
            &["id_prestashop".into(), "detalle".into()],
            &[vec![json!(872), json!("=SUM(A1); duplicado")]],
            ';',
        );
        assert!(csv.starts_with('\u{FEFF}'));
        assert!(csv.contains("id_prestashop;detalle\r\n872;\"'=SUM(A1); duplicado\""));
    }
}
