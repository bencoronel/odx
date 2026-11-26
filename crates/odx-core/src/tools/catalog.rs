//! API/MCP tools and validation. All use shared grants, policy, limits and audit.
use crate::{
    audit::AuditDetails,
    clients::{Capability, ClientIdentity},
    config::Environment,
    error::OdxError,
    execution::{AuditedCall, Core, Upstream},
    output::{Outcome, ToolError, ToolOutput},
    policy::{self},
    tools::{
        args::{
            Args, common, domain, field_list, fields, ids, kw, model, order, paged, valid_method,
            with_id,
        },
        files::OutputKind,
        mutations,
        query::effective_context,
    },
};
use serde_json::{Map, Value, json};
use std::{collections::HashSet, sync::Arc};

const SCHEMAS: &str = include_str!("tools.schema.json");

/// Grant a client needs for a tool to be listed and called.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Access {
    Any,
    Read,
    Write,
    /// call: reads with odoo:read, writes with odoo:write-request.
    ReadOrWrite,
}
impl Access {
    pub(crate) fn allowed(self, capabilities: &[Capability]) -> bool {
        let read = capabilities.contains(&Capability::OdooRead);
        let write = capabilities.contains(&Capability::OdooWriteRequest);
        match self {
            Access::Any => true,
            Access::Read => read,
            Access::Write => write,
            Access::ReadOrWrite => read || write,
        }
    }
}

/// One tool's MCP metadata and grant. Its arguments are the tools.schema.json `$defs` entry of the same name.
pub(crate) struct Tool {
    pub(crate) name: &'static str,
    pub(crate) title: &'static str,
    pub(crate) description: &'static str,
    pub(crate) read_only: bool,
    pub(crate) destructive: bool,
    pub(crate) access: Access,
}

