//! SQLCipher-encrypted audit events with one projection per request.
use crate::execution::Core;
use crate::{
    config::Environment,
    error::{OdxError, Result},
};
use chrono::{DateTime, Utc};
use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    io::Write,
    sync::atomic::{AtomicUsize, Ordering},
};

/// Escaped payload budgets: responses get the most room so reads can be shown as tables and JSON.
const REQUEST_BUDGET: usize = 8000;
const RESPONSE_BUDGET: usize = 64000;
const ERROR_BUDGET: usize = 4000;
const MAX_EVENT: usize = 80 * 1024;
const MAX_QUEUED_EVENTS: usize = 128;
const MAX_HISTORY: u64 = 256 * 1024 * 1024;
/// Prune on the first insert and every 64th after; 64 events of at most MAX_EVENT stay far below the page reserve.
const PRUNE_EVERY: u32 = 64;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<AuditDetails>,
    pub at: DateTime<Utc>,
    /// Originating API/MCP client; absent for local administration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    pub environment_id: String,
    pub operation: String,
    pub outcome: String,
}

#[derive(Clone)]
pub(crate) struct AuditRequest {
    pub log: std::sync::Arc<AuditLog>,
    pub id: String,
    pub details: AuditDetails,
    pub started: std::time::Instant,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AuditDetails {
    pub request: Option<AuditPayload>,
    pub response: Option<AuditPayload>,
    pub error: Option<AuditPayload>,
    /// Stable application error identity for localized desktop activity details.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_params: Option<Value>,
    pub duration_ms: Option<u64>,
    /// Write authorization: individual approval, rule ID or free-write mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorization: Option<crate::policy::Authorization>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditPayload {
    pub text: String,
    pub truncated: bool,
}
impl AuditDetails {
    pub(crate) fn call(
        env: &Environment,
        model: &str,
        method: &str,
        args: &Value,
        kwargs: &Value,
    ) -> Self {
        Self {
            request: Some(AuditPayload::capture(
                &serde_json::json!({
                    "model": model, "method": method, "args": args, "kwargs": kwargs
                }),
                env,
                REQUEST_BUDGET,
            )),
            ..Default::default()
        }
    }
}
impl AuditRequest {
    pub(crate) fn finish(
        &mut self,
        env: &Environment,
        result: std::result::Result<&Value, String>,
    ) {
        self.details.duration_ms = Some(self.started.elapsed().as_millis() as u64);
        match result {
            Ok(value) => {
                self.details.response = Some(AuditPayload::capture(value, env, RESPONSE_BUDGET))
            }
            Err(error) => {
                self.details.error = Some(AuditPayload::capture(
                    &Value::String(error),
                    env,
                    ERROR_BUDGET,
                ))
            }
        }
    }

    pub(crate) fn finish_odx_error(&mut self, env: &Environment, error: &OdxError) {
        self.details.duration_ms = Some(self.started.elapsed().as_millis() as u64);
        self.details.error_code = Some(error.code().into());
        self.details.error_params = Some(error.ui_params());
        if let Some(detail) = error.safe_detail() {
            self.details.error = Some(AuditPayload::capture(
                &Value::String(detail.into()),
                env,
                ERROR_BUDGET,
            ));
        }
    }
}

impl AuditPayload {
    fn capture(value: &Value, env: &Environment, budget: usize) -> Self {
        // Include JSON escaping in the MAX_EVENT budget.
        let mut writer = PreviewWriter {
            bytes: Vec::new(),
            budget,
        };
        let truncated =
            serde_json::to_writer_pretty(&mut writer, &Redacted { value, env }).is_err();
        while std::str::from_utf8(&writer.bytes).is_err() {
            writer.bytes.pop();
        }
        Self {
            text: String::from_utf8(writer.bytes).unwrap(),
            truncated,
        }
    }
}

// Audit logging on the core: open a request, record its outcome, and refuse work for a changed environment.

impl Core {
    /// Record the attempt before dispatch; audit failure blocks new operations.
    pub(crate) async fn log_as(
        &self,
        client: Option<&str>,
        env: &str,
        operation: &str,
        outcome: &str,
        details: AuditDetails,
    ) -> Result<AuditRequest> {
        self.log_request_as(
            client,
            env,
            operation,
            outcome,
            &uuid::Uuid::new_v4().to_string(),
            details,
        )
        .await
    }
    pub(crate) async fn log_request_as(
        &self,
        client: Option<&str>,
        env: &str,
        operation: &str,
        outcome: &str,
        request_id: &str,
        details: AuditDetails,
    ) -> Result<AuditRequest> {
        let request = self.audit_request(request_id, details).await?;
        self.log_open(request.clone(), client, env, operation, outcome)
            .await?;
        Ok(request)
    }
    pub(crate) async fn audit_request(
        &self,
        id: &str,
        details: AuditDetails,
    ) -> Result<AuditRequest> {
        let s = self.inner.lock().await;
        if s.audit_failed {
            return Err(OdxError::Persistence);
        }
        Ok(AuditRequest {
            log: s.audit.as_ref().cloned().ok_or(OdxError::Locked)?,
            id: id.into(),
            details,
            started: std::time::Instant::now(),
        })
    }
    /// Opening or advancing a request uses the bounded audit queue.
    pub(crate) async fn log_open(
        &self,
        audit: AuditRequest,
        client: Option<&str>,
        env: &str,
        operation: &str,
        outcome: &str,
    ) -> Result<()> {
        self.append(audit, client, env, operation, outcome, true)
            .await
    }
    /// Closing a recorded request is bounded by admission, not queue capacity.
    pub(crate) async fn log_outcome_as(
        &self,
        audit: AuditRequest,
        client: Option<&str>,
        env: &str,
        operation: &str,
        outcome: &str,
    ) -> Result<()> {
        self.append(audit, client, env, operation, outcome, false)
            .await
    }
    async fn append(
        &self,
        audit: AuditRequest,
        client: Option<&str>,
        env: &str,
        operation: &str,
        outcome: &str,
        bounded: bool,
    ) -> Result<()> {
        let event = AuditEvent {
            request_id: Some(audit.id),
            details: Some(audit.details),
            at: Utc::now(),
            client_id: client.map(Into::into),
            environment_id: env.into(),
            operation: operation.into(),
            outcome: outcome.into(),
        };
        // Run SQLCipher fsync outside worker threads.
        let log = audit.log;
        let appended = tokio::task::spawn_blocking(move || {
            if bounded {
                log.append(&event)
            } else {
                log.append_closing(&event)
            }
        })
        .await;
        match appended {
            Ok(Ok(())) => Ok(()),
            // A full queue records nothing but leaves audit healthy.
            Ok(Err(OdxError::Capacity)) => Err(OdxError::Capacity),
            _ => {
                self.inner.lock().await.audit_failed = true;
                Err(OdxError::Persistence)
            }
        }
    }
    pub(crate) async fn assert_environment_current(
        &self,
        id: &str,
        revision: u64,
        session: u64,
    ) -> Result<()> {
        let s = self.inner.lock().await;
        if s.audit_failed {
            return Err(OdxError::Persistence);
        }
        if s.session != session {
            return Err(OdxError::Locked);
        }
        let v = s.vault()?;
        let env = v.data().environment(id)?;
        if env.revision != revision {
            return Err(OdxError::ApprovalUnavailable);
        }
        Ok(())
    }
}

#[cfg(test)]
mod error_contract_tests {
    use super::AuditDetails;

