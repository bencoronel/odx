//! API/MCP tools and validation. All use shared grants, policy, limits and audit.
use crate::{
    approval::Approval,
    audit::AuditDetails,
    clients::{Capability, ClientIdentity},
    config::Environment,
    error::OdxError,
    execution::{ApprovalEnd, Core, Upstream},
    output::{Format, Outcome, ToolError, ToolOutput},
    policy::{self, Effect},
    tools::{files::OutputKind, mutations, query::effective_context},
};
use chrono::Utc;
use serde_json::{Map, Value, json};
use std::{
    collections::HashSet,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::oneshot;

const SCHEMAS: &str = include_str!("tools.schema.json");

/// All 21 tools in alphabetical order.
pub const TOOLS: [&str; 21] = [
    "access",
    "call",
    "compare",
    "create",
    "download",
    "envs",
    "fields",
    "history",
    "models",
    "name_search",
    "read",
    "read_group",
    "ref",
    "search",
    "search_count",
    "search_read",
    "unlink",
    "view",
    "wait",
    "whoami",
    "write",
];

/// Methods call treats as reads; all others are potential writes.
pub(crate) const READ_METHODS: [&str; 10] = [
    "check_access_rights",
    "fields_get",
    "get_external_id",
    "get_views",
    "name_search",
    "read",
    "read_group",
    "search",
    "search_count",
    "search_read",
];

#[derive(Debug, Clone)]
pub struct ToolDefinition {
    pub name: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
    pub read_only: bool,
    pub destructive: bool,
}

fn describe(name: &str) -> (&'static str, &'static str, bool, bool) {
    match name {
        "envs" => (
            "Environments",
            "Odoo environments granted to this client, with their write modes and tags. Makes no Odoo calls.",
            true,
            false,
        ),
        "whoami" => (
            "Odoo user",
            "Odoo user used by the proxy: uid, login, name and companies.",
            true,
            false,
        ),
        "search_read" => (
            "Search and read",
            "Read records and fields in one call. Defaults to id and display_name. Paginates with limit/offset. Use search for IDs only and search_count for counts.",
            true,
            false,
        ),
        "read" => (
            "Read by IDs",
            "Read up to 1000 records by ID; flag missing IDs.",
            true,
            false,
        ),
        "search" => (
            "Search IDs",
            "Find IDs matching the domain. Paginates with limit/offset.",
            true,
            false,
        ),
        "search_count" => ("Count", "Count records matching the domain.", true, false),
        "fields" => (
            "Fields",
            "Inspect model field metadata before choosing fields, relations or selections. names limits upstream fields; q filters names/labels afterward.",
            true,
            false,
        ),
        "models" => (
            "Models",
            "Find models whose technical name or label contains q.",
            true,
            false,
        ),
        "name_search" => (
            "Search by name",
            "Search names as in Odoo dropdowns. No offset; refine the query when more matches exist.",
            true,
            false,
        ),
        "create" => (
            "Create",
            "Create up to 1000 records. The environment mode determines execution or human approval.",
            false,
            false,
        ),
        "write" => (
            "Update",
            "Update records by ID under the environment write mode. Relational delete/unlink commands require individual human approval.",
            false,
            true,
        ),
        "unlink" => (
            "Delete",
            "Delete records by ID. Always requires human approval; one approval covers all listed IDs. Cannot be undone.",
            false,
            true,
        ),
        "call" => (
            "Call a method",
            "Call a public business method; prefer dedicated tools when available. Known reads retain their validation and limits. JSON-2 requires empty args and named kwargs, including kwargs.ids for records. Other methods follow write mode; unlink always requires human approval. Preserves the JSON result; to_file saves it locally.",
            false,
            true,
        ),
        "wait" => (
            "Wait for a condition",
            "Poll search_count for matching records, or none with absent=true. Timeout defaults to 60 s, maximum 600 s; minimum interval 3 s. Returns matched=false on timeout.",
            true,
            false,
        ),
        "read_group" => (
            "Group",
            "Aggregate in Odoo without downloading all records. groupby supports partner_id or date:month; groupby=[] requires explicit aggregates such as amount_total:sum. Uses lazy=false and paginates groups, not records.",
            true,
            false,
        ),
        "ref" => (
            "Resolve XML ID",
            "Resolve an XML ID (module.name) to {model,id}. Missing IDs return not_found; never falls back to name search.",
            true,
            false,
        ),
        "history" => (
            "History",
            "Read chatter newest first with tracked changes (field, old, new). Paginates messages, not changes. Unreadable tracking returns changes=null with warnings; this does not imply no changes occurred.",
            true,
            false,
        ),
        "view" => (
            "View",
            "Read the connected user's final view architecture and paginated inheritance chain. q filters lines and marks matching views. Readable views may differ from applied views due to groups or inactive views.",
            true,
            false,
        ),
        "access" => (
            "Permissions",
            "Inspect the Odoo user's CRUD rights, ACLs and record rules. Each ACL and rule has applies: whether it matches the user's groups (null when unknown; a hint, not a guarantee). Partial data is flagged. Excludes proxy restrictions and does not guarantee permission for a specific operation.",
            true,
            false,
        ),
        "compare" => (
            "Compare environments",
            "Compare model records across env_a/env_b by XML ID or an explicit key field. Reads and pages are independent, not transactional. Zero differences on a partial page do not imply equality.",
            true,
            false,
        ),
        "download" => (
            "Download binary",
            "Save a binary field (default ir.attachment.datas) to a server-chosen local file. Returns path and size, never base64.",
            true,
            false,
        ),
        _ => ("", "", false, false),
    }
}

/// Published definitions with schemas from the contract.
pub fn tool_definitions() -> Vec<ToolDefinition> {
    let schemas: Value = serde_json::from_str(SCHEMAS).expect("valid tools.schema.json");
    TOOLS
        .iter()
        .map(|name| {
            let schema = schemas["$defs"][*name].clone();
            let (title, description, read_only, destructive) = describe(name);
            ToolDefinition {
                name,
                title,
                description,
                input_schema: schema,
                read_only,
                destructive,
            }
        })
        .collect()
}

fn from_error(error: OdxError) -> ToolError {
    match error {
        OdxError::Locked => ToolError::new("locked", "Odx vault is locked", Outcome::NotSent),
        OdxError::Unauthorized | OdxError::ClientNotFound => ToolError::new(
            "unauthenticated",
            "invalid, replaced or revoked client token",
            Outcome::NotSent,
        ),
        OdxError::Invalid(_) => ToolError::invalid("invalid input"),
        error @ OdxError::Validation { .. } => {
            let code = error.code();
            let params = error.ui_params();
            ToolError::invalid(error.to_string()).with_ui_error(code, params)
        }
        OdxError::EnvironmentNotFound => ToolError::new(
            "environment_unavailable",
            "environment unavailable to this client",
            Outcome::NotSent,
        ),
        OdxError::Denied => ToolError::new(
            "forbidden",
            "denied by environment policy",
            Outcome::NotSent,
        ),
        OdxError::ApprovalUnavailable => ToolError::new(
            "approval_revoked",
            "request no longer valid: revoked, expired or environment changed",
            Outcome::NotSent,
        ),
        OdxError::Uncertain => ToolError::new(
            "uncertain_result",
            "Odoo may have applied the write; no retry was attempted. Check its state before repeating",
            Outcome::Uncertain,
        ),
        OdxError::Persistence | OdxError::Io(_) | OdxError::Json(_) => ToolError::new(
            "audit_unavailable",
            "could not record audit; nothing was sent to Odoo",
            Outcome::NotSent,
        ),
        OdxError::Capacity => ToolError::new(
            "capacity_exceeded",
            "capacity limit reached; retry later",
            Outcome::NotSent,
        ),
        OdxError::Remote(m) => ToolError::new("upstream_error", m, Outcome::Failed),
        OdxError::AuthenticationRejected => ToolError::new(
            "upstream_auth",
            "Odoo rejected the environment credential",
            Outcome::Failed,
        ),
        OdxError::Network {
            kind: "timeout",
            message,
        } => ToolError::new("upstream_timeout", message, Outcome::Failed),
        OdxError::Network {
            kind: "proxy",
            message,
        } => ToolError::new(
            "upstream_network",
            format!("outbound proxy failure: {message}"),
            Outcome::Failed,
        ),
        OdxError::Network { message, .. } => {
            ToolError::new("upstream_network", message, Outcome::Failed)
        }
        error @ OdxError::HttpStatus { .. } => {
            let (code, params) = (error.code(), error.ui_params());
            ToolError::new("upstream_error", error.to_string(), Outcome::Failed)
                .with_ui_error(code, params)
        }
        OdxError::Upstream(m) => ToolError::new("upstream_network", m, Outcome::Failed),
        e @ OdxError::ResponseTooLarge { .. } => {
            ToolError::new("output_too_large", e.to_string(), Outcome::Failed)
        }
        OdxError::UnlockFailed => ToolError::new(
            "service_unavailable",
            "service unavailable",
            Outcome::NotSent,
        ),
    }
}
impl From<OdxError> for ToolError {
    fn from(error: OdxError) -> Self {
        from_error(error)
    }
}

// Arguments

pub(crate) struct Args {
    pub(crate) map: Map<String, Value>,
}
impl Args {
    pub(crate) fn new(value: Value, allowed: &[&'static str]) -> Result<Self, ToolError> {
        let Value::Object(map) = value else {
            return Err(ToolError::invalid("arguments must be a JSON object"));
        };
        for (key, v) in &map {
            if !allowed.contains(&key.as_str()) {
                return Err(ToolError::invalid(format!("unknown field: {key}")));
            }
            if v.is_null() {
                return Err(ToolError::invalid(format!("{key} cannot be null; omit it")));
            }
        }
        Ok(Self { map })
    }
    pub(crate) fn has(&self, key: &str) -> bool {
        self.map.contains_key(key)
    }
    pub(crate) fn str_opt(&mut self, key: &'static str) -> Result<Option<String>, ToolError> {
        match self.map.get(key) {
            None => Ok(None),
            Some(Value::String(s)) if !s.is_empty() => Ok(Some(s.clone())),
            Some(_) => Err(ToolError::invalid(format!("{key} must be nonempty text"))),
        }
    }
    pub(crate) fn str(&mut self, key: &'static str) -> Result<String, ToolError> {
        self.str_opt(key)?
            .ok_or_else(|| ToolError::invalid(format!("missing {key}")))
    }
    pub(crate) fn int(
        &mut self,
        key: &'static str,
        default: i64,
        min: i64,
        max: i64,
    ) -> Result<i64, ToolError> {
        match self.map.get(key) {
            None => Ok(default),
            Some(v) => v
                .as_i64()
                .filter(|n| (min..=max).contains(n))
                .ok_or_else(|| {
                    ToolError::invalid(format!("{key} must be an integer between {min} and {max}"))
                }),
        }
    }
    pub(crate) fn object(&mut self, key: &'static str) -> Result<Map<String, Value>, ToolError> {
        match self.map.get(key) {
            None => Ok(Map::new()),
            Some(Value::Object(m)) => Ok(m.clone()),
            Some(_) => Err(ToolError::invalid(format!("{key} must be a JSON object"))),
        }
    }
    pub(crate) fn array(&mut self, key: &'static str) -> Result<Option<Vec<Value>>, ToolError> {
        match self.map.get(key) {
            None => Ok(None),
            Some(Value::Array(a)) => Ok(Some(a.clone())),
            Some(_) => Err(ToolError::invalid(format!("{key} must be a list"))),
        }
    }
    pub(crate) fn bool(&mut self, key: &'static str) -> Result<bool, ToolError> {
        match self.map.get(key) {
            None => Ok(false),
            Some(Value::Bool(b)) => Ok(*b),
            Some(_) => Err(ToolError::invalid(format!("{key} must be boolean"))),
        }
    }
    pub(crate) fn raw(&mut self, key: &'static str) -> Option<Value> {
        self.map.get(key).cloned()
    }
    /// Text allowing empty values for fields/models q filters.
    pub(crate) fn text(&mut self, key: &'static str) -> Result<Option<String>, ToolError> {
        match self.map.get(key) {
            None => Ok(None),
            Some(Value::String(s)) => Ok(Some(s.clone())),
            Some(_) => Err(ToolError::invalid(format!("{key} must be text"))),
        }
    }
}

pub(crate) fn is_identifier(s: &str, dotted: bool) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase() || c == '_' || (!dotted && c.is_ascii_uppercase()))
        && s.len() <= 128
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || (dotted && c == '.'))
}
pub(crate) fn model(args: &mut Args) -> Result<String, ToolError> {
    let m = args.str("model")?;
    if !is_identifier(&m, true) || m.contains("..") || m.ends_with('.') {
        return Err(ToolError::invalid(format!("invalid model: {m}")));
    }
    Ok(m)
}
pub(crate) fn fields(args: &mut Args, default: &[&str]) -> Result<Vec<String>, ToolError> {
    field_list(args, "fields", default)
}
pub(crate) fn field_list(
    args: &mut Args,
    key: &'static str,
    default: &[&str],
) -> Result<Vec<String>, ToolError> {
    let Some(list) = args.array(key)? else {
        return Ok(default.iter().map(|s| s.to_string()).collect());
    };
    if list.is_empty() {
        return Err(ToolError::invalid(format!("{key} cannot be empty")));
    }
    let mut out: Vec<String> = Vec::new();
    for f in list {
        let Some(name) = f.as_str().filter(|n| is_identifier(n, false)) else {
            return Err(ToolError::invalid(format!("invalid field: {f}")));
        };
        if out.iter().any(|x| x == name) {
            return Err(ToolError::invalid(format!("duplicate field: {name}")));
        }
        out.push(name.into());
    }
    Ok(out)
}
pub(crate) fn with_id(mut fields: Vec<String>) -> Vec<String> {
    if !fields.iter().any(|f| f == "id") {
        fields.insert(0, "id".into());
    }
    fields
}
pub(crate) fn domain(args: &mut Args) -> Result<Value, ToolError> {
    let Some(items) = args.array("domain")? else {
        return Ok(json!([]));
    };
    for item in &items {
        let ok = match item {
            Value::String(op) => matches!(op.as_str(), "&" | "|" | "!"),
            Value::Array(t) => {
                t.len() == 3 && t[0].as_str().is_some_and(|f| !f.is_empty()) && t[1].is_string()
            }
            _ => false,
        };
        if !ok {
            return Err(ToolError::invalid(format!(
                "invalid domain item: {item}; use [field, operator, value] or '&', '|', '!'"
            )));
        }
    }
    Ok(Value::Array(items))
}
/// Append id as a stable tie-breaker if missing.
pub(crate) fn order(args: &mut Args, key: &'static str) -> Result<String, ToolError> {
    let Some(raw) = args.str_opt(key)? else {
        return Ok("id asc".into());
    };
    let mut has_id = false;
    for part in raw.split(',') {
        let tokens: Vec<&str> = part.split_whitespace().collect();
        let valid = match tokens.as_slice() {
            [f] => is_identifier(f, false),
            [f, dir] => {
                is_identifier(f, false)
                    && matches!(dir.to_ascii_lowercase().as_str(), "asc" | "desc")
            }
            _ => false,
        };
        if !valid {
            return Err(ToolError::invalid(format!("invalid order: {raw}")));
        }
        has_id |= tokens[0] == "id";
    }
    Ok(if has_id {
        raw
    } else {
        format!("{raw}, id asc")
    })
}
pub(crate) fn ids(args: &mut Args) -> Result<Vec<i64>, ToolError> {
    let list = args
        .array("ids")?
        .ok_or_else(|| ToolError::invalid("missing ids"))?;
    let ids: Vec<i64> = list
        .iter()
        .map(|v| v.as_i64().filter(|n| *n > 0 && *n <= 9_007_199_254_740_991))
        .collect::<Option<_>>()
        .ok_or_else(|| ToolError::invalid("ids must be positive integers"))?;
    mutations::validate_ids(&ids)?;
    Ok(ids)
}
/// Remove the extra fetched row and populate meta.page.
pub(crate) fn paged(
    mut data: Value,
    limit: usize,
    offset: usize,
    meta: &mut Map<String, Value>,
) -> Value {
    if let Value::Array(rows) = &mut data {
        meta.insert("page".into(), crate::output::page_of(rows, limit, offset));
    }
    data
}
pub(crate) fn common(args: &mut Args) -> Result<(Format, usize), ToolError> {
    let format = match args.map.get("format").and_then(Value::as_str) {
        None if !args.has("format") => Format::Compact,
        Some("compact") => Format::Compact,
        Some("json") => Format::Json,
        _ => return Err(ToolError::invalid("format must be compact or json")),
    };
    let trunc = args.int("trunc", 200, 0, i64::MAX)? as usize;
    Ok((format, trunc))
}