/// Every tool, in alphabetical order: the one list MCP, HTTP and run_tool share.
pub(crate) const TOOLS: [Tool; 31] = [
    Tool {
        name: "access",
        title: "Permissions",
        description: "Inspect the Odoo user's CRUD rights, ACLs and record rules. Each ACL and rule has applies: whether it matches the user's groups (null when unknown; a hint, not a guarantee). Partial data is flagged. Excludes proxy restrictions and does not guarantee permission for a specific operation.",
        read_only: true,
        destructive: false,
        access: Access::Read,
    },
    Tool {
        name: "call",
        title: "Call a method",
        description: "Call a public business method; prefer dedicated tools when available. Known reads retain their validation and limits. JSON-2 requires empty args and named kwargs, including kwargs.ids for records. Other methods follow write mode; unlink always requires human approval. Preserves the JSON result; to_file saves it locally.",
        read_only: false,
        destructive: true,
        access: Access::ReadOrWrite,
    },
    Tool {
        name: "compare",
        title: "Compare environments",
        description: "Compare model records across env_a/env_b by XML ID or an explicit key field. Reads and pages are independent, not transactional. Zero differences on a partial page do not imply equality.",
        read_only: true,
        destructive: false,
        access: Access::Read,
    },
    Tool {
        name: "create",
        title: "Create",
        description: "Create up to 1000 records. The environment mode determines execution or human approval.",
        read_only: false,
        destructive: false,
        access: Access::Write,
    },
    Tool {
        name: "download",
        title: "Download binary",
        description: "Save a binary field (default ir.attachment.datas) to a server-chosen local file. Returns path and size, never base64.",
        read_only: true,
        destructive: false,
        access: Access::Read,
    },
    Tool {
        name: "envs",
        title: "Environments",
        description: "Odoo environments granted to this client, with their project, stage (dev/test/pre/pro), write modes and tags. Makes no Odoo calls.",
        read_only: true,
        destructive: false,
        access: Access::Any,
    },
    Tool {
        name: "export_csv",
        title: "Export table as CSV",
        description: "Save a custom/computed table directly to the user's Downloads folder as a UTF-8 CSV file (comma or semicolon for Excel, never overwriting existing files). Use this when the user wants a spreadsheet-ready file from derived data.",
        read_only: true,
        destructive: false,
        access: Access::Read,
    },
    Tool {
        name: "fields",
        title: "Fields",
        description: "Inspect model field metadata before choosing fields, relations or selections. names limits upstream fields; q filters names/labels afterward.",
        read_only: true,
        destructive: false,
        access: Access::Read,
    },
    Tool {
        name: "history",
        title: "History",
        description: "Read chatter newest first with tracked changes (field, old, new). Paginates messages, not changes. Unreadable tracking returns changes=null with warnings; this does not imply no changes occurred.",
        read_only: true,
        destructive: false,
        access: Access::Read,
    },
    Tool {
        name: "models",
        title: "Models",
        description: "Find models whose technical name or label contains q.",
        read_only: true,
        destructive: false,
        access: Access::Read,
    },
    Tool {
        name: "name_search",
        title: "Search by name",
        description: "Search names as in Odoo dropdowns. No offset; refine the query when more matches exist.",
        read_only: true,
        destructive: false,
        access: Access::Read,
    },
    Tool {
        name: "open_record",
        title: "Open in browser",
        description: "Open an Odoo environment or a specific record (model + id) in the user's web browser using the environment's saved URL. Never exposes credentials.",
        read_only: true,
        destructive: false,
        access: Access::Read,
    },
    Tool {
        name: "read",
        title: "Read by IDs",
        description: "Read up to 1000 records by ID; flag missing IDs.",
        read_only: true,
        destructive: false,
        access: Access::Read,
    },
    Tool {
        name: "read_group",
        title: "Group",
        description: "Aggregate in Odoo without downloading all records. groupby supports partner_id or date:month; groupby=[] requires explicit aggregates such as amount_total:sum. Uses lazy=false and paginates groups, not records.",
        read_only: true,
        destructive: false,
        access: Access::Read,
    },
    Tool {
        name: "ref",
        title: "Resolve XML ID",
        description: "Resolve an XML ID (module.name) to {model,id}. Missing IDs return not_found; never falls back to name search.",
        read_only: true,
        destructive: false,
        access: Access::Read,
    },
    Tool {
        name: "search",
        title: "Search IDs",
        description: "Find IDs matching the domain. Paginates with limit/offset.",
        read_only: true,
        destructive: false,
        access: Access::Read,
    },
    Tool {
        name: "search_count",
        title: "Count",
        description: "Count records matching the domain.",
        read_only: true,
        destructive: false,
        access: Access::Read,
    },
    Tool {
        name: "search_read",
        title: "Search and read",
        description: "Read records and fields in one call. Defaults to id and display_name. Paginates with limit/offset. Use search for IDs only and search_count for counts.",
        read_only: true,
        destructive: false,
        access: Access::Read,
    },
    Tool {
        name: "show",
        title: "Show in Odx",
        description: "Bring the Odx window forward on something for the user to see: for now a Terminal tab. Use it when you want to show the user a result, not for every command.",
        read_only: true,
        destructive: false,
        access: Access::Read,
    },
    Tool {
        name: "show_table",
        title: "Show custom table in Odx",
        description: "Open a native interactive, filterable, sortable, exportable (CSV/JSON/TSV/Markdown) table window in the Odx desktop app with arbitrary or computed columns and rows (e.g. fields parsed out of JSON/args, joined records, grouped summaries). Use this whenever you want to present derived data that a raw Odoo model read cannot show directly.",
        read_only: true,
        destructive: false,
        access: Access::Read,
    },
    Tool {
        name: "terminal_read",
        title: "Terminal history",
        description: "Recent commands of one Odx Terminal tab with their output, newest first: text lines, errors and tables (columns and rows). Commands an agent ran carry its name. Makes no Odoo calls.",
        read_only: true,
        destructive: false,
        access: Access::Read,
    },
    Tool {
        name: "terminal_run",
        title: "Run in Terminal",
        description: "Type and run one Odx Terminal command (the Terminal syntax: search, read, count, \\d, call…) in a tab, or in a session of env, and return its output. The user can watch it in that tab; the window does not come forward (use show for that). Reads run at once. A write runs only if this client may request writes, and only after the user confirms it in the Terminal, so the call waits for that answer. Audited as this client.",
        read_only: false,
        destructive: true,
        access: Access::Read,
    },
    Tool {
        name: "terminals",
        title: "Terminal sessions",
        description: "Odx Terminal tabs on environments granted to this client: id, name, environment, whether it is the tab in front, how many commands it ran and the last one. Read from what the app saved, a second or so behind. Makes no Odoo calls.",
        read_only: true,
        destructive: false,
        access: Access::Read,
    },
    Tool {
        name: "unlink",
        title: "Delete",
        description: "Delete records by ID. Always requires human approval; one approval covers all listed IDs. Cannot be undone.",
        read_only: false,
        destructive: true,
        access: Access::Write,
    },
    Tool {
        name: "view",
        title: "View",
        description: "Read the connected user's final view architecture and paginated inheritance chain. q filters lines and marks matching views. Readable views may differ from applied views due to groups or inactive views.",
        read_only: true,
        destructive: false,
        access: Access::Read,
    },
    Tool {
        name: "view_run",
        title: "Run saved view",
        description: "Execute one of Odx's saved views (by id or name) on a granted environment, expanding dynamic date placeholders ({{today}}, {{days_ago:N}}, etc.) and returning its rows and total count.",
        read_only: true,
        destructive: false,
        access: Access::Read,
    },
    Tool {
        name: "view_save",
        title: "Save reusable view",
        description: "Create or update a read-only saved view in Odx (model, domain with optional date placeholders, fields, order, limit). Makes no Odoo calls.",
        read_only: false,
        destructive: false,
        access: Access::Read,
    },
    Tool {
        name: "views",
        title: "Saved views",
        description: "List Odx's saved views and the latest monitor results/alerts for environments granted to this client. Makes no Odoo calls.",
        read_only: true,
        destructive: false,
        access: Access::Read,
    },
    Tool {
        name: "wait",
        title: "Wait for a condition",
        description: "Poll search_count for matching records, or none with absent=true. Timeout defaults to 60 s, maximum 600 s; minimum interval 3 s. Returns matched=false on timeout.",
        read_only: true,
        destructive: false,
        access: Access::Read,
    },
    Tool {
        name: "whoami",
        title: "Odoo user",
        description: "Odoo user used by the proxy: uid, login, name and companies.",
        read_only: true,
        destructive: false,
        access: Access::Read,
    },
    Tool {
        name: "write",
        title: "Update",
        description: "Update records by ID under the environment write mode. Relational delete/unlink commands require individual human approval.",
        read_only: false,
        destructive: true,
        access: Access::Write,
    },
];

