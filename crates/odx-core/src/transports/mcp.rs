//! Stateless MCP server, revision 2026-07-28. Validate params._meta per request. Identity comes from the locally authorized channel, never clientInfo. Core execution applies the current grant.
//! An `initialize` request instead opens a 2025-06-18 session for clients without 2026-07-28 support; both profiles share the same tools and authorization.
use crate::{
    clients::{Capability, ClientIdentity},
    config::validate_json,
    execution::Core,
    output::{self, Format, ToolError},
    tools::catalog::{TOOLS, tool_definitions},
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tokio::{
    sync::{broadcast, mpsc},
    task::AbortHandle,
};

pub const PROTOCOL_VERSION: &str = "2026-07-28";
/// Revision negotiated by `initialize`; the server answers with it whatever the client requested.
pub const LEGACY_PROTOCOL_VERSION: &str = "2025-06-18";
const INSTRUCTIONS: &str = "Odx provides named Odoo environments. Start with envs. Writes follow each environment's mode and may await human approval; deletes always require approval.";
const META_VERSION: &str = "io.modelcontextprotocol/protocolVersion";
const META_CAPABILITIES: &str = "io.modelcontextprotocol/clientCapabilities";
const META_SERVER_INFO: &str = "io.modelcontextprotocol/serverInfo";
const META_SUBSCRIPTION: &str = "io.modelcontextprotocol/subscriptionId";
/// Concurrent tools/call requests per connection.
const MAX_INFLIGHT: usize = 16;
/// Open subscriptions/listen per connection, separate from MAX_INFLIGHT.
const MAX_SUBSCRIPTIONS: usize = 8;
/// Application error outside JSON-RPC's reserved -32768..-32000 range.
pub const SUBSCRIPTION_LIMIT: i64 = -31000;
const LIST_TTL_MS: u64 = 60_000;

fn server_meta() -> Value {
    json!({META_SERVER_INFO: {"name": "odx", "version": env!("CARGO_PKG_VERSION")}})
}
fn error(id: &Value, code: i64, message: &str, data: Option<Value>) -> Value {
    let mut err = json!({"code": code, "message": message});
    if let Some(data) = data {
        err["data"] = data;
    }
    json!({"jsonrpc": "2.0", "id": id, "error": err})
}
fn result(id: &Value, mut body: Value) -> Value {
    body["resultType"] = json!("complete");
    body["_meta"] = server_meta();
    json!({"jsonrpc": "2.0", "id": id, "result": body})
}
/// 2025-06-18 results carry no 2026-07-28 fields.
fn legacy_result(id: &Value, body: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": body})
}

/// Correlation key for string or numeric JSON-RPC IDs.
fn id_key(id: &Value) -> String {
    id.to_string()
}

/// Track calls and subscriptions for cancellation; retain no client-declared version, capabilities or identity.
pub struct McpConnection {
    core: Arc<Core>,
    identity: ClientIdentity,
    out: mpsc::Sender<Value>,
    inflight: Arc<Mutex<HashMap<String, AbortHandle>>>,
    subscriptions: Subscriptions,
    /// Some(last tool list) once `initialize` opened a 2025-06-18 session.
    legacy: Legacy,
    watcher: AbortHandle,
}

/// Subscriptions by ID; Some(tool_list) tracks toolsListChanged and the last known tool list.
type Subscriptions = Arc<tokio::sync::Mutex<HashMap<String, (Value, Option<Vec<&'static str>>)>>>;
type Legacy = Arc<tokio::sync::Mutex<Option<Vec<&'static str>>>>;

impl McpConnection {
    pub fn new(core: Arc<Core>, identity: ClientIdentity, out: mpsc::Sender<Value>) -> Self {
        let subscriptions = Subscriptions::default();
        let legacy = Legacy::default();
        let watcher = tokio::spawn(watch_grant(
            core.clone(),
            identity.clone(),
            out.clone(),
            subscriptions.clone(),
            legacy.clone(),
            core.grant_changes.subscribe(),
        ))
        .abort_handle();
        Self {
            core,
            identity,
            out,
            inflight: Arc::new(Mutex::new(HashMap::new())),
            subscriptions,
            legacy,
            watcher,
        }
    }

    /// Tools from the current grant, not the connection-time grant.
    async fn current_tools(&self) -> Result<Vec<&'static str>, ToolError> {
        let (_, capabilities) = self.core.current_grant(&self.identity).await?;
        Ok(tool_names(&capabilities))
    }

    /// Handle one transport-framed client message.
    pub async fn on_message(&self, raw: &[u8]) {
        let message = match validate_json(raw) {
            Ok(v) => v,
            Err(_) => {
                let _ = self
                    .out
                    .send(error(&Value::Null, -32700, "Parse error", None))
                    .await;
                return;
            }
        };
        let Value::Object(msg) = message else {
            let _ = self
                .out
                .send(error(
                    &Value::Null,
                    -32600,
                    "Invalid Request: expected a JSON-RPC object",
                    None,
                ))
                .await;
            return;
        };
        if msg.get("jsonrpc") != Some(&json!("2.0")) {
            let id = msg.get("id").cloned().unwrap_or(Value::Null);
            let _ = self
                .out
                .send(error(
                    &id,
                    -32600,
                    "Invalid Request: jsonrpc must be 2.0",
                    None,
                ))
                .await;
            return;
        }
        let method = msg
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let Some(id) = msg.get("id").cloned() else {
            // Never respond to notifications.
            if method == "notifications/cancelled"
                && let Some(request) = msg.get("params").and_then(|p| p.get("requestId"))
            {
                let key = id_key(request);
                let handle = self.inflight.lock().expect("inflight").remove(&key);
                if let Some(handle) = handle {
                    handle.abort();
                }
                // Cancellation frees the subscription without further messages.
                self.subscriptions.lock().await.remove(&key);
            }
            return;
        };
        if !(id.is_string() || id.is_i64() || id.is_u64()) {
            let _ = self
                .out
                .send(error(
                    &Value::Null,
                    -32600,
                    "Invalid Request: id must be text or an integer",
                    None,
                ))
                .await;
            return;
        }
        if method.is_empty() {
            if msg.contains_key("result") || msg.contains_key("error") {
                return; // Ignore client responses; the server sends no requests.
            }
            let _ = self
                .out
                .send(error(&id, -32600, "Invalid Request: missing method", None))
                .await;
            return;
        }
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        match method.as_str() {
            "initialize" => return self.initialize(id).await,
            // 2025-06-18 allows ping at any time; 2026-07-28 clients never send it.
            "ping" => {
                let _ = self.out.send(legacy_result(&id, json!({}))).await;
                return;
            }
            _ => {}
        }
        let legacy = self.legacy.lock().await.is_some();
        if legacy {
            if !matches!(method.as_str(), "tools/list" | "tools/call") {
                let _ = self
                    .out
                    .send(error(&id, -32601, "Method not found", None))
                    .await;
                return;
            }
        } else if let Err(reply) = validate_meta(&id, &params) {
            let _ = self.out.send(reply).await;
            return;
        }
        let key = id_key(&id);
        let in_use = self.inflight.lock().expect("inflight").contains_key(&key);
        if in_use || self.subscriptions.lock().await.contains_key(&key) {
            let _ = self
                .out
                .send(error(
                    &id,
                    -32600,
                    "Invalid Request: id already in flight",
                    None,
                ))
                .await;
            return;
        }
        match method.as_str() {
            "server/discover" => {
                let body = json!({
                    "supportedVersions": [PROTOCOL_VERSION],
                    "capabilities": {"tools": {"listChanged": true}},
                    "instructions": INSTRUCTIONS,
                    "ttlMs": LIST_TTL_MS,
                    "cacheScope": "public",
                });
                let _ = self.out.send(result(&id, body)).await;
            }
            "tools/list" => {
                if params.get("cursor").is_some() {
                    let _ = self
                        .out
                        .send(error(&id, -32602, "Invalid params: invalid cursor", None))
                        .await;
                    return;
                }
                let names = match self.current_tools().await {
                    Ok(names) => names,
                    Err(err) => {
                        let _ = self
                            .out
                            .send(error(&id, -32603, &err.compact(), None))
                            .await;
                        return;
                    }
                };
                let tools: Vec<Value> = tool_definitions()
                    .into_iter()
                    .filter(|d| names.contains(&d.name))
                    .map(|d| {
                        json!({
                            "name": d.name,
                            "title": d.title,
                            "description": d.description,
                            "inputSchema": d.input_schema,
                            "annotations": {
                                "title": d.title,
                                "readOnlyHint": d.read_only,
                                "destructiveHint": d.destructive,
                                "idempotentHint": d.read_only,
                                "openWorldHint": false,
                            },
                        })
                    })
                    .collect();
                let reply = if legacy {
                    legacy_result(&id, json!({"tools": tools}))
                } else {
                    // Grant-dependent tool lists use short private caching.
                    result(
                        &id,
                        json!({"tools": tools, "ttlMs": LIST_TTL_MS, "cacheScope": "private"}),
                    )
                };
                let _ = self.out.send(reply).await;
            }
            "tools/call" => self.spawn_call(id, key, params, legacy),
            "subscriptions/listen" => {
                // Accept only toolsListChanged; keep the request open until cancellation or disconnect.
                let mut subscriptions = self.subscriptions.lock().await;
                if subscriptions.len() >= MAX_SUBSCRIPTIONS {
                    let _ = self
                        .out
                        .send(error(
                            &id,
                            SUBSCRIPTION_LIMIT,
                            "too many open subscriptions on this connection",
                            Some(json!({"max": MAX_SUBSCRIPTIONS})),
                        ))
                        .await;
                    return;
                }
                let tools = params["notifications"]["toolsListChanged"] == json!(true);
                // Capture the initial list under the registry lock so concurrent saved changes are detected.
                let seen = if tools {
                    Some(self.current_tools().await.unwrap_or_default())
                } else {
                    None
                };
                let filter = if tools {
                    json!({"toolsListChanged": true})
                } else {
                    json!({})
                };
                let ack = json!({
                    "jsonrpc": "2.0",
                    "method": "notifications/subscriptions/acknowledged",
                    "params": {"_meta": {META_SUBSCRIPTION: id}, "notifications": filter},
                });
                let _ = self.out.send(ack).await;
                subscriptions.insert(key, (id, seen));
            }
            _ => {
                let _ = self
                    .out
                    .send(error(&id, -32601, "Method not found", None))
                    .await;
            }
        }
    }

    /// Open a 2025-06-18 session; its later requests carry no `_meta`.
    async fn initialize(&self, id: Value) {
        let mut legacy = self.legacy.lock().await;
        let reply = if legacy.is_some() {
            error(&id, -32600, "Invalid Request: already initialized", None)
        } else {
            *legacy = Some(self.current_tools().await.unwrap_or_default());
            legacy_result(
                &id,
                json!({
                    "protocolVersion": LEGACY_PROTOCOL_VERSION,
                    "capabilities": {"tools": {"listChanged": true}},
                    "serverInfo": {"name": "odx", "version": env!("CARGO_PKG_VERSION")},
                    "instructions": INSTRUCTIONS,
                }),
            )
        };
        let _ = self.out.send(reply).await;
    }

    fn spawn_call(&self, id: Value, key: String, params: Value, legacy: bool) {
        let wrap = if legacy { legacy_result } else { result };
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let out = self.out.clone();
        // Check identity existence; the core enforces the current grant.
        if !TOOLS.contains(&name.as_str()) {
            tokio::spawn(async move {
                let _ = out
                    .send(error(
                        &id,
                        -32602,
                        // Bounded echo: the reply must stay far below the frame limit.
                        &format!(
                            "Unknown tool: {}",
                            name.chars().take(64).collect::<String>()
                        ),
                        None,
                    ))
                    .await;
            });
            return;
        }
        let arguments = match params.get("arguments") {
            None => json!({}),
            Some(v @ Value::Object(_)) => v.clone(),
            Some(_) => {
                tokio::spawn(async move {
                    let _ = out
                        .send(error(
                            &id,
                            -32602,
                            "Invalid params: arguments must be an object",
                            None,
                        ))
                        .await;
                });
                return;
            }
        };
        let mut inflight = self.inflight.lock().expect("inflight");
        if inflight.len() >= MAX_INFLIGHT {
            let reply = call_error(
                wrap,
                &id,
                &arguments,
                &ToolError::new(
                    "capacity_exceeded",
                    "too many concurrent calls on this connection",
                    output::Outcome::NotSent,
                ),
            );
            tokio::spawn(async move {
                let _ = out.send(reply).await;
            });
            return;
        }
        let core = self.core.clone();
        let identity = self.identity.clone();
        let registry = self.inflight.clone();
        let task_key = key.clone();
        let task = tokio::spawn(async move {
            let reply = match core
                .run_tool(&identity, &name, arguments.clone())
                .await
                .and_then(output::fit)
            {
                Ok((out, rendered)) => {
                    let mut body = json!({
                        "content": [{"type": "text", "text": rendered.text}],
                        "isError": false,
                    });
                    if out.format == Format::Json {
                        body["structuredContent"] = rendered.structured.unwrap_or(Value::Null);
                    }
                    wrap(&id, body)
                }
                Err(err) => call_error(wrap, &id, &arguments, &err),
            };
            // Cancelled requests are unregistered and receive no response.
            let still_registered = registry
                .lock()
                .expect("inflight")
                .remove(&task_key)
                .is_some();
            if still_registered {
                let _ = out.send(reply).await;
            }
        });
        inflight.insert(key, task.abort_handle());
    }
}

impl Drop for McpConnection {
    /// Abandon all connection requests on EOF or disconnect.
    fn drop(&mut self) {
        self.watcher.abort();
        for (_, handle) in self.inflight.lock().expect("inflight").drain() {
            handle.abort();
        }
    }
}

fn call_error(
    wrap: fn(&Value, Value) -> Value,
    id: &Value,
    arguments: &Value,
    err: &ToolError,
) -> Value {
    let mut body = json!({
        "content": [{"type": "text", "text": err.compact()}],
        "isError": true,
    });
    if arguments.get("format") == Some(&json!("json")) {
        body["structuredContent"] = err.envelope("mcp");
    }
    wrap(id, body)
}

/// Grant-filtered tools; call supports reads and writes.
fn tool_names(capabilities: &[Capability]) -> Vec<&'static str> {
    let read = capabilities.contains(&Capability::OdooRead);
    let write = capabilities.contains(&Capability::OdooWriteRequest);
    TOOLS
        .into_iter()
        .filter(|name| match *name {
            "envs" => true,
            "call" => read || write,
            "create" | "write" | "unlink" => write,
            _ => read,
        })
        .collect()
}