const BASE: [&str; 4] = ["env", "context", "format", "trunc"];
pub(crate) fn allowed(extra: &[&'static str]) -> Vec<&'static str> {
    BASE.iter().chain(extra).copied().collect()
}

// Execution

/// Environment resolved within the client's current grant.
pub(crate) struct Target {
    pub(crate) env: Environment,
    pub(crate) global: Value,
    pub(crate) session: u64,
}

impl Core {
    /// Run as the specified client, revalidating its grant on every call.
    pub async fn run_tool(
        self: &Arc<Self>,
        client: &ClientIdentity,
        name: &str,
        arguments: Value,
    ) -> Result<ToolOutput, ToolError> {
        let (environment_ids, capabilities) = self.current_grant(client).await?;
        self.mark_used(&client.client_id);
        let client = ClientIdentity {
            environment_ids,
            capabilities,
            ..client.clone()
        };
        // Reject credential-returning calls before transport discovery probes.
        if TOOLS.contains(&name)
            && let Some(model) = arguments.get("model").and_then(Value::as_str)
        {
            let method = if name == "call" {
                arguments
                    .get("method")
                    .and_then(Value::as_str)
                    .unwrap_or("")
            } else {
                name
            };
            if policy::is_credential_call(model, method) {
                let capability = if ["create", "write", "unlink"].contains(&name)
                    || (name == "call" && !READ_METHODS.contains(&method))
                {
                    Capability::OdooWriteRequest
                } else {
                    Capability::OdooRead
                };
                for key in ["env", "env_a", "env_b"] {
                    if let Some(environment) = arguments.get(key).and_then(Value::as_str) {
                        let target = self
                            .configured_target(&client, environment, capability)
                            .await?;
                        self.reject_credential_call(
                            Some(&client.client_id),
                            &target.env,
                            model,
                            method,
                            arguments.get("args").unwrap_or(&json!([])),
                            arguments.get("kwargs").unwrap_or(&json!({})),
                        )
                        .await?;
                    }
                }
            }
        }
        match name {
            "envs" => self.tool_envs(&client, arguments).await,
            "whoami" | "search_read" | "read" | "search" | "search_count" | "fields" | "models"
            | "name_search" => self.tool_read(&client, name, arguments).await,
            "create" | "write" | "unlink" | "call" => {
                self.tool_mutation(&client, name, arguments).await
            }
            "wait" => self.tool_wait(&client, arguments).await,
            "read_group" => self.tool_read_group(&client, arguments).await,
            "ref" => self.tool_ref(&client, arguments).await,
            "history" => self.tool_history(&client, arguments).await,
            "view" => self.tool_view(&client, arguments).await,
            "access" => self.tool_access(&client, arguments).await,
            "compare" => self.tool_compare(&client, arguments).await,
            "download" => self.tool_download(&client, arguments).await,
            _ => Err(ToolError::invalid(format!("unknown tool: {name}"))),
        }
    }

    /// Current grant; fail if the token was replaced or revoked after authentication.
    pub(crate) async fn current_grant(
        &self,
        client: &ClientIdentity,
    ) -> Result<(Vec<String>, Vec<Capability>), ToolError> {
        let s = self.inner.lock().await;
        let v = s.vault.as_ref().ok_or(OdxError::Locked)?;
        let grant = v
            .data()
            .clients
            .iter()
            .find(|g| g.id == client.client_id && !g.revoked && g.revision == client.revision)
            .ok_or(OdxError::Unauthorized)?;
        // Resolve all-environment scope per call to include future environments.
        let environment_ids = if grant.all_environments {
            v.data().environments.iter().map(|e| e.id.clone()).collect()
        } else {
            grant.environment_ids.clone()
        };
        Ok((environment_ids, grant.capabilities.clone()))
    }

    pub(crate) async fn target(
        &self,
        client: &ClientIdentity,
        name: &str,
        capability: Capability,
    ) -> Result<Target, ToolError> {
        let target = self.configured_target(client, name, capability).await?;
        Ok(Target {
            env: self.resolve_protocol(target.env, target.session).await?,
            ..target
        })
    }

    async fn configured_target(
        &self,
        client: &ClientIdentity,
        name: &str,
        capability: Capability,
    ) -> Result<Target, ToolError> {
        let s = self.inner.lock().await;
        let v = s.vault.as_ref().ok_or(OdxError::Locked)?;
        let env = v
            .data()
            .environments
            .iter()
            .find(|e| e.name == name && client.environment_ids.contains(&e.id))
            .ok_or(OdxError::EnvironmentNotFound)?;
        let env = v.data().connection_environment(&env.id)?;
        if !client.capabilities.contains(&capability) {
            return Err(ToolError::new(
                "forbidden",
                match capability {
                    Capability::OdooRead => "client lacks odoo:read capability",
                    Capability::OdooWriteRequest => "client lacks odoo:write-request capability",
                },
                Outcome::NotSent,
            ));
        }
        let target = Target {
            global: v.data().global_context.clone(),
            env,
            session: s.session,
        };
        drop(s);
        Ok(target)
    }

    async fn tool_envs(
        &self,
        client: &ClientIdentity,
        arguments: Value,
    ) -> Result<ToolOutput, ToolError> {
        let mut args = Args::new(arguments, &["format", "trunc"])?;
        let (format, trunc) = common(&mut args)?;
        let s = self.inner.lock().await;
        let v = s.vault.as_ref().ok_or(OdxError::Locked)?;
        let now = v.now();
        let mut envs: Vec<&Environment> = v
            .data()
            .environments
            .iter()
            .filter(|e| client.environment_ids.contains(&e.id))
            .collect();
        envs.sort_by(|a, b| a.name.cmp(&b.name));
        let data = envs
            .iter()
            .map(|e| {
                json!({
                    "name": e.name,
                    "protocol": if e.protocol == crate::config::Protocol::Auto { e.detected_protocol.unwrap_or(e.protocol) } else { e.protocol },
                    "write_mode": e.effective_write_mode(now),
                    "context": e.context,
                    "tags": e.tags,
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

    /// Deny and audit API-key generation/exposure in every mode. No approval request or Odoo call is made.
    pub(crate) async fn reject_credential_call(
        &self,
        client: Option<&str>,
        env: &Environment,
        model: &str,
        method: &str,
        args: &Value,
        kwargs: &Value,
    ) -> Result<(), ToolError> {
        if !policy::is_credential_call(model, method) {
            return Ok(());
        }
        self.log_as(
            client,
            &env.id,
            &format!("{method}:{model}"),
            "denied",
            AuditDetails::call(env, model, method, args, kwargs),
        )
        .await?;
        Err(ToolError::new(
            "forbidden",
            format!(
                "{model}.{method} generates or exposes Odoo API keys; Odx never returns credentials to clients and denies this in every mode, without approval"
            ),
            Outcome::NotSent,
        ))
    }

    /// Read with permissions, audit and pre-send revision checks.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn read_call(
        &self,
        client: &ClientIdentity,
        target: &Target,
        model: &str,
        method: &str,
        args: Value,
        kwargs: Map<String, Value>,
        call_context: &Value,
        upstream: &mut Upstream,
    ) -> Result<Value, ToolError> {
        self.read_call_with(
            client,
            target,
            model,
            method,
            |_| args.clone(),
            kwargs,
            call_context,
            upstream,
        )
        .await
    }

    /// Like read_call, building arguments from session uid; record None before login.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn read_call_with(
        &self,
        client: &ClientIdentity,
        target: &Target,
        model: &str,
        method: &str,
        args: impl Fn(Option<i64>) -> Value,
        kwargs: Map<String, Value>,
        call_context: &Value,
        upstream: &mut Upstream,
    ) -> Result<Value, ToolError> {
        self.read_call_limited(
            client,
            target,
            model,
            method,
            args,
            kwargs,
            call_context,
            upstream,
            crate::odoo::jsonrpc::MAX_RESPONSE,
        )
        .await
    }

    /// Like read_call_with, with an upstream byte limit. Reads use 16 MiB; only download differs.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn read_call_limited(
        &self,
        client: &ClientIdentity,
        target: &Target,
        model: &str,
        method: &str,
        args: impl Fn(Option<i64>) -> Value,
        mut kwargs: Map<String, Value>,
        call_context: &Value,
        upstream: &mut Upstream,
        max_response: usize,
    ) -> Result<Value, ToolError> {
        let env = &target.env;
        kwargs.insert(
            "context".into(),
            effective_context(&target.global, &env.context, call_context),
        );
        self.reject_credential_call(
            Some(&client.client_id),
            env,
            model,
            method,
            &args(None),
            &Value::Object(kwargs.clone()),
        )
        .await?;
        let permits = self.limits.acquire(&env.id).await?;
        let operation = format!("{method}:{model}");
        let mut audit = self
            .log_as(
                Some(&client.client_id),
                &env.id,
                &operation,
                "attempted",
                AuditDetails::call(
                    env,
                    model,
                    method,
                    &args(None),
                    &Value::Object(kwargs.clone()),
                ),
            )
            .await?;
        // A change while waiting for a slot closes the recorded attempt as not sent.
        if let Err(e) = self
            .assert_environment_current(&env.id, env.revision, target.session)
            .await
        {
            audit.finish_odx_error(env, &e);
            self.log_outcome_as(
                audit,
                Some(&client.client_id),
                &env.id,
                &operation,
                "not_sent",
            )
            .await?;
            return Err(e.into());
        }
        let result = async {
            let (uid, login) = self.odoo.authenticate_measured(env).await?;
            // Only the call that logged in counts login in its metrics.
            if let Some(bytes) = login {
                upstream.calls += 1;
                upstream.bytes += bytes;
            }
            self.odoo
                .execute_kw_limited(
                    env,
                    uid,
                    model,
                    method,
                    args(Some(uid)),
                    Value::Object(kwargs),
                    max_response,
                )
                .await
        }
        .await;
        upstream.calls += 1;
        let outcome = if result.is_ok() {
            "succeeded"
        } else {
            "failed"
        };
        match &result {
            Ok((value, _)) => audit.finish(env, Ok(value)),
            Err(error) => audit.finish_odx_error(env, error),
        }
        self.log_outcome_as(audit, Some(&client.client_id), &env.id, &operation, outcome)
            .await?;
        // Release after the outcome is logged so drain() waits for it.
        drop(permits);
        let (value, bytes) = result?;
        upstream.bytes += bytes;
        Ok(value)
    }

    async fn tool_read(
        &self,
        client: &ClientIdentity,
        name: &str,
        arguments: Value,
    ) -> Result<ToolOutput, ToolError> {
        let extra: &[&'static str] = match name {
            "whoami" => &[],
            "search_read" => &[
                "model", "domain", "fields", "limit", "offset", "order", "to_file",
            ],
            "read" => &["model", "ids", "fields", "to_file"],
            "search" => &["model", "domain", "limit", "offset", "order"],
            "search_count" => &["model", "domain"],
            "fields" => &["model", "q", "names", "limit", "offset"],
            "models" => &["q", "limit", "offset"],
            "name_search" => &["model", "name", "domain", "limit"],
            _ => &[],
        };
        let mut args = Args::new(arguments, &allowed(extra))?;
        let (format, trunc) = common(&mut args)?;
        let env_name = args.str("env")?;
        let call_context = Value::Object(args.object("context")?);
        let mut up = Upstream::start();
        let mut meta = Map::new();
        let limit = args.int("limit", 50, 1, 1000)? as usize;
        let offset = args.int("offset", 0, 0, i64::MAX)? as usize;
        let to_file = args.bool("to_file")?;
        // Validate before accessing environments or the network.
        let plan = match name {
            "whoami" => ReadPlan::Whoami,
            "search_read" => ReadPlan::SearchRead {
                model: model(&mut args)?,
                domain: domain(&mut args)?,
                fields: with_id(fields(&mut args, &["display_name"])?),
                order: order(&mut args, "order")?,
            },
            "read" => ReadPlan::Read {
                model: model(&mut args)?,
                ids: ids(&mut args)?,
                fields: with_id(fields(&mut args, &["display_name"])?),
            },
            "search" => ReadPlan::Search {
                model: model(&mut args)?,
                domain: domain(&mut args)?,
                order: order(&mut args, "order")?,
            },
            "search_count" => ReadPlan::Count {
                model: model(&mut args)?,
                domain: domain(&mut args)?,
            },
            "fields" => ReadPlan::Fields {
                model: model(&mut args)?,
                q: args.text("q")?.unwrap_or_default(),
                names: if args.has("names") {
                    let names = field_list(&mut args, "names", &[])?;
                    if names.len() > 1000 {
                        return Err(ToolError::invalid("names accepts at most 1000 fields"));
                    }
                    Some(names)
                } else {
                    None
                },
            },
            "models" => ReadPlan::Models {
                q: args
                    .text("q")?
                    .ok_or_else(|| ToolError::invalid("missing q"))?,
            },
            "name_search" => ReadPlan::NameSearch {
                model: model(&mut args)?,
                name: match args.raw("name") {
                    Some(Value::String(s)) => s,
                    _ => return Err(ToolError::invalid("name must be text")),
                },
                domain: domain(&mut args)?,
            },
            _ => {
                return Err(ToolError::invalid(format!("unknown tool: {name}")));
            }
        };
        let target = self.target(client, &env_name, Capability::OdooRead).await?;
        let data = match plan {
            ReadPlan::Whoami => {
                let rows = self
                    .read_call_with(
                        client,
                        &target,
                        "res.users",
                        "read",
                        |uid| json!([[uid]]),
                        kw(&[(
                            "fields",
                            json!(["login", "name", "company_id", "company_ids"]),
                        )]),
                        &call_context,
                        &mut up,
                    )
                    .await?;
                let user = rows.get(0).cloned().unwrap_or(json!({}));
                json!({"uid": user["id"], "login": user["login"], "name": user["name"],
                       "company_id": user["company_id"], "company_ids": user["company_ids"]})
            }
            ReadPlan::SearchRead {
                model,
                domain,
                fields,
                order,
            } => {
                let rows = self
                    .read_call(
                        client,
                        &target,
                        &model,
                        "search_read",
                        json!([]),
                        kw(&[
                            ("domain", domain),
                            ("fields", json!(fields)),
                            ("offset", json!(offset)),
                            ("limit", json!(limit + 1)),
                            ("order", json!(order)),
                        ]),
                        &call_context,
                        &mut up,
                    )
                    .await?;
                paged(rows, limit, offset, &mut meta)
            }
            ReadPlan::Read { model, ids, fields } => {
                let rows = self
                    .read_call(
                        client,
                        &target,
                        &model,
                        "read",
                        json!([ids]),
                        kw(&[("fields", json!(fields))]),
                        &call_context,
                        &mut up,
                    )
                    .await?;
                let found: HashSet<i64> = rows
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|r| r["id"].as_i64())
                    .collect();
                let missing: Vec<i64> =
                    ids.iter().copied().filter(|i| !found.contains(i)).collect();
                if !missing.is_empty() {
                    meta.insert("missing_ids".into(), json!(missing));
                }
                rows
            }
            ReadPlan::Search {
                model,
                domain,
                order,
            } => {
                let ids = self
                    .read_call(
                        client,
                        &target,
                        &model,
                        "search",
                        json!([domain]),
                        kw(&[
                            ("offset", json!(offset)),
                            ("limit", json!(limit + 1)),
                            ("order", json!(order)),
                        ]),
                        &call_context,
                        &mut up,
                    )
                    .await?;
                paged(ids, limit, offset, &mut meta)
            }
            ReadPlan::Count { model, domain } => {
                self.read_call(
                    client,
                    &target,
                    &model,
                    "search_count",
                    json!([domain]),
                    Map::new(),
                    &call_context,
                    &mut up,
                )
                .await?
            }
            ReadPlan::Fields { model, q, names } => {
                let mut kwargs = kw(&[(
                    "attributes",
                    json!([
                        "type",
                        "relation",
                        "selection",
                        "string",
                        "required",
                        "readonly",
                        "store"
                    ]),
                )]);
                if let Some(names) = names {
                    kwargs.insert("allfields".into(), json!(names));
                }
                let raw = self
                    .read_call(
                        client,
                        &target,
                        &model,
                        "fields_get",
                        json!([]),
                        kwargs,
                        &call_context,
                        &mut up,
                    )
                    .await?;
                let q = q.to_lowercase();
                let mut rows: Vec<Value> = raw
                    .as_object()
                    .into_iter()
                    .flatten()
                    .filter(|(name, f)| {
                        q.is_empty()
                            || name.to_lowercase().contains(&q)
                            || f["string"].as_str().is_some_and(|l| l.to_lowercase().contains(&q))
                    })
                    .map(|(name, f)| json!({
                        "name": name, "type": f["type"], "relation": f.get("relation").cloned().unwrap_or(Value::Null),
                        "selection": f.get("selection").cloned().unwrap_or(Value::Null), "label": f["string"],
                        "required": f["required"], "readonly": f["readonly"], "stored": f["store"],
                    }))
                    .collect();
                rows.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
                let rows: Vec<Value> = rows.into_iter().skip(offset).take(limit + 1).collect();
                paged(Value::Array(rows), limit, offset, &mut meta)
            }
            ReadPlan::Models { q } => {
                let rows = self
                    .read_call(
                        client,
                        &target,
                        "ir.model",
                        "search_read",
                        json!([]),
                        kw(&[
                            (
                                "domain",
                                json!(["|", ["model", "ilike", q], ["name", "ilike", q]]),
                            ),
                            ("fields", json!(["model", "name"])),
                            ("offset", json!(offset)),
                            ("limit", json!(limit + 1)),
                            ("order", json!("model asc, id asc")),
                        ]),
                        &call_context,
                        &mut up,
                    )
                    .await?;
                let rows: Vec<Value> = rows
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|r| json!({"model": r["model"], "name": r["name"]}))
                    .collect();
                paged(Value::Array(rows), limit, offset, &mut meta)
            }
            ReadPlan::NameSearch {
                model,
                name,
                domain,
            } => {
                let pairs = self
                    .read_call(
                        client,
                        &target,
                        &model,
                        "name_search",
                        // Pass domain positionally: args in Odoo 16–18, domain in 19.
                        json!([name, domain, "ilike", limit + 1]),
                        Map::new(),
                        &call_context,
                        &mut up,
                    )
                    .await?;
                let mut rows: Vec<Value> = pairs
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|p| json!({"id": p[0], "name": p[1]}))
                    .collect();
                let more = rows.len() > limit;
                rows.truncate(limit);
                if more {
                    let mut out = ToolOutput {
                        data: json!(null),
                        meta: Map::new(),
                        format,
                        trunc,
                    };
                    out.warn(
                        "refine_query",
                        "more matches exist; refine name or domain (name_search has no offset)",
                    );
                    meta = out.meta;
                }
                Value::Array(rows)
            }
        };
        up.write_meta(&mut meta);
        let out = ToolOutput {
            data,
            meta,
            format,
            trunc,
        };
        if to_file {
            return self.output_to_file(out, OutputKind::Tsv, name).await;
        }
        Ok(out)
    }

    async fn tool_mutation(
        self: &Arc<Self>,
        client: &ClientIdentity,
        name: &str,
        arguments: Value,
    ) -> Result<ToolOutput, ToolError> {
        let extra: &[&'static str] = match name {
            "create" => &["model", "values"],
            "write" => &["model", "ids", "values"],
            "unlink" => &["model", "ids"],
            _ => &["model", "method", "args", "kwargs", "to_file"],
        };
        let mut args = Args::new(arguments, &allowed(extra))?;
        let (format, trunc) = common(&mut args)?;
        let to_file = args.bool("to_file")?;
        let env_name = args.str("env")?;
        let explicit_context = args.has("context");
        let mut call_context = args.object("context")?;
        let model = model(&mut args)?;
        // Method, args, kwargs without context, effect. create/write/unlink share call validation and delete policy.
        let (method, positional, mut kwargs, mut effect) = match name {
            "create" | "write" | "unlink" => {
                let positional = match name {
                    "create" => json!([args
                        .raw("values")
                        .ok_or_else(|| ToolError::invalid("missing values"))?]),
                    "write" => json!([ids(&mut args)?, Value::Object(args.object("values")?)]),
                    _ => json!([ids(&mut args)?]),
                };
                let effect = mutations::call_effect(
                    name,
                    positional.as_array().expect("lista"),
                    &Map::new(),
                    &[],
                )?;
                (name.to_string(), positional, Map::new(), effect)
            }
            _ => {
                let method = args.str("method")?;
                if !is_identifier(&method, false) || method.starts_with('_') {
                    return Err(ToolError::invalid(format!("method not allowed: {method}")));
                }
                let positional = Value::Array(args.array("args")?.unwrap_or_default());
                let mut kwargs = args.object("kwargs")?;
                if let Some(ctx) = kwargs.remove("context") {
                    if explicit_context {
                        return Err(ToolError::invalid(
                            "set context in context or kwargs.context, not both",
                        ));
                    }
                    let Value::Object(ctx) = ctx else {
                        return Err(ToolError::invalid("kwargs.context must be an object"));
                    };
                    call_context = ctx;
                }
                if READ_METHODS.contains(&method.as_str()) {
                    // Known reads share the read tools' execution path and permissions.
                    let target = self.target(client, &env_name, Capability::OdooRead).await?;
                    crate::odoo::json2::reject_positional(&target.env, &positional)?;
                    let request = super::query::prepare_read(
                        &method,
                        positional.as_array().expect("args is a list").clone(),
                        kwargs,
                        target.env.protocol == crate::config::Protocol::Json2,
                    )?;
                    let mut up = Upstream::start();
                    let data = self
                        .read_call(
                            client,
                            &target,
                            &model,
                            &method,
                            request.args,
                            request.kwargs,
                            &Value::Object(call_context),
                            &mut up,
                        )
                        .await?;
                    let mut meta = Map::new();
                    up.write_meta(&mut meta);
                    let mut out = ToolOutput {
                        data,
                        meta,
                        format,
                        trunc,
                    };
                    if let Some((limit, offset)) = request.page {
                        out.page(limit, offset);
                    }
                    if let Some(limit) = request.name_limit
                        && let Some(rows) = out.data.as_array_mut()
                    {
                        let more = rows.len() > limit;
                        rows.truncate(limit);
                        if more {
                            out.warn("refine_query", "more matches exist; refine name or domain (name_search has no offset)");
                        }
                    }
                    if to_file {
                        return self.output_to_file(out, OutputKind::Json, &method).await;
                    }
                    return Ok(out);
                }
                let effect = mutations::call_effect(
                    &method,
                    positional.as_array().expect("args must be a list"),
                    &kwargs,
                    &[],
                )?;
                (method, positional, kwargs, effect)
            }
        };
        let target = self
            .target(client, &env_name, Capability::OdooWriteRequest)
            .await?;
        if name == "call" {
            crate::odoo::json2::reject_positional(&target.env, &positional)?;
        }
        // Odoo turns x2many false/null and flat id lists into clear/set; classify them by field type.
        let mut checked = Upstream::start();
        let args_list = positional.as_array().expect("args must be a list");
        let shorthand = mutations::shorthand_keys(&method, args_list, &kwargs);
        if !shorthand.is_empty() {
            let types = self
                .read_call(
                    client,
                    &target,
                    &model,
                    "fields_get",
                    json!([]),
                    Map::from_iter([
                        ("allfields".into(), json!(shorthand)),
                        ("attributes".into(), json!(["type"])),
                    ]),
                    &Value::Object(call_context.clone()),
                    &mut checked,
                )
                .await?;
            let x2many: Vec<String> = shorthand
                .into_iter()
                .filter(|k| matches!(types[k]["type"].as_str(), Some("one2many" | "many2many")))
                .collect();
            effect = mutations::call_effect(&method, args_list, &kwargs, &x2many)?;
        }
        kwargs.insert(
            "context".into(),
            effective_context(
                &target.global,
                &target.env.context,
                &Value::Object(call_context),
            ),
        );
        let (data, up, approval_wait) = self
            .submit(
                client,
                &target,
                &model,
                &method,
                positional,
                Value::Object(kwargs),
                effect,
            )
            .await?;
        let mut up = up;
        up.calls += checked.calls;
        up.bytes += checked.bytes;
        let mut meta = Map::new();
        up.write_meta(&mut meta);
        meta.insert(
            "approval_wait_ms".into(),
            json!(approval_wait.as_millis() as u64),
        );
        let data = if name == "create" {
            match data {
                Value::Number(id) => json!([id]),
                other => other,
            }
        } else {
            data
        };
        let mut out = ToolOutput {
            data,
            meta,
            format,
            trunc,
        };
        if to_file {
            // The write succeeded; file failure adds a warning with inline or omitted output instead of reporting write failure.
            match self
                .output_to_file(out.clone(), OutputKind::Json, &method)
                .await
            {
                Ok(saved) => return Ok(saved),
                Err(error) => out.warn(
                    "output_storage_error",
                    format!(
                        "write succeeded but the output file could not be saved: {}",
                        error.message
                    ),
                ),
            }
        }
        if crate::output::fit(out.clone()).is_err() {
            // Oversized results cannot turn a successful write into a reported failure.
            out.warn(
                "output_omitted",
                "write succeeded but its result exceeds the output limit",
            );
            out.data = json!({"omitted": true});
        }
        Ok(out)
    }

    /// Register, execute if permitted or await human approval. Return result, upstream metrics and human wait time (zero in free-write mode).
    #[allow(clippy::too_many_arguments)]
    async fn submit(
        self: &Arc<Self>,
        client: &ClientIdentity,
        target: &Target,
        model: &str,
        method: &str,
        args: Value,
        kwargs: Value,
        effect: Effect,
    ) -> Result<(Value, Upstream, Duration), ToolError> {
        let env = &target.env;
        self.reject_credential_call(Some(&client.client_id), env, model, method, &args, &kwargs)
            .await?;
        // Human wait spans request creation to execution start.
        let requested;
        let (id, deadline, details, waiter) = {
            let mut s = self.inner.lock().await;
            let v = s.vault.as_ref().ok_or(OdxError::Locked)?;
            let now = v.now();
            // Free-write skips approval; approval mode uses a matching valid rule or a human decision. unlink and relational deletes always require individual approval.
            let authorization = match policy::authorize(env, effect.clone(), now) {
                Err(_) => {
                    return Err(ToolError::new(
                        "forbidden",
                        format!("environment {} is read-only", env.name),
                        Outcome::NotSent,
                    ));
                }
                Ok(false) => policy::Authorization::Free,
                Ok(true) => {
                    let rule = match (args.as_array(), kwargs.as_object()) {
                        (Some(positional), Some(named)) => policy::matching_rule(
                            &v.data().rules,
                            &policy::EffectiveCall {
                                environment_id: &env.id,
                                model,
                                method,
                                args: positional,
                                kwargs: named,
                            },
                            &effect,
                            now,
                        ),
                        _ => None,
                    };
                    match rule {
                        Some(rule) => policy::Authorization::Rule {
                            rule_id: rule.id.clone(),
                            rule_revision: rule.revision,
                        },
                        None => policy::Authorization::Individual,
                    }
                }
            };
            let needs_approval = authorization == policy::Authorization::Individual;
            let minutes = s
                .vault
                .as_ref()
                .map_or(5, |v| v.data().settings.approval_minutes);
            // Vault clock: a system clock moved back must not extend approvals.
            let now = s.vault.as_ref().map_or_else(Utc::now, |v| v.now());
            // Remove only orphaned terminal/expired requests without waiters. Approved and sent requests remain until their outcome.
            let crate::execution::Inner {
                pending, waiters, ..
            } = &mut *s;
            pending.retain(|id, p| {
                use crate::approval::State;
                waiters.contains_key(id)
                    || matches!(p.state, State::Approved | State::Sent)
                    || (p.state == State::Pending && p.call.expires_at > now)
            });
            if s.pending.len() >= 100
                || s.pending
                    .values()
                    .filter(|p| p.call.environment_id == env.id)
                    .count()
                    >= 20
            {
                return Err(OdxError::Capacity.into());
            }
            let mut approval = Approval::with_ttl(
                env,
                model.into(),
                method.into(),
                args,
                kwargs,
                chrono::Duration::minutes(i64::from(minutes)),
                now,
            );
            approval.client_id = Some(client.client_id.clone());
            approval.details.authorization = Some(authorization.clone());
            approval.authorization = authorization;
            let id = approval.id.clone();
            // Wait on Instant from the vault-clock TTL; system time can be skewed against the vault clock.
            let ttl = (approval.call.expires_at - now)
                .to_std()
                .unwrap_or_default();
            let details = approval.details.clone();
            s.pending.insert(id.clone(), approval);
            requested = Instant::now();
            let deadline = requested + ttl;
            let waiter = if needs_approval {
                let (tx, rx) = oneshot::channel();
                s.waiters.insert(id.clone(), tx);
                let client_label = s.vault.as_ref().and_then(|v| {
                    v.data()
                        .clients
                        .iter()
                        .find(|g| g.id == client.client_id)
                        .map(|g| g.label.clone())
                });
                let _ = self.notices.send(crate::execution::ApprovalNotice {
                    approval_id: id.clone(),
                    client_label,
                    environment_name: env.name.clone(),
                    model: model.into(),
                    method: method.into(),
                });
                Some(rx)
            } else {
                None
            };
            (id, deadline, details, waiter)
        };
        let Some(mut waiter) = waiter else {
            // Free-write/rule execution uses the human-approval path in a separate task. Cancellation or disconnect cannot interrupt sent writes or their outcome audit.
            let core = self.clone();
            return tokio::spawn(async move { core.decide_measured(&id, true, None).await })
                .await
                .map_err(|_| {
                    ToolError::new(
                        "uncertain_result",
                        "execution interrupted; Odoo may have applied the write",
                        Outcome::Uncertain,
                    )
                })?
                .map(|(value, up)| (value, up, Duration::ZERO))
                .map_err(Into::into);
        };
        // Register in a separate task with the guard armed before awaiting it; cancellation removes the request after registration completes.
        let logging = {
            let (core, client_id, env_id) =
                (self.clone(), client.client_id.clone(), env.id.clone());
            let (operation, id) = (format!("{method}:{model}"), id.clone());
            tokio::spawn(async move {
                let logged = core
                    .log_request_as(
                        Some(&client_id),
                        &env_id,
                        &operation,
                        "pending",
                        &id,
                        details,
                    )
                    .await;
                if logged.is_err() {
                    // Failed registration removes the request without an Activity entry.
                    let mut s = core.inner.lock().await;
                    s.pending.remove(&id);
                    s.waiters.remove(&id);
                }
                logged.map(drop)
            })
        };
        let mut guard = CancelGuard {
            core: self.clone(),
            id: id.clone(),
            armed: true,
            logging: Some(logging),
        };
        let logged = guard.logging.as_mut().expect("registration").await;
        guard.logging = None;
        match logged {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                guard.armed = false;
                return Err(error.into());
            }
            Err(_) => return Err(OdxError::Persistence.into()),
        }
        let end = match tokio::time::timeout_at(deadline.into(), &mut waiter).await {
            Ok(end) => end,
            Err(_) => match self.withdraw(&id, true).await {
                Ok(_) => {
                    guard.armed = false;
                    return Err(ToolError::new(
                        "approval_expired",
                        "request expired without a human decision",
                        Outcome::NotSent,
                    ));
                }
                // Approved or sent: receive the real outcome through the channel.
                Err(_) => waiter.await,
            },
        };
        guard.armed = false;
        match end {
            Err(_) => Err(ToolError::new(
                "approval_revoked",
                "request revoked",
                Outcome::NotSent,
            )),
            Ok(ApprovalEnd::Executed(value, up)) => {
                let wait = up.started.saturating_duration_since(requested);
                Ok((value, up, wait))
            }
            Ok(ApprovalEnd::Rejected(reason)) => Err(ToolError::new(
                "approval_rejected",
                match reason {
                    Some(reason) => format!(
                        "request rejected by a human; nothing was sent to Odoo. Reason: {reason}"
                    ),
                    None => "request rejected by a human; nothing was sent to Odoo".into(),
                },
                Outcome::NotSent,
            )),
            Ok(ApprovalEnd::Revoked) => Err(ToolError::new(
                "approval_revoked",
                "request revoked: vault, environment, mode or client changed",
                Outcome::NotSent,
            )),
            Ok(ApprovalEnd::Expired) => Err(ToolError::new(
                "approval_expired",
                "request expired without a human decision",
                Outcome::NotSent,
            )),
            Ok(ApprovalEnd::Cancelled) => Err(ToolError::new(
                "cancelled",
                "request cancelled; nothing was sent to Odoo",
                Outcome::NotSent,
            )),
            Ok(ApprovalEnd::Failed(error)) => Err(error.into()),
        }
    }
}