    #[test]
    fn structured_audit_errors_round_trip_with_legacy_defaults() {
        let details = AuditDetails {
            error_code: Some("upstream.uncertain_result".into()),
            error_params: Some(serde_json::json!({})),
            ..Default::default()
        };
        let json = serde_json::to_value(&details).unwrap();
        let decoded: AuditDetails = serde_json::from_value(json).unwrap();
        assert_eq!(
            decoded.error_code.as_deref(),
            Some("upstream.uncertain_result")
        );
        assert_eq!(decoded.error_params, Some(serde_json::json!({})));
        let old: AuditDetails = serde_json::from_value(serde_json::json!({})).unwrap();
        assert!(old.error_code.is_none());
    }
}
struct PreviewWriter {
    bytes: Vec<u8>,
    budget: usize,
}
impl Write for PreviewWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        for &b in bytes {
            let cost = match b {
                b'"' | b'\\' => 2,
                0..=31 => 6,
                _ => 1,
            };
            if cost > self.budget {
                return Err(std::io::Error::other("preview limit"));
            }
            self.budget -= cost;
            self.bytes.push(b);
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn sensitive_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase().replace('-', "_");
    [
        "password",
        "passwd",
        "secret",
        "token",
        "api_key",
        "apikey",
        "authorization",
        "cookie",
        "private_key",
        "session_id",
    ]
    .iter()
    .any(|part| key.contains(part))
}
fn redact_text(text: &str, env: &Environment) -> String {
    let mut clean = env.redact(text);
    // Local tokens have an odx_ prefix and 64 hex digits.
    let mut offset = 0;
    while let Some(start) = clean[offset..].find("odx_").map(|i| i + offset) {
        let end = start + 68;
        if clean
            .as_bytes()
            .get(start + 4..end)
            .is_some_and(|s| s.iter().all(u8::is_ascii_hexdigit))
        {
            clean.replace_range(start..end, "[redacted]");
            offset = start + 10;
        } else {
            offset = start + 4;
        }
    }
    clean
}
struct Redacted<'a> {
    value: &'a Value,
    env: &'a Environment,
}
impl Serialize for Redacted<'_> {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        match self.value {
            Value::String(text) => {
                // Only the start can fit in an event: redact that, never a whole 48 MiB download.
                // The margin keeps a secret or an odx_ token that starts inside it whole.
                let longest = self.env.redaction_secrets().first().map_or(0, |s| s.len());
                let keep = text
                    .char_indices()
                    .nth(MAX_EVENT + longest + 68)
                    .map_or(text.len(), |(i, _)| i);
                serializer.serialize_str(&redact_text(&text[..keep], self.env))
            }
            Value::Array(values) => {
                let mut seq = serializer.serialize_seq(Some(values.len()))?;
                for value in values {
                    seq.serialize_element(&Redacted {
                        value,
                        env: self.env,
                    })?;
                }
                seq.end()
            }
            Value::Object(values) => {
                let mut map = serializer.serialize_map(Some(values.len()))?;
                for (key, value) in values {
                    let name = redact_text(key, self.env);
                    if sensitive_key(key) {
                        map.serialize_entry(&name, "[redacted]")?;
                    } else {
                        map.serialize_entry(
                            &name,
                            &Redacted {
                                value,
                                env: self.env,
                            },
                        )?;
                    }
                }
                map.end()
            }
            value => value.serialize(serializer),
        }
    }
}