/// Notify toolsListChanged subscribers when saved capabilities change their tool list.
async fn watch_grant(
    core: Arc<Core>,
    identity: ClientIdentity,
    out: mpsc::Sender<Value>,
    subscriptions: Subscriptions,
    legacy: Legacy,
    mut changes: broadcast::Receiver<String>,
) {
    loop {
        match changes.recv().await {
            Ok(client_id) if client_id != identity.client_id => continue,
            Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
            Err(broadcast::error::RecvError::Closed) => return,
        }
        let Ok((_, capabilities)) = core.current_grant(&identity).await else {
            continue;
        };
        let names = tool_names(&capabilities);
        if let Some(seen) = legacy.lock().await.as_mut()
            && *seen != names
        {
            seen.clone_from(&names);
            let notice = json!({"jsonrpc": "2.0", "method": "notifications/tools/list_changed"});
            let _ = out.send(notice).await;
        }
        // Hold the registry lock while sending; later cancellations receive no notices.
        let mut subscriptions = subscriptions.lock().await;
        for (id, seen) in subscriptions.values_mut() {
            if let Some(seen) = seen
                && *seen != names
            {
                seen.clone_from(&names);
                let notice = json!({
                    "jsonrpc": "2.0",
                    "method": "notifications/tools/list_changed",
                    "params": {"_meta": {META_SUBSCRIPTION: id}},
                });
                let _ = out.send(notice).await;
            }
        }
    }
}

/// Require params._meta on every request, including server/discover.
fn validate_meta(id: &Value, params: &Value) -> Result<(), Value> {
    let invalid = |msg: &str| error(id, -32602, &format!("Invalid params: {msg}"), None);
    let Some(params) = params.as_object() else {
        return Err(invalid("missing params._meta"));
    };
    let Some(meta) = params.get("_meta").and_then(Value::as_object) else {
        return Err(invalid("missing params._meta"));
    };
    let Some(version) = meta.get(META_VERSION).and_then(Value::as_str) else {
        return Err(invalid(&format!("missing _meta[\"{META_VERSION}\"]")));
    };
    if !meta.get(META_CAPABILITIES).is_some_and(Value::is_object) {
        return Err(invalid(&format!("missing _meta[\"{META_CAPABILITIES}\"]")));
    }
    if version != PROTOCOL_VERSION {
        return Err(error(
            id,
            -32022,
            "Unsupported protocol version",
            Some(json!({"supported": [PROTOCOL_VERSION], "requested": version})),
        ));
    }
    Ok(())
}
