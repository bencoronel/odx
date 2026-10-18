//! Desktop admin Shell. Bypasses environment write mode but requires explicit write confirmation and shares limits, revision checks, redaction and audit.
use crate::{
    audit::AuditDetails,
    error::OdxError,
    execution::Core,
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

/// Treat all methods outside the read catalog as potential writes.
pub fn is_modifying(method: &str) -> bool {
    !READ_METHODS.contains(&method)
}

fn identifier(s: &str, dotted: bool) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && !s.starts_with('_')
        && !s.starts_with('.')
        && !s.ends_with('.')
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || (dotted && c == '.'))
}

impl Core {
    pub async fn shell_history(&self) -> crate::error::Result<Vec<HistoryEntry>> {
        let s = self.inner.lock().await;
        s.vault.as_ref().ok_or(OdxError::Locked)?.shell_history()
    }
    /// Add a command, skipping consecutive duplicates and oversized input.
    pub async fn shell_history_add(
        &self,
        command: &str,
        environment: Option<String>,
    ) -> crate::error::Result<()> {
        let s = self.inner.lock().await;
        s.vault
            .as_ref()
            .ok_or(OdxError::Locked)?
            .shell_history_add(command, environment)
    }
    pub async fn shell_history_clear(&self) -> crate::error::Result<()> {
        let s = self.inner.lock().await;
        s.vault
            .as_ref()
            .ok_or(OdxError::Locked)?
            .shell_history_clear()
    }
    pub async fn shell_call(&self, call: ShellCall) -> Result<ShellResult, ToolError> {
        if !identifier(&call.model, true) {
            return Err(ToolError::invalid(format!("invalid model: {}", call.model)));
        }
        if !identifier(&call.method, false) {
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
        let write = is_modifying(&call.method);
        if write && !call.confirmed {
            return Err(ToolError::new(
                "confirmation_required",
                "write requires confirmation",
                Outcome::NotSent,
            ));
        }
        let (env, global, session) = {
            let s = self.inner.lock().await;
            let v = s.vault.as_ref().ok_or(OdxError::Locked)?;
            let env = v.data().connection_environment(&call.environment_id)?;
            (env, v.data().global_context.clone(), s.session)
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
        self.reject_credential_call(
            None,
            &env,
            &call.model,
            &call.method,
            &Value::Array(call.args.clone()),
            &Value::Object(kwargs.clone()),
        )
        .await?;
        let env = self.resolve_protocol(env, session).await?;
        let operation = format!("{}:{}", call.method, call.model);
        let permits = self.limits.acquire(&env.id).await?;
        // Environment changes during admission must still close recorded attempts.
        self.assert_environment_current(&env.id, env.revision, session)
            .await?;
        let mut audit = self
            .log_as(
                None,
                &env.id,
                &operation,
                "attempted",
                AuditDetails::call(
                    &env,
                    &call.model,
                    &call.method,
                    &Value::Array(call.args.clone()),
                    &Value::Object(kwargs.clone()),
                ),
            )
            .await?;
        if let Err(e) = self
            .assert_environment_current(&env.id, env.revision, session)
            .await
        {
            audit.finish_odx_error(&env, &e);
            self.log_outcome_as(audit, None, &env.id, &operation, "not_sent")
                .await?;
            return Err(e.into());
        }
        let started = Instant::now();
        let uid = match self.odoo.authenticate(&env).await {
            Ok(uid) => uid,
            Err(e) => {
                drop(permits);
                audit.finish_odx_error(&env, &e);
                self.log_outcome_as(audit, None, &env.id, &operation, "failed")
                    .await?;
                return Err(e.into());
            }
        };
        let result = self
            .odoo
            .execute_kw_measured(
                &env,
                uid,
                &call.model,
                &call.method,
                Value::Array(call.args),
                Value::Object(kwargs),
            )
            .await;
        drop(permits);
        let elapsed = started.elapsed().as_millis() as u64;
        let (outcome, result): (&str, Result<(Value, usize), ToolError>) = match result {
            Ok(ok) => ("succeeded", Ok(ok)),
            // Odoo error confirms the transaction failed.
            Err(OdxError::Remote(m)) => ("failed", Err(OdxError::Remote(m).into())),
            // A sent write without a response may have applied; never retry.
            Err(_) if write => ("uncertain", Err(OdxError::Uncertain.into())),
            Err(e) => ("failed", Err(e.into())),
        };
        match &result {
            Ok((value, _)) => audit.finish(&env, Ok(value)),
            Err(error) => audit.finish_tool_error(&env, error),
        }
        // Already sent: keep the real outcome; append() sets audit_failed, which blocks new operations.
        let _ = self
            .log_outcome_as(audit, None, &env.id, &operation, outcome)
            .await;
        let (data, bytes) = result?;
        Ok(ShellResult {
            data,
            write,
            upstream_ms: elapsed,
            upstream_bytes: bytes,
        })
    }
}