/// Optional activity_date filters; the cursor bounds the index scan.
/// List columns only: details stay encrypted in audit_events until a request is opened.
const PAGE_SQL: &str = "SELECT at,activity_key,environment_id,client_id,operation,outcome FROM activity
                WHERE (at,activity_key)<(?8,?9)
                AND (?1 IS NULL OR environment_id=?1) AND (?2 IS NULL OR client_id=?2)
                AND (?3=0 OR write=1) AND (?4=0 OR outcome IN ('failed','uncertain','rejected','cancelled'))
                AND (?5='' OR instr(lower(operation),?5)>0 OR instr(lower(outcome),?5)>0
                    OR environment_id IN (SELECT value FROM json_each(?6)) OR client_id IN (SELECT value FROM json_each(?7)))
                AND (?10 IS NULL OR at>=?10) AND at>=?11
                ORDER BY at DESC,activity_key DESC LIMIT 101";

pub struct AuditLog {
    database: std::sync::Arc<crate::database::Database>,
    queued: AtomicUsize,
    /// Current retention limits in days and content bytes.
    retention_days: std::sync::atomic::AtomicU32,
    retention_bytes: std::sync::atomic::AtomicU64,
    inserts: std::sync::atomic::AtomicU32,
}
/// Decrements a queue counter on drop.
pub(crate) struct QueueSlot<'a>(pub(crate) &'a AtomicUsize);
impl Drop for QueueSlot<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}
impl AuditLog {
    pub(crate) fn new(database: std::sync::Arc<crate::database::Database>) -> Self {
        Self {
            database,
            queued: AtomicUsize::new(0),
            retention_days: std::sync::atomic::AtomicU32::new(30),
            retention_bytes: std::sync::atomic::AtomicU64::new(MAX_HISTORY),
            inserts: std::sync::atomic::AtomicU32::new(0),
        }
    }
    pub fn for_vault(vault: &crate::vault::Vault) -> Self {
        Self::new(vault.database.clone())
    }
    /// Queued writes; tests fill this to simulate saturation.
    #[cfg(test)]
    pub(crate) fn queued(&self) -> &AtomicUsize {
        &self.queued
    }
    /// Set retention for the next write or `prune_now`.
    pub(crate) fn set_retention(&self, days: u32, mib: u32) {
        self.retention_days.store(days, Ordering::Release);
        self.retention_bytes.store(
            (u64::from(mib) * 1024 * 1024).min(MAX_HISTORY),
            Ordering::Release,
        );
    }
    fn prune(&self, tx: &rusqlite::Transaction<'_>) -> Result<()> {
        prune_retention(
            tx,
            i64::from(self.retention_days.load(Ordering::Acquire).min(30)),
            self.retention_bytes.load(Ordering::Acquire) as i64,
        )
    }
    /// Apply retention immediately after reducing it in Settings.
    /// Prune now and give the freed pages back to the file system (the database uses auto_vacuum=INCREMENTAL).
    pub(crate) fn prune_now(&self) -> Result<()> {
        self.database.transaction(|tx| self.prune(tx))?;
        // Each step frees one page: run it to the end (execute_batch would step it once).
        let conn = self.database.lock()?;
        let mut vacuum = conn.prepare("PRAGMA incremental_vacuum")?;
        let mut rows = vacuum.query([])?;
        while rows.next()?.is_some() {}
        Ok(())
    }
    pub fn append(&self, event: &AuditEvent) -> Result<()> {
        self.queued
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |queued| {
                (queued < MAX_QUEUED_EVENTS).then_some(queued + 1)
            })
            .map_err(|_| OdxError::Capacity)?;
        self.write(event)
    }
    /// Closing a recorded request is bounded by admission, not queue capacity.
    pub(crate) fn append_closing(&self, event: &AuditEvent) -> Result<()> {
        self.queued.fetch_add(1, Ordering::AcqRel);
        self.write(event)
    }
    fn write(&self, event: &AuditEvent) -> Result<()> {
        let _slot = QueueSlot(&self.queued);
        let prune = self
            .inserts
            .fetch_add(1, Ordering::Relaxed)
            .is_multiple_of(PRUNE_EVERY);
        self.database.transaction(|tx| {
            insert_event(tx, event)?;
            if prune {
                self.prune(tx)?;
            }
            Ok(())
        })
    }
    pub(crate) fn last_used(&self) -> Result<Vec<(String, chrono::DateTime<Utc>)>> {
        let conn = self.database.lock()?;
        let mut statement = conn.prepare("SELECT client_id,MAX(at) FROM audit_events WHERE client_id IS NOT NULL GROUP BY client_id")?;
        let rows =
            statement.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
        rows.map(|row| {
            let (id, at) = row?;
            Ok((
                id,
                chrono::DateTime::from_timestamp_micros(at).ok_or(OdxError::Persistence)?,
            ))
        })
        .collect()
    }
    /// Up to 100 requests, newest first, after `cursor`. Read audit_events only for returned rows.
    pub fn page(
        &self,
        cursor: Option<&crate::activity::ActivityCursor>,
        filter: &crate::activity::ActivityFilter,
        envs: &std::collections::HashMap<String, String>,
        clients: &std::collections::HashMap<String, String>,
    ) -> Result<(Vec<AuditEvent>, Option<crate::activity::ActivityCursor>)> {
        let q = filter.q.as_deref().unwrap_or("").to_lowercase();
        let matching_envs: Vec<_> = envs
            .iter()
            .filter(|(_, name)| name.to_lowercase().contains(&q))
            .map(|(id, _)| id)
            .collect();
        let matching_clients: Vec<_> = clients
            .iter()
            .filter(|(_, name)| name.to_lowercase().contains(&q))
            .map(|(id, _)| id)
            .collect();
        // Without a cursor, scan activity_date from the newest entry.
        let (at, key) = cursor.map_or((i64::MAX, ""), |c| (c.at, c.key.as_str()));
        let conn = self.database.lock()?;
        let mut statement = conn.prepare(PAGE_SQL)?;
        let rows = statement.query_map(
            rusqlite::params![
                filter.environment_id,
                filter.client_id,
                filter.writes_only,
                filter.errors_only,
                q,
                serde_json::to_string(&matching_envs)?,
                serde_json::to_string(&matching_clients)?,
                at,
                key,
                filter.since.map(|since| since.timestamp_micros()),
                // Retention applies to what is shown even between periodic prunes.
                (Utc::now()
                    - chrono::Duration::days(i64::from(
                        self.retention_days.load(Ordering::Acquire)
                    )))
                .timestamp_micros(),
            ],
            |r| {
                let at: i64 = r.get(0)?;
                let key: String = r.get(1)?;
                let event = AuditEvent {
                    request_id: key.strip_prefix("request:").map(String::from),
                    details: None,
                    at: DateTime::from_timestamp_micros(at).unwrap_or_default(),
                    environment_id: r.get(2)?,
                    client_id: r.get(3)?,
                    operation: r.get(4)?,
                    outcome: r.get(5)?,
                };
                Ok((at, key, event))
            },
        )?;
        let mut rows = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        // Row 101 signals continuation; the cursor points to the last returned row.
        let next = (rows.len() > 100).then(|| {
            rows.truncate(100);
            let (at, key, _) = &rows[99];
            crate::activity::ActivityCursor {
                at: *at,
                key: key.clone(),
            }
        });
        Ok((rows.into_iter().map(|(_, _, event)| event).collect(), next))
    }
}