pub(crate) fn tool(name: &str) -> Option<&'static Tool> {
    TOOLS.iter().find(|tool| tool.name == name)
}

/// tools.schema.json, parsed once.
pub(crate) static SCHEMA: std::sync::LazyLock<Value> =
    std::sync::LazyLock::new(|| serde_json::from_str(SCHEMAS).expect("valid tools.schema.json"));

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

/// Published definitions with schemas from the contract.
pub fn tool_definitions() -> Vec<ToolDefinition> {
    TOOLS
        .iter()
        .map(|tool| ToolDefinition {
            name: tool.name,
            title: tool.title,
            description: tool.description,
            input_schema: SCHEMA["$defs"][tool.name].clone(),
            read_only: tool.read_only,
            destructive: tool.destructive,
        })
        .collect()
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
        if tool(name).is_some()
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
            "terminals" => self.tool_terminals(&client, arguments).await,
            "terminal_read" => self.tool_terminal_read(&client, arguments).await,
            "terminal_run" => self.tool_terminal_run(&client, arguments).await,
            "show" => self.tool_show(&client, arguments).await,
            "show_table" => self.tool_show_table(&client, arguments).await,
            "open_record" => self.tool_open_record(&client, arguments).await,
            "export_csv" => self.tool_export_csv(&client, arguments).await,
            "views" => self.tool_views(&client, arguments).await,
            "view_run" => self.tool_view_run(&client, arguments).await,
            "view_save" => self.tool_view_save(&client, arguments).await,
            _ => Err(ToolError::invalid(format!("unknown tool: {name}"))),
        }
    }

    /// Current grant; fail if the token was replaced or revoked after authentication.
    pub(crate) async fn current_grant(
        &self,
        client: &ClientIdentity,
    ) -> Result<(Vec<String>, Vec<Capability>), ToolError> {
        let s = self.inner.lock().await;
        let v = s.vault()?;
        let grant = v
            .data()
            .clients
            .iter()
            .find(|g| g.id == client.client_id && !g.revoked && g.revision == client.revision)
            .ok_or(OdxError::Unauthorized)?;
        // Resolved per call, so an all-environment grant includes environments added since.
        let current = crate::clients::identity(grant, v.data());
        Ok((current.environment_ids, current.capabilities))
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

    pub(crate) async fn configured_target(
        &self,
        client: &ClientIdentity,
        name: &str,
        capability: Capability,
    ) -> Result<Target, ToolError> {
        let s = self.inner.lock().await;
        let v = s.vault()?;
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
        let mut args = Args::for_tool("envs", arguments)?;
        let (format, trunc) = common(&mut args)?;
        let s = self.inner.lock().await;
        let v = s.vault()?;
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
                    "project": e.project,
                    "stage": e.stage,
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
        let (policy_rules, now) = self
            .policy_snapshot(&env.id)
            .await
            .map_err(ToolError::from)?;
        let mut positional = args(None);
        let domain_paths = positional
            .as_array()
            .map(|args| {
                policy::domain_paths(&policy::EffectiveCall {
                    environment_id: &env.id,
                    model,
                    method,
                    args,
                    kwargs: &kwargs,
                })
            })
            .unwrap_or_default();
        let mut resolved_domain = None;
        if !domain_paths.is_empty() && policy::has_any_read_block(&policy_rules, &env.id, now) {
            let mut relations = Vec::new();
            for path in domain_paths {
                let mut current = model.to_owned();
                let parts: Vec<_> = path.split('.').collect();
                if parts.len() > 16
                    || parts
                        .iter()
                        .any(|part| !crate::tools::args::is_identifier(part, false))
                {
                    return Err(ToolError::new(
                        "forbidden",
                        "invalid relational domain path",
                        Outcome::NotSent,
                    ));
                }
                for (index, field) in parts.iter().enumerate() {
                    let metadata = Box::pin(self.read_call(
                        client,
                        target,
                        &current,
                        "fields_get",
                        json!([]),
                        Map::from_iter([
                            ("allfields".into(), json!([field])),
                            ("attributes".into(), json!(["type", "relation"])),
                        ]),
                        call_context,
                        upstream,
                    ))
                    .await?;
                    let Some(definition) = metadata.get(*field).and_then(Value::as_object) else {
                        let traces = policy::trace_field_rules(
                            &policy_rules,
                            &env.id,
                            &current,
                            "fields_get",
                            &[(*field).to_owned()],
                            now,
                        );
                        self.log_as(
                            Some(&client.client_id),
                            &env.id,
                            &format!("{method}:{model}"),
                            "denied",
                            AuditDetails {
                                error_code: Some("app.denied".into()),
                                policy_rules: (!traces.is_empty()).then_some(traces),
                                ..Default::default()
                            },
                        )
                        .await
                        .map_err(ToolError::from)?;
                        return Err(ToolError::new(
                            "forbidden",
                            "relation metadata unavailable",
                            Outcome::NotSent,
                        ));
                    };
                    relations.push((current.clone(), (*field).to_owned()));
                    if index + 1 < parts.len() {
                        let relation = definition
                            .get("relation")
                            .and_then(Value::as_str)
                            .filter(|v| crate::tools::args::valid_model(v))
                            .ok_or_else(|| {
                                ToolError::new(
                                    "forbidden",
                                    "relation target unavailable",
                                    Outcome::NotSent,
                                )
                            })?;
                        current = relation.to_owned();
                    }
                }
            }
            resolved_domain = Some(relations);
        }
        let field_rule = policy::has_field_block(&policy_rules, &env.id, model, method, now);
        let mut redacted_fields = Vec::<String>::new();
        if field_rule && method != "fields_get" && !policy::is_value_free_read(method) {
            let values = positional.as_array_mut().ok_or_else(|| {
                ToolError::new(
                    "forbidden",
                    "ambiguous read arguments under field protection",
                    Outcome::NotSent,
                )
            })?;
            let (field_index, named_fields) = match method {
                "read" if kwargs.contains_key("fields") => {
                    if values.len() > 1 {
                        return Err(ToolError::new(
                            "forbidden",
                            "ambiguous field selection under field protection",
                            Outcome::NotSent,
                        ));
                    }
                    (None, true)
                }
                "read" => (Some(1usize), false),
                "search_read" if kwargs.contains_key("fields") => {
                    if values.len() > 1 {
                        return Err(ToolError::new(
                            "forbidden",
                            "ambiguous field selection under field protection",
                            Outcome::NotSent,
                        ));
                    }
                    (None, true)
                }
                "search_read" => (Some(1usize), false),
                _ => {
                    return Err(ToolError::new(
                        "forbidden",
                        "this read method cannot be safely filtered under field protection",
                        Outcome::NotSent,
                    ));
                }
            };
            let request_fields = if named_fields {
                if kwargs.get("fields").is_some_and(|v| !v.is_array()) {
                    return Err(ToolError::new(
                        "forbidden",
                        "ambiguous field selection under field protection",
                        Outcome::NotSent,
                    ));
                }
                kwargs.get("fields").and_then(Value::as_array)
            } else {
                let max_positional = if method == "search_read" { 5 } else { 2 };
                if kwargs.contains_key("fields") || values.len() > max_positional {
                    return Err(ToolError::new(
                        "forbidden",
                        "ambiguous field selection under field protection",
                        Outcome::NotSent,
                    ));
                }
                field_index
                    .and_then(|index| values.get(index))
                    .and_then(Value::as_array)
            };
            let Some(request_fields) = request_fields else {
                return Err(ToolError::new(
                    "forbidden",
                    "explicit fields are required when field protection applies",
                    Outcome::NotSent,
                ));
            };
            let mut allowed = Vec::new();
            for value in request_fields {
                let Some(field) = value.as_str() else {
                    return Err(ToolError::new(
                        "forbidden",
                        "invalid field selection under field protection",
                        Outcome::NotSent,
                    ));
                };
                if policy::field_is_blocked(&policy_rules, &env.id, model, method, field, now) {
                    redacted_fields.push(field.to_owned());
                } else {
                    allowed.push(value.clone());
                }
            }
            if allowed.is_empty() {
                let details = AuditDetails {
                    error_code: Some("app.denied".into()),
                    policy_rules: Some(policy::trace_field_rules(
                        &policy_rules,
                        &env.id,
                        model,
                        method,
                        &redacted_fields,
                        now,
                    )),
                    ..Default::default()
                };
                self.log_as(
                    Some(&client.client_id),
                    &env.id,
                    &format!("{method}:{model}"),
                    "denied",
                    details,
                )
                .await
                .map_err(ToolError::from)?;
                return Err(ToolError::new(
                    "forbidden",
                    "all requested fields are blocked by a rule",
                    Outcome::NotSent,
                ));
            }
            if named_fields {
                kwargs.insert("fields".into(), Value::Array(allowed));
            } else {
                values[1] = Value::Array(allowed);
            }
        }
        if let Some(positional_args) = positional.as_array() {
            let call = policy::EffectiveCall {
                environment_id: &env.id,
                model,
                method,
                args: positional_args,
                kwargs: &kwargs,
            };
            if let Some(block_rule) = policy::matching_block_with_domain(
                &policy_rules,
                &call,
                &policy::Effect::Read,
                now,
                resolved_domain.is_some(),
            )
            .or_else(|| {
                resolved_domain.as_ref().and_then(|relations| {
                    policy::matching_read_relations(&policy_rules, &call, relations, now)
                })
            }) {
                let details = AuditDetails {
                    error_code: Some("app.denied".into()),
                    policy_rules: Some(vec![policy::trace_rule(block_rule)]),
                    ..Default::default()
                };
                self.log_as(
                    Some(&client.client_id),
                    &env.id,
                    &format!("{method}:{model}"),
                    "denied",
                    details,
                )
                .await
                .map_err(ToolError::from)?;
                return Err(ToolError::new(
                    "forbidden",
                    "operation blocked by a rule",
                    Outcome::NotSent,
                ));
            }
        }
        let operation = format!("{method}:{model}");
        let mut audit_details = AuditDetails::call(
            env,
            model,
            method,
            &positional,
            &Value::Object(kwargs.clone()),
        );
        if !redacted_fields.is_empty() {
            audit_details.redacted_fields = Some(redacted_fields.clone());
            audit_details.policy_rules = Some(policy::trace_field_rules(
                &policy_rules,
                &env.id,
                model,
                method,
                &redacted_fields,
                now,
            ));
        }
        let audit = self
            .audit_request(&uuid::Uuid::new_v4().to_string(), audit_details)
            .await?;
        let dispatch_args = args;
        let redacted_for_dispatch = redacted_fields.clone();
        let dispatched_kwargs = kwargs.clone();
        let filtered_positional_fields = positional
            .as_array()
            .and_then(|values| values.get(1))
            .cloned();
        let policy_args = positional.clone();
        let policy_kwargs = kwargs.clone();
        let call = AuditedCall {
            actor: Some(&client.client_id),
            env,
            revision: env.revision,
            session: target.session,
            operation: &operation,
            model,
            method,
            args: move |uid: i64| {
                let mut call_args = dispatch_args(Some(uid));
                if !redacted_for_dispatch.is_empty()
                    && matches!(method, "read" | "search_read")
                    && !dispatched_kwargs.contains_key("fields")
                    && let Some(values) = call_args.as_array_mut()
                    && values.len() > 1
                    && let Some(filtered_fields) = &filtered_positional_fields
                {
                    values[1] = filtered_fields.clone();
                }
                call_args
            },
            kwargs: Value::Object(kwargs),
            write: false,
            recorded: false,
            max_response,
            initial_policy: Some((policy_rules.clone(), now)),
            read_relations: resolved_domain.clone(),
        };
        let (value, _) = self
            .audited_call(audit, call, upstream, || async {
                let (current_rules, current_time) = self.policy_snapshot(&env.id).await?;
                let Some(policy_args) = policy_args.as_array() else {
                    return Err(OdxError::Denied);
                };
                let effective = policy::EffectiveCall {
                    environment_id: &env.id,
                    model,
                    method,
                    args: policy_args,
                    kwargs: &policy_kwargs,
                };
                let block_rule = policy::matching_block_with_domain(
                    &current_rules,
                    &effective,
                    &policy::Effect::Read,
                    current_time,
                    resolved_domain.is_some(),
                )
                .or_else(|| {
                    resolved_domain.as_ref().and_then(|relations| {
                        policy::matching_read_relations(
                            &current_rules,
                            &effective,
                            relations,
                            current_time,
                        )
                    })
                })
                .or_else(|| {
                    (policy::has_field_block(&current_rules, &env.id, model, method, current_time)
                        && !policy::has_explicit_read_fields(policy_args, &policy_kwargs, method))
                    .then(|| {
                        policy::first_field_block(
                            &current_rules,
                            &env.id,
                            model,
                            method,
                            current_time,
                        )
                    })
                    .flatten()
                });
                if let Some(rule) = block_rule {
                    return Err(OdxError::RuleDenied(vec![policy::trace_rule(rule)]));
                }
                Ok(env.clone())
            })
            .await?;
        Ok(value)
    }

    async fn tool_read(
        &self,
        client: &ClientIdentity,
        name: &str,
        arguments: Value,
    ) -> Result<ToolOutput, ToolError> {
        let mut args = Args::for_tool(name, arguments)?;
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
        let mut args = Args::for_tool(name, arguments)?;
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
                    "create" => vec![
                        args.raw("values")
                            .ok_or_else(|| ToolError::invalid("missing values"))?,
                    ],
                    "write" => vec![
                        json!(ids(&mut args)?),
                        Value::Object(args.object("values")?),
                    ],
                    _ => vec![json!(ids(&mut args)?)],
                };
                let effect = mutations::call_effect(name, &positional, &Map::new(), &[], 0)?;
                (name.to_string(), positional, Map::new(), effect)
            }
            _ => {
                let method = args.str("method")?;
                if !valid_method(&method) {
                    return Err(ToolError::invalid(format!("method not allowed: {method}")));
                }
                let positional = args.array("args")?.unwrap_or_default();
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
                        positional.clone(),
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
                let effect = if READ_METHODS.contains(&method.as_str()) {
                    policy::Effect::Read
                } else {
                    mutations::call_effect(&method, &positional, &kwargs, &[], 0)?
                };
                (method, positional, kwargs, effect)
            }
        };
        // Odoo applies default_* context values to the records a call creates, x2many commands included.
        if mutations::has_relational_delete(&Value::Object(call_context.clone())) {
            return Err(ToolError::invalid(
                "context cannot contain relational delete commands; send them in values",
            ));
        }
        let target = self
            .target(client, &env_name, Capability::OdooWriteRequest)
            .await?;
        if name == "call" {
            crate::odoo::json2::reject_positional(&target.env, &positional)?;
        }
        // Odoo turns x2many false/null and flat id lists into clear/set; classify them by field type.
        let mut checked = Upstream::start();
        let args_list = &positional;
        let shorthand = mutations::shorthand_keys(&method, args_list, &kwargs);
        let nested = mutations::nested_shorthand_keys(&method, args_list, &kwargs);
        if !shorthand.is_empty() || !nested.is_empty() {
            let is_x2many = |types: &Value, k: &str| {
                matches!(types[k]["type"].as_str(), Some("one2many" | "many2many"))
            };
            let asked: Vec<&str> = shorthand
                .iter()
                .map(String::as_str)
                .chain(nested.iter().map(|(field, _)| field.as_str()))
                .collect();
            let types = self
                .read_call(
                    client,
                    &target,
                    &model,
                    "fields_get",
                    json!([]),
                    Map::from_iter([
                        ("allfields".into(), json!(asked)),
                        ("attributes".into(), json!(["type", "relation"])),
                    ]),
                    &Value::Object(call_context.clone()),
                    &mut checked,
                )
                .await?;
            let x2many: Vec<String> = shorthand
                .into_iter()
                .filter(|k| is_x2many(&types, k))
                .collect();
            // Shorthand inside create/update commands targets comodel fields: one fields_get per comodel.
            let mut nested_deletes = 0;
            let mut comodels: std::collections::BTreeMap<&str, Vec<&str>> = Default::default();
            for (field, key) in &nested {
                if let Some(comodel) = types[field.as_str()]["relation"].as_str() {
                    comodels.entry(comodel).or_default().push(key);
                }
            }
            for (comodel, keys) in comodels {
                let comodel_types = self
                    .read_call(
                        client,
                        &target,
                        comodel,
                        "fields_get",
                        json!([]),
                        Map::from_iter([
                            ("allfields".into(), json!(keys)),
                            ("attributes".into(), json!(["type"])),
                        ]),
                        &Value::Object(call_context.clone()),
                        &mut checked,
                    )
                    .await?;
                nested_deletes += keys.iter().filter(|k| is_x2many(&comodel_types, k)).count();
            }
            effect = mutations::call_effect(&method, args_list, &kwargs, &x2many, nested_deletes)?;
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
                Value::Array(positional),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_registry_and_the_schema_list_the_same_tools() {
        let mut schema: Vec<&str> = SCHEMA["$defs"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        schema.sort();
        let names: Vec<&str> = TOOLS.iter().map(|tool| tool.name).collect();
        assert_eq!(
            names, schema,
            "registry sorted and matching tools.schema.json"
        );
        for tool in &TOOLS {
            assert!(
                !tool.title.is_empty() && !tool.description.is_empty(),
                "{}",
                tool.name
            );
            assert!(
                SCHEMA["$defs"][tool.name]["properties"].is_object(),
                "{}",
                tool.name
            );
        }
    }

    #[test]
    fn published_catalog_is_sorted_complete_and_self_contained() {
        let defs = tool_definitions();
        assert_eq!(defs.len(), 31);
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
}