/// Remove the request when its client abandons the wait through cancellation, EOF or disconnect.
struct CancelGuard {
    core: Arc<Core>,
    id: String,
    armed: bool,
    /// Await pending registration before cancellation.
    logging: Option<tokio::task::JoinHandle<crate::error::Result<()>>>,
}
impl Drop for CancelGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let core = self.core.clone();
        let id = std::mem::take(&mut self.id);
        let logging = self.logging.take();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                // Failed registration already removed the request.
                if let Some(logging) = logging
                    && !matches!(logging.await, Ok(Ok(())))
                {
                    return;
                }
                // Dispatch closes approved or sent requests.
                let _ = core.cancel_approval(&id).await;
            });
        }
    }
}

enum ReadPlan {
    Whoami,
    SearchRead {
        model: String,
        domain: Value,
        fields: Vec<String>,
        order: String,
    },
    Read {
        model: String,
        ids: Vec<i64>,
        fields: Vec<String>,
    },
    Search {
        model: String,
        domain: Value,
        order: String,
    },
    Count {
        model: String,
        domain: Value,
    },
    Fields {
        model: String,
        q: String,
        names: Option<Vec<String>>,
    },
    Models {
        q: String,
    },
    NameSearch {
        model: String,
        name: String,
        domain: Value,
    },
}

