//! Desktop admin Shell. Explicit write confirmation replaces approval; read-only environments still deny writes. Shares limits, revision checks, redaction and audit.
use crate::{
    audit::AuditDetails,
    clients::Capability,
    error::OdxError,
    execution::{AuditedCall, Core, Upstream},
    output::{Outcome, ToolError},
    tools::{catalog::READ_METHODS, query::effective_context},
};
use chrono::{DateTime, Utc};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::time::Instant;

#[derive(Debug, Clone, Deserialize)]
pub struct ShellCall {
    pub environment_id: String,
    pub model: String,
    pub method: String,
    #[serde(default)]
    pub args: Vec<Value>,
    #[serde(default)]
    pub kwargs: Map<String, Value>,
    /// Shell session context overrides global and environment context.
    #[serde(default)]
    pub context: Map<String, Value>,
    /// The human confirmed the displayed write.
    #[serde(default)]
    pub confirmed: bool,
    /// An MCP client ran this through `terminal_run`: its grant is checked and the audit names it.
    #[serde(default)]
    pub client_id: Option<String>,
    /// That client's token revision when it asked; a replaced token voids the request.
    #[serde(default)]
    pub client_revision: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ShellResult {
    pub data: Value,
    pub write: bool,
    pub upstream_ms: u64,
    pub upstream_bytes: usize,
}

/// Commands persisted between sessions in the encrypted vault.
const HISTORY_LIMIT: usize = 500;
const HISTORY_MAX_COMMAND: usize = 4096;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub at: DateTime<Utc>,
    /// Connected environment name when the command ran, if any.
    pub environment: Option<String>,
    pub command: String,
}

pub(crate) fn history(
    connection: &rusqlite::Connection,
) -> crate::error::Result<Vec<HistoryEntry>> {
    let mut statement =
        connection.prepare("SELECT at,environment,command FROM shell_history ORDER BY id")?;
    let rows = statement.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get(1)?, r.get(2)?)))?;
    rows.map(|row| {
        let (at, environment, command) = row?;
        Ok(HistoryEntry {
            at: DateTime::from_timestamp_micros(at).ok_or(OdxError::Persistence)?,
            environment,
            command,
        })
    })
    .collect()
}
/// One row per command; skip consecutive duplicates in the same environment.
pub(crate) fn add_history(
    tx: &rusqlite::Transaction<'_>,
    command: &str,
    environment: Option<String>,
) -> crate::error::Result<()> {
    insert_history(
        tx,
        &HistoryEntry {
            at: Utc::now(),
            environment,
            command: command.into(),
        },
    )
}
/// Insert within count/size limits; also used by migration.
pub(crate) fn insert_history(
    tx: &rusqlite::Transaction<'_>,
    entry: &HistoryEntry,
) -> crate::error::Result<()> {
    let command = entry.command.trim();
    if command.is_empty() || command.len() > HISTORY_MAX_COMMAND {
        return Ok(());
    }
    let repeated = tx
        .query_row(
            "SELECT command=?1 AND environment IS ?2 FROM shell_history ORDER BY id DESC LIMIT 1",
            rusqlite::params![command, entry.environment],
            |r| r.get::<_, bool>(0),
        )
        .optional()?
        .unwrap_or(false);
    if repeated {
        return Ok(());
    }
    tx.execute(
        "INSERT INTO shell_history(at,environment,command) VALUES(?1,?2,?3)",
        rusqlite::params![entry.at.timestamp_micros(), entry.environment, command],
    )?;
    tx.execute(
        "DELETE FROM shell_history WHERE id NOT IN (SELECT id FROM shell_history ORDER BY id DESC LIMIT ?1)",
        [HISTORY_LIMIT as i64],
    )?;
    Ok(())
}