impl AuditLog {
    /// Details of the latest event of one request.
    pub fn details(&self, request_id: &str) -> Result<Option<AuditDetails>> {
        use rusqlite::OptionalExtension;
        let conn = self.database.lock()?;
        let payload: Option<Vec<u8>> = conn
            .query_row(
                "SELECT e.payload FROM activity a JOIN audit_events e ON e.seq=a.last_seq WHERE a.activity_key=?1",
                [format!("request:{request_id}")],
                |r| r.get(0),
            )
            .optional()?;
        Ok(match payload {
            Some(payload) => serde_json::from_slice::<AuditEvent>(&payload)?.details,
            None => None,
        })
    }
}

pub(crate) fn insert_event(tx: &rusqlite::Transaction<'_>, event: &AuditEvent) -> Result<()> {
    let clear = zeroize::Zeroizing::new(serde_json::to_vec(event)?);
    if clear.len() > MAX_EVENT {
        return Err(OdxError::Persistence);
    }
    let activity_key = event
        .request_id
        .as_ref()
        .map(|id| format!("request:{id}"))
        .unwrap_or_else(|| format!("legacy:{}", uuid::Uuid::new_v4()));
    let at = event.at.timestamp_micros();
    let bytes = clear.len() as i64;
    tx.execute(
        "INSERT INTO audit_events(activity_key,at,client_id,payload,bytes) VALUES(?1,?2,?3,?4,?5)",
        rusqlite::params![activity_key, at, event.client_id, clear.as_slice(), bytes],
    )?;
    let seq = tx.last_insert_rowid();
    let terminal = !matches!(event.outcome.as_str(), "pending" | "attempted");
    tx.execute("INSERT INTO activity(activity_key,at,environment_id,client_id,operation,outcome,write,terminal,last_seq)
        VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)
        ON CONFLICT(activity_key) DO UPDATE SET at=excluded.at,environment_id=excluded.environment_id,
            client_id=excluded.client_id,operation=excluded.operation,outcome=excluded.outcome,write=excluded.write,
            terminal=excluded.terminal,last_seq=excluded.last_seq
        WHERE excluded.terminal>=activity.terminal AND (excluded.at>=activity.at OR excluded.terminal>activity.terminal)",
        rusqlite::params![activity_key,at,event.environment_id,event.client_id,event.operation,event.outcome,
            crate::activity::is_write(&event.operation),terminal,seq])?;
    Ok(())
}
pub(crate) fn prune(tx: &rusqlite::Transaction<'_>) -> Result<()> {
    prune_to(tx, MAX_HISTORY as i64)
}
/// Retain 30 days and `max_history` content bytes; tests use a smaller limit.
pub(crate) fn prune_to(tx: &rusqlite::Transaction<'_>, max_history: i64) -> Result<()> {
    prune_retention(tx, 30, max_history)
}
/// Retain `days` and `max_history` content bytes, plus the page limit.
fn prune_retention(tx: &rusqlite::Transaction<'_>, days: i64, max_history: i64) -> Result<()> {
    let cutoff = (Utc::now() - chrono::Duration::days(days)).timestamp_micros();
    tx.execute("DELETE FROM audit_events WHERE activity_key IN (SELECT activity_key FROM activity WHERE at<?1)", [cutoff])?;
    tx.execute("DELETE FROM activity WHERE at<?1", [cutoff])?;
    loop {
        let bytes: i64 =
            tx.query_row("SELECT history_bytes FROM metadata WHERE id=1", [], |r| {
                r.get(0)
            })?;
        let pages: i64 = tx.query_row("PRAGMA page_count", [], |r| r.get(0))?;
        let free: i64 = tx.query_row("PRAGMA freelist_count", [], |r| r.get(0))?;
        // Reserve space for indexes, configuration and the next transaction.
        if bytes <= max_history && pages - free < 80_000 {
            break;
        }
        tx.execute("DELETE FROM audit_events WHERE activity_key IN (SELECT activity_key FROM activity ORDER BY at LIMIT 64)", [])?;
        let removed = tx.execute("DELETE FROM activity WHERE activity_key IN (SELECT activity_key FROM activity ORDER BY at LIMIT 64)", [])?;
        if removed == 0 {
            return Err(OdxError::Persistence);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retention_setting_prunes_older_activity_now() {
        let dir = tempfile::tempdir().unwrap();
        let vault =
            crate::vault::Vault::create(dir.path().join("vault.sqlite3"), "fictional password")
                .unwrap();
        let audit = AuditLog::for_vault(&vault);
        let event = |id: &str, days: i64| AuditEvent {
            request_id: Some(id.into()),
            details: None,
            at: Utc::now() - chrono::Duration::days(days),
            client_id: None,
            environment_id: "fixture".into(),
            operation: "read:res.partner".into(),
            outcome: "succeeded".into(),
        };
        audit.append(&event("old", 3)).unwrap();
        audit.append(&event("new", 0)).unwrap();
        let count = || -> i64 {
            audit
                .database
                .lock()
                .unwrap()
                .query_row("SELECT count(*) FROM activity", [], |r| r.get(0))
                .unwrap()
        };
        assert_eq!(count(), 2, "default 30-day retention");
        audit.set_retention(2, 256);
        audit.prune_now().unwrap();
        assert_eq!(count(), 1, "reduced retention applies immediately");
        // Out-of-range values cannot exceed the 30-day limit.
        audit.set_retention(400, 256);
        audit.append(&event("ancient", 31)).unwrap();
        assert_eq!(count(), 2, "inserts between periodic prunes skip retention");
        for _ in 3..PRUNE_EVERY {
            audit.append(&event("new", 0)).unwrap();
        }
        assert_eq!(count(), 2);
        audit.append(&event("new", 0)).unwrap();
        assert_eq!(count(), 1, "periodic prune keeps the 30-day limit");
    }
    #[test]
    fn sql_projection_groups_requests_before_filters_and_pagination() {
        let dir = tempfile::tempdir().unwrap();
        let vault =
            crate::vault::Vault::create(dir.path().join("vault.sqlite3"), "fictional password")
                .unwrap();
        let audit = AuditLog::for_vault(&vault);
        let mut event = AuditEvent {
            request_id: Some("a".into()),
            details: None,
            at: Utc::now(),
            client_id: None,
            environment_id: "fixture".into(),
            operation: "write:res.partner".into(),
            outcome: "pending".into(),
        };
        audit.append(&event).unwrap();
        event.outcome = "attempted".into();
        audit.append(&event).unwrap();
        event.outcome = "failed".into();
        audit.append(&event).unwrap();
        event.outcome = "pending".into();
        event.at = Utc::now();
        audit.append(&event).unwrap();
        let envs = std::collections::HashMap::from([("fixture".into(), "Test environment".into())]);
        let clients = std::collections::HashMap::new();
        let filter = crate::activity::ActivityFilter {
            errors_only: true,
            q: Some("test".into()),
            ..Default::default()
        };
        let (page, next) = audit.page(None, &filter, &envs, &clients).unwrap();
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].outcome, "failed");
        assert!(next.is_none());
        for i in 0..101 {
            event.request_id = Some(format!("parallel-{i}"));
            event.outcome = "succeeded".into();
            audit.append(&event).unwrap();
        }
        let (page, next) = audit
            .page(None, &Default::default(), &envs, &clients)
            .unwrap();
        assert_eq!(page.len(), 100);
        let (page, next) = audit
            .page(next.as_ref(), &Default::default(), &envs, &clients)
            .unwrap();
        assert_eq!(page.len(), 2);
        assert!(next.is_none());
        let since = crate::activity::ActivityFilter {
            since: Some(Utc::now() + chrono::Duration::seconds(1)),
            ..Default::default()
        };
        let (page, _) = audit.page(None, &since, &envs, &clients).unwrap();
        assert!(page.is_empty(), "since excludes older requests");
        let events = || -> i64 {
            audit
                .database
                .lock()
                .unwrap()
                .query_row("SELECT count(*) FROM audit_events", [], |r| r.get(0))
                .unwrap()
        };
        assert_eq!(events(), 105, "audit events are preserved");
        event.request_id = Some("expired".into());
        event.at = Utc::now() - chrono::Duration::days(31);
        audit.append(&event).unwrap();
        audit.prune_now().unwrap();
        assert_eq!(events(), 105, "retention removes expired requests");
    }

    fn fixture_event(id: String, at: DateTime<Utc>) -> AuditEvent {
        AuditEvent {
            request_id: Some(id),
            details: Some(AuditDetails {
                request: Some(AuditPayload {
                    text: "fixture request".into(),
                    truncated: false,
                }),
                ..Default::default()
            }),
            at,
            client_id: None,
            environment_id: "fixture".into(),
            operation: "read:res.partner".into(),
            outcome: "succeeded".into(),
        }
    }

    #[test]
    fn prune_now_gives_every_freed_page_back() {
        let dir = tempfile::tempdir().unwrap();
        let vault =
            crate::vault::Vault::create(dir.path().join("vault.sqlite3"), "fictional password")
                .unwrap();
        let audit = AuditLog::for_vault(&vault);
        let old = Utc::now() - chrono::Duration::days(10);
        for i in 0..200 {
            let mut event = fixture_event(format!("old-{i}"), old);
            event.details.as_mut().unwrap().response = Some(AuditPayload {
                text: "x".repeat(8000),
                truncated: false,
            });
            audit.append(&event).unwrap();
        }
        audit.set_retention(1, 256);
        audit.prune_now().unwrap();
        let free: i64 = audit
            .database
            .lock()
            .unwrap()
            .query_row("PRAGMA freelist_count", [], |r| r.get(0))
            .unwrap();
        assert_eq!(free, 0);
    }

    #[test]
    fn the_list_hides_requests_past_retention_before_the_next_prune() {
        let dir = tempfile::tempdir().unwrap();
        let vault =
            crate::vault::Vault::create(dir.path().join("vault.sqlite3"), "fictional password")
                .unwrap();
        let audit = AuditLog::for_vault(&vault);
        let old = Utc::now() - chrono::Duration::days(3);
        audit.append(&fixture_event("old".into(), old)).unwrap();
        audit
            .append(&fixture_event("new".into(), Utc::now()))
            .unwrap();
        audit.set_retention(2, 256);
        let none = std::collections::HashMap::new();
        let (page, _) = audit.page(None, &Default::default(), &none, &none).unwrap();
        let ids: Vec<_> = page
            .iter()
            .map(|e| e.request_id.as_deref().unwrap())
            .collect();
        assert_eq!(ids, ["new"]);
    }

    #[test]
    fn details_are_read_per_request_from_its_latest_event() {
        let dir = tempfile::tempdir().unwrap();
        let vault =
            crate::vault::Vault::create(dir.path().join("vault.sqlite3"), "fictional password")
                .unwrap();
        let audit = AuditLog::for_vault(&vault);
        let mut event = fixture_event("one".into(), Utc::now());
        audit.append(&event).unwrap();
        event.details.as_mut().unwrap().response = Some(AuditPayload {
            text: "fixture response".into(),
            truncated: false,
        });
        audit.append(&event).unwrap();
        let details = audit.details("one").unwrap().unwrap();
        assert_eq!(details.request.unwrap().text, "fixture request");
        assert_eq!(details.response.unwrap().text, "fixture response");
        assert!(audit.details("missing").unwrap().is_none());
    }

    #[test]
    fn cursor_pages_do_not_repeat_keys_when_new_events_arrive() {
        let dir = tempfile::tempdir().unwrap();
        let vault =
            crate::vault::Vault::create(dir.path().join("vault.sqlite3"), "fictional password")
                .unwrap();
        let audit = AuditLog::for_vault(&vault);
        let base = Utc::now() - chrono::Duration::hours(1);
        // Two requests per timestamp exercise the key tie-breaker.
        for i in 0..130 {
            let at = base + chrono::Duration::seconds(i / 2);
            audit
                .append(&fixture_event(format!("old-{i:03}"), at))
                .unwrap();
        }
        let none = std::collections::HashMap::new();
        let (first, next) = audit.page(None, &Default::default(), &none, &none).unwrap();
        assert_eq!(first.len(), 100);
        // The list carries no details; they are read per request.
        assert!(first[0].details.is_none());
        let id = first[0].request_id.as_deref().unwrap();
        assert_eq!(
            audit.details(id).unwrap().unwrap().request.unwrap().text,
            "fixture request"
        );
        for i in 0..30 {
            audit
                .append(&fixture_event(format!("new-{i:02}"), Utc::now()))
                .unwrap();
        }
        let (second, next) = audit
            .page(next.as_ref(), &Default::default(), &none, &none)
            .unwrap();
        assert!(next.is_none());
        let mut keys: Vec<_> = first
            .iter()
            .chain(&second)
            .map(|e| e.request_id.clone().unwrap())
            .collect();
        assert_eq!(keys.len(), 130);
        assert!(keys.iter().all(|k| k.starts_with("old-")));
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), 130, "no duplicate or missing keys");
    }

    #[test]
    fn size_pruning_keeps_history_under_a_reduced_limit_and_appends_afterwards() {
        let dir = tempfile::tempdir().unwrap();
        let vault =
            crate::vault::Vault::create(dir.path().join("vault.sqlite3"), "fictional password")
                .unwrap();
        const LIMIT: i64 = 64 * 1024;
        let bytes = || -> i64 {
            vault
                .database
                .lock()
                .unwrap()
                .query_row("SELECT history_bytes FROM metadata", [], |r| r.get(0))
                .unwrap()
        };
        let append = |i: usize| {
            vault
                .database
                .transaction(|tx| {
                    let mut event = fixture_event(format!("prune-{i}"), Utc::now());
                    event.details.as_mut().unwrap().response = Some(AuditPayload {
                        text: "x".repeat(2000),
                        truncated: false,
                    });
                    insert_event(tx, &event)?;
                    prune_to(tx, LIMIT)
                })
                .unwrap()
        };
        for i in 0..100 {
            append(i);
            assert!(bytes() <= LIMIT);
        }
        let rows: i64 = vault
            .database
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM activity", [], |r| r.get(0))
            .unwrap();
        assert!(rows < 100, "old requests were pruned");
        append(100);
        let none = std::collections::HashMap::new();
        let (page, _) = AuditLog::for_vault(&vault)
            .page(None, &Default::default(), &none, &none)
            .unwrap();
        assert_eq!(page[0].request_id.as_deref(), Some("prune-100"));
        assert!(bytes() <= LIMIT);
    }

    #[test]
    fn redaction_hides_sensitive_header_values_and_the_secret_but_not_visible_headers() {
        let header = |name: &str, value: &str| crate::headers::HeaderEntry {
            name: name.into(),
            value: value.into(),
            enabled: true,
        };
        let env = Environment {
            secret: crate::security::Secret::new("fixture-odoo-secret").unwrap(),
            headers: vec![
                header("Accept-Language", "es_ES"),
                header("X-Custom-Gate", "fixture-gate-value"),
            ],
            ..Default::default()
        };
        let text = redact_text(
            "lang es_ES gate fixture-gate-value key fixture-odoo-secret",
            &env,
        );
        assert_eq!(text, "lang es_ES gate [redacted] key [redacted]");
    }

    #[test]
    fn payload_redacts_nested_credentials_before_truncation() {
        let env = Environment {
            secret: crate::security::Secret::new("fixture-odoo-secret").unwrap(),
            headers: vec![crate::headers::HeaderEntry {
                name: "Authorization".into(),
                value: "Bearer fixture-header-secret".into(),
                enabled: true,
            }],
            ..Default::default()
        };
        let token = format!("odx_{}", "a".repeat(64));
        let value = serde_json::json!({
            "nested": [{"password": "fixture-password", "API-Key": "fixture-key"}],
            "message": format!("fixture-odoo-secret Bearer fixture-header-secret {token}"),
            "normal": [false, 3, null]
        });
        let preview = AuditPayload::capture(&value, &env, REQUEST_BUDGET);
        assert!(!preview.truncated);
        for secret in [
            "fixture-password",
            "fixture-key",
            "fixture-odoo-secret",
            "fixture-header-secret",
            &token,
        ] {
            assert!(!preview.text.contains(secret));
        }
        assert_eq!(
            serde_json::from_str::<Value>(&preview.text).unwrap()["normal"],
            serde_json::json!([false, 3, null])
        );
        let large = serde_json::json!({"data": "ñ\n\"".repeat(20_000)});
        let capture = |budget| {
            let preview = AuditPayload::capture(&large, &env, budget);
            assert!(preview.truncated);
            assert!(serde_json::to_vec(&preview).unwrap().len() < budget + 96);
            preview
        };
        let event = AuditEvent {
            request_id: Some("fixture-request".into()),
            client_id: None,
            at: Utc::now(),
            environment_id: "fixture".into(),
            operation: "read:res.partner".into(),
            outcome: "succeeded".into(),
            details: Some(AuditDetails {
                request: Some(capture(REQUEST_BUDGET)),
                response: Some(capture(RESPONSE_BUDGET)),
                error: Some(capture(ERROR_BUDGET)),
                ..Default::default()
            }),
        };
        assert!(serde_json::to_vec(&event).unwrap().len() < MAX_EVENT);
    }
}