pub(crate) fn kw(pairs: &[(&str, Value)]) -> Map<String, Value> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation_error_keeps_external_envelope_and_filters_desktop_parameters() {
        let error = OdxError::validation_with_params(
            "validation.rule.limit_reached",
            json!({ "limit": 128, "secret": "do-not-send" }),
            "legacy validation text",
        );
        let tool = from_error(error);
        assert_eq!(tool.code, "invalid_arguments");
        assert_eq!(tool.message, "legacy validation text");
        assert_eq!(tool.ui_code, Some("validation.rule.limit_reached"));
        assert_eq!(tool.ui_params, json!({ "limit": 128 }));
        assert_eq!(
            tool.envelope("request")["error"]["code"],
            "invalid_arguments"
        );
        assert!(tool.envelope("request").get("ui_code").is_none());
        assert!(!tool.envelope("request").to_string().contains("do-not-send"));

        let legacy = from_error(OdxError::Invalid("api_key=must-not-leak".into()));
        assert_eq!(legacy.message, "invalid input");
        assert!(
            !legacy
                .envelope("request")
                .to_string()
                .contains("must-not-leak")
        );
    }

    #[test]
    fn published_catalog_is_sorted_complete_and_self_contained() {
        let defs = tool_definitions();
        assert_eq!(defs.len(), 21);
        assert!(defs.iter().find(|d| d.name == "write").unwrap().destructive);
        let names: Vec<_> = defs.iter().map(|d| d.name).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
        for d in &defs {
            assert_eq!(d.input_schema["type"], "object", "{}", d.name);
            assert!(
                d.input_schema["properties"]
                    .as_object()
                    .unwrap()
                    .values()
                    .all(|p| p["description"].as_str().is_some_and(|s| !s.is_empty())),
                "{}: argumentos documentados",
                d.name
            );
            let to_file = d.input_schema["properties"].get("to_file").is_some();
            let with_file = [
                "search_read",
                "read",
                "read_group",
                "history",
                "view",
                "compare",
                "call",
            ];
            assert_eq!(to_file, with_file.contains(&d.name), "{}", d.name);
            assert!(!d.input_schema.to_string().contains("\"$ref\""));
            assert!(!d.description.is_empty());
        }
    }

    #[test]
    fn arguments_reject_unknown_fields_nulls_and_bad_identifiers() {
        assert!(Args::new(json!({"env":"a","extra":1}), &allowed(&[])).is_err());
        assert!(Args::new(json!({"env":null}), &allowed(&[])).is_err());
        let mut a = Args::new(json!({"model":"res.partner;drop"}), &allowed(&["model"])).unwrap();
        assert!(model(&mut a).is_err());
        let mut a = Args::new(json!({"domain":"[('id','=',1)]"}), &allowed(&["domain"])).unwrap();
        assert!(domain(&mut a).is_err());
        let mut a = Args::new(json!({"order":"name desc; drop"}), &allowed(&["order"])).unwrap();
        assert!(order(&mut a, "order").is_err());
        let mut a = Args::new(json!({"order":"name desc"}), &allowed(&["order"])).unwrap();
        assert_eq!(order(&mut a, "order").unwrap(), "name desc, id asc");
        let mut a = Args::new(json!({"fields":["name","name"]}), &allowed(&["fields"])).unwrap();
        assert!(fields(&mut a, &[]).is_err());
    }
}