impl Core {
    pub async fn shell_history(&self) -> crate::error::Result<Vec<HistoryEntry>> {
        let s = self.inner.lock().await;
        s.vault()?.shell_history()
    }
    /// Add a command, skipping consecutive duplicates and oversized input.
    pub async fn shell_history_add(
        &self,
        command: &str,
        environment: Option<String>,
    ) -> crate::error::Result<()> {
        let s = self.inner.lock().await;
        s.vault()?.shell_history_add(command, environment)
    }
    pub async fn shell_sessions(&self) -> crate::error::Result<Option<String>> {
        let s = self.inner.lock().await;
        s.vault()?.shell_sessions()
    }
    pub async fn shell_sessions_save(&self, data: &str) -> crate::error::Result<()> {
        let s = self.inner.lock().await;
        s.vault()?.shell_sessions_save(data)
    }
    pub async fn shell_history_clear(&self) -> crate::error::Result<()> {
        let s = self.inner.lock().await;
        s.vault()?.shell_history_clear()
    }
    pub async fn shell_call(&self, call: ShellCall) -> Result<ShellResult, ToolError> {
        if !crate::tools::args::valid_model(&call.model) {
            return Err(ToolError::invalid(format!("invalid model: {}", call.model)));
        }
        if !crate::tools::args::valid_method(&call.method) {
            return Err(ToolError::invalid(format!(
                "method not allowed: {} (private methods are not exposed)",
                call.method
            )));
        }
        if call.kwargs.contains_key("context") && !call.context.is_empty() {
            return Err(ToolError::invalid(
                "set context in the session or kwargs.context, not both",
            ));
        }
        // Every method outside the read catalog is a potential write.
        let write = !READ_METHODS.contains(&call.method.as_str());
        if write && !call.confirmed {
            return Err(ToolError::new(
                "confirmation_required",
                "write requires confirmation",
                Outcome::NotSent,
            ));
        }
        let actor = call.client_id.clone();
        let (env, global, session, now) = {
            let s = self.inner.lock().await;
            let v = s.vault()?;
            // The app already limits agents to their grant; the core checks again, as for any client call.
            if let Some(client_id) = &actor {
                let grant = v
                    .data()
                    .clients
                    .iter()
                    .find(|g| {
                        &g.id == client_id && !g.revoked && Some(g.revision) == call.client_revision
                    })
                    .ok_or(OdxError::Unauthorized)?;
                let in_scope =
                    grant.all_environments || grant.environment_ids.contains(&call.environment_id);
                let capability = if write {
                    Capability::OdooWriteRequest
                } else {
                    Capability::OdooRead
                };
                if !in_scope || !grant.capabilities.contains(&capability) {
                    return Err(ToolError::new(
                        "forbidden",
                        "the client may not run this in that environment",
                        Outcome::NotSent,
                    ));
                }
            }
            let env = v.data().connection_environment(&call.environment_id)?;
            (env, v.data().global_context.clone(), s.session, v.now())
        };
        let mut kwargs = call.kwargs;
        let call_context = match kwargs.remove("context") {
            Some(Value::Object(ctx)) => ctx,
            Some(_) => return Err(ToolError::invalid("kwargs.context must be an object")),
            None => call.context,
        };
        kwargs.insert(
            "context".into(),
            effective_context(&global, &env.context, &Value::Object(call_context)),
        );
        let mut call_args = call.args.clone();
        let (rules, policy_now) = {
            let s = self.inner.lock().await;
            let v = s.vault()?;
            (v.data().rules.clone(), v.now())
        };
        let mut redacted_fields = Vec::new();
        if let Some(rule) =
            crate::policy::first_any_field_block(&rules, &env.id, &call.model, policy_now)
            && !READ_METHODS.contains(&call.method.as_str())
            && !matches!(call.method.as_str(), "create" | "write" | "unlink")
        {
            self.log_as(
                actor.as_deref(),
                &env.id,
                &format!("{}:{}", call.method, call.model),
                "denied",
                AuditDetails {
                    error_code: Some("app.denied".into()),
                    policy_rules: Some(vec![crate::policy::trace_rule(rule)]),
                    ..Default::default()
                },
            )
            .await
            .map_err(ToolError::from)?;
            return Err(ToolError::new(
                "forbidden",
                "opaque methods are blocked while field protection applies",
                Outcome::NotSent,
            ));
        }
        if !write
            && call.method != "fields_get"
            && !crate::policy::is_value_free_read(&call.method)
            && crate::policy::has_field_block(
                &rules,
                &env.id,
                &call.model,
                &call.method,
                policy_now,
            )
        {
            if matches!(call.method.as_str(), "read" | "search_read")
                && kwargs.contains_key("fields")
                && call_args.len() > 1
            {
                return Err(ToolError::new(
                    "forbidden",
                    "ambiguous field selection under field protection",
                    Outcome::NotSent,
                ));
            }
            let requested = match call.method.as_str() {
                "read" | "search_read" if kwargs.contains_key("fields") => {
                    kwargs.get("fields").and_then(Value::as_array)
                }
                "read" | "search_read" if call_args.len() > 1 => {
                    call_args.get(1).and_then(Value::as_array)
                }
                _ => None,
            }
            .cloned();
            let Some(requested) = requested else {
                let rule = crate::policy::first_field_block(
                    &rules,
                    &env.id,
                    &call.model,
                    &call.method,
                    policy_now,
                );
                self.log_as(
                    actor.as_deref(),
                    &env.id,
                    &format!("{}:{}", call.method, call.model),
                    "denied",
                    AuditDetails {
                        error_code: Some("app.denied".into()),
                        policy_rules: rule.map(|rule| vec![crate::policy::trace_rule(rule)]),
                        ..Default::default()
                    },
                )
                .await
                .map_err(ToolError::from)?;
                return Err(ToolError::new(
                    "forbidden",
                    "explicit fields are required when field protection applies",
                    Outcome::NotSent,
                ));
            };
            let mut allowed = Vec::new();
            for value in requested {
                let Some(field) = value.as_str() else {
                    return Err(ToolError::new(
                        "forbidden",
                        "invalid field selection under field protection",
                        Outcome::NotSent,
                    ));
                };
                if crate::policy::field_is_blocked(
                    &rules,
                    &env.id,
                    &call.model,
                    &call.method,
                    field,
                    policy_now,
                ) {
                    redacted_fields.push(field.to_owned());
                } else {
                    allowed.push(value);
                }
            }
            if allowed.is_empty() {
                let traces = crate::policy::trace_field_rules(
                    &rules,
                    &env.id,
                    &call.model,
                    &call.method,
                    &redacted_fields,
                    policy_now,
                );
                self.log_as(
                    actor.as_deref(),
                    &env.id,
                    &format!("{}:{}", call.method, call.model),
                    "denied",
                    AuditDetails {
                        error_code: Some("app.denied".into()),
                        policy_rules: Some(traces),
                        ..Default::default()
                    },
                )
                .await
                .map_err(ToolError::from)?;
                return Err(ToolError::new(
                    "forbidden",
                    "all requested fields are blocked by a rule",
                    Outcome::NotSent,
                ));
            }
            if kwargs.contains_key("fields") {
                kwargs.insert("fields".into(), Value::Array(allowed));
            } else {
                call_args[1] = Value::Array(allowed);
            }
        }
        let effective = crate::policy::EffectiveCall {
            environment_id: &env.id,
            model: &call.model,
            method: &call.method,
            args: &call_args,
            kwargs: &kwargs,
        };
        let effect = if call.method == "unlink"
            || crate::tools::mutations::has_relational_delete(&Value::Array(call_args.clone()))
        {
            crate::policy::Effect::Delete
        } else if write {
            crate::policy::Effect::Write
        } else {
            crate::policy::Effect::Read
        };
        if let Some(rule) = crate::policy::matching_block(&rules, &effective, &effect, policy_now) {
            let details = AuditDetails {
                error_code: Some("app.denied".into()),
                policy_rules: Some(vec![crate::policy::trace_rule(rule)]),
                ..Default::default()
            };
            self.log_as(
                actor.as_deref(),
                &env.id,
                &format!("{}:{}", call.method, call.model),
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
        self.reject_credential_call(
            actor.as_deref(),
            &env,
            &call.model,
            &call.method,
            &Value::Array(call_args.clone()),
            &Value::Object(kwargs.clone()),
        )
        .await?;
        // Confirmation stands in for approval but never overrides read-only or an expired deadline.
        if write {
            crate::policy::authorize(&env, crate::policy::Effect::Write, now)?;
        }
        let env = self.resolve_protocol(env, session).await?;
        let operation = format!("{}:{}", call.method, call.model);
        let mut audit_details = AuditDetails::call(
            &env,
            &call.model,
            &call.method,
            &Value::Array(call_args.clone()),
            &Value::Object(kwargs.clone()),
        );
        if !redacted_fields.is_empty() {
            audit_details.redacted_fields = Some(redacted_fields.clone());
            audit_details.policy_rules = Some(crate::policy::trace_field_rules(
                &rules,
                &env.id,
                &call.model,
                &call.method,
                &redacted_fields,
                policy_now,
            ));
        }
        let audit = self
            .audit_request(&uuid::Uuid::new_v4().to_string(), audit_details)
            .await?;
        let started = Instant::now();
        let policy_args = call_args.clone();
        let policy_kwargs = kwargs.clone();
        let policy_model = call.model.clone();
        let policy_method = call.method.clone();
        let policy_env_id = env.id.clone();
        let audited = AuditedCall {
            actor: actor.as_deref(),
            env: &env,
            revision: env.revision,
            session,
            operation: &operation,
            model: &call.model,
            method: &call.method,
            args: move |_| Value::Array(call_args),
            kwargs: Value::Object(kwargs),
            write,
            recorded: false,
            max_response: crate::odoo::jsonrpc::MAX_RESPONSE,
            initial_policy: (!write).then(|| (rules.clone(), policy_now)),
            read_relations: None,
        };
        let (data, bytes) = self
            .audited_call(audit, audited, &mut Upstream::start(), || async {
                let (current_rules, current_time) = self.policy_snapshot(&policy_env_id).await?;
                let effective = crate::policy::EffectiveCall {
                    environment_id: &policy_env_id,
                    model: &policy_model,
                    method: &policy_method,
                    args: &policy_args,
                    kwargs: &policy_kwargs,
                };
                let block_rule = crate::policy::matching_block(
                    &current_rules,
                    &effective,
                    &effect,
                    current_time,
                )
                .or_else(|| {
                    (!READ_METHODS.contains(&policy_method.as_str())
                        && !matches!(policy_method.as_str(), "create" | "write" | "unlink"))
                    .then(|| {
                        crate::policy::first_any_field_block(
                            &current_rules,
                            &policy_env_id,
                            &policy_model,
                            current_time,
                        )
                    })
                    .flatten()
                })
                .or_else(|| {
                    (!write
                        && crate::policy::has_field_block(
                            &current_rules,
                            &policy_env_id,
                            &policy_model,
                            &policy_method,
                            current_time,
                        )
                        && !crate::policy::has_explicit_read_fields(
                            &policy_args,
                            &policy_kwargs,
                            &policy_method,
                        ))
                    .then(|| {
                        crate::policy::first_field_block(
                            &current_rules,
                            &policy_env_id,
                            &policy_model,
                            &policy_method,
                            current_time,
                        )
                    })
                    .flatten()
                });
                if let Some(rule) = block_rule {
                    return Err(OdxError::RuleDenied(vec![crate::policy::trace_rule(rule)]));
                }
                Ok(env.clone())
            })
            .await?;
        let elapsed = started.elapsed().as_millis() as u64;
        Ok(ShellResult {
            data,
            write,
            upstream_ms: elapsed,
            upstream_bytes: bytes,
        })
    }
}