/// Manual benchmark: `ODX_BENCH_DIR=<real-disk> cargo test --release -p odx-core --lib audit::bench -- --ignored --nocapture`. Creates ~240 MiB of synthetic history.
#[cfg(test)]
mod bench {
    use super::*;
    use std::time::Instant;

    #[test]
    #[ignore]
    fn writes_filter_on_240_mib_history() {
        let dir = std::path::PathBuf::from(std::env::var("ODX_BENCH_DIR").unwrap());
        let path = dir.join("vault.sqlite3");
        let _ = std::fs::remove_file(&path);
        let vault = crate::vault::Vault::create(&path, "fixture bench password").unwrap();
        let text = |n: usize| AuditPayload {
            text: "x".repeat(n),
            truncated: false,
        };
        let start = Utc::now() - chrono::Duration::days(29);
        let (mut generated, mut requests) = (0usize, 0usize);
        let started = Instant::now();
        while generated < 240 * 1024 * 1024 {
            vault
                .database
                .transaction(|tx| {
                    for _ in 0..250 {
                        // One write per 50 requests; the rest are reads.
                        let method = if requests % 50 == 0 {
                            "write"
                        } else {
                            "search_read"
                        };
                        let mut event = AuditEvent {
                            request_id: Some(format!("bench-{requests}")),
                            details: Some(AuditDetails {
                                request: Some(text(1000)),
                                ..Default::default()
                            }),
                            at: start + chrono::Duration::milliseconds(requests as i64 * 80_000),
                            client_id: Some("bench-client".into()),
                            environment_id: format!("env-{}", requests % 4),
                            operation: format!("{method}:res.partner"),
                            outcome: "attempted".into(),
                        };
                        insert_event(tx, &event)?;
                        event.outcome = "succeeded".into();
                        event.details.as_mut().unwrap().response = Some(text(6000));
                        insert_event(tx, &event)?;
                        generated += 8200;
                        requests += 1;
                    }
                    prune(tx)
                })
                .unwrap();
        }
        let history: i64 = vault
            .database
            .lock()
            .unwrap()
            .query_row("SELECT history_bytes FROM metadata", [], |r| r.get(0))
            .unwrap();
        println!(
            "generated {requests} requests in {:?}; history_bytes={} MiB",
            started.elapsed(),
            history / 1024 / 1024
        );
        drop(vault);
        let size: u64 = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().metadata().unwrap().len())
            .sum();
        println!("disk size {} MiB", size / 1024 / 1024);
        let vault = crate::vault::Vault::unlock(&path, "fixture bench password").unwrap();
        let audit = AuditLog::for_vault(&vault);
        let none = std::collections::HashMap::new();
        for (label, filter) in [
            (
                "Escrituras",
                crate::activity::ActivityFilter {
                    writes_only: true,
                    ..Default::default()
                },
            ),
            ("Unfiltered", Default::default()),
            (
                "Environment",
                crate::activity::ActivityFilter {
                    environment_id: Some("env-1".into()),
                    ..Default::default()
                },
            ),
            (
                "Errores",
                crate::activity::ActivityFilter {
                    errors_only: true,
                    ..Default::default()
                },
            ),
        ] {
            let mut times = vec![];
            for _ in 0..11 {
                let t = Instant::now();
                let page = bench_page(&audit, &filter, &none);
                times.push(t.elapsed());
                assert!(page <= 100);
            }
            let first = times[0];
            times.sort();
            println!("{label}: primera {first:?}, mediana {:?}", times[5]);
        }
        let plan = |sql: &str| -> Vec<String> {
            let conn = vault.database.lock().unwrap();
            let mut statement = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
            let rows = statement
                .query_map(
                    rusqlite::params![
                        Option::<String>::None,
                        Option::<String>::None,
                        true,
                        false,
                        "",
                        "[]",
                        "[]",
                        i64::MAX,
                        ""
                    ],
                    |r| r.get::<_, String>(3),
                )
                .unwrap();
            rows.map(|r| r.unwrap()).collect()
        };
        println!("plan v3: {:?}", plan(PAGE_SQL));
        // v2 indexes are unused when optional filters share one query.
        vault
            .database
            .lock()
            .unwrap()
            .execute_batch(
                "CREATE INDEX activity_environment_date ON activity(environment_id,at DESC);
                CREATE INDEX activity_client_date ON activity(client_id,at DESC);
                CREATE INDEX activity_outcome_date ON activity(outcome,at DESC); ANALYZE;",
            )
            .unwrap();
        println!("v2 index plan: {:?}", plan(PAGE_SQL));
        let environment = crate::activity::ActivityFilter {
            environment_id: Some("env-1".into()),
            ..Default::default()
        };
        let t = Instant::now();
        bench_page(&audit, &environment, &none);
        println!("Environment with v2 indexes: {:?}", t.elapsed());
        drop(audit);
        drop(vault);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
#[cfg(test)]
fn bench_page(
    audit: &AuditLog,
    filter: &crate::activity::ActivityFilter,
    none: &std::collections::HashMap<String, String>,
) -> usize {
    audit.page(None, filter, none, none).unwrap().0.len()
}
