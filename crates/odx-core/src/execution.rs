use crate::{
    approval::{Approval, Decision, State},
    audit::{AuditDetails, AuditEvent, AuditLog, AuditRequest},
    clients::{self, Capability, ClientIdentity, ClientView, IssuedToken},
    config::{Bound, Environment, EnvironmentDraft, Settings, WriteMode},
    error::{OdxError, Result},
    limits::Limits,
    odoo::jsonrpc::OdooClient,
    policy::{self, Effect},
    vault::Vault,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};
use tokio::sync::{Mutex, oneshot};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentView {
    pub id: String,
    pub name: String,
    pub url: String,
    pub database: String,
    pub login: String,
    pub protocol: crate::config::Protocol,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detected_protocol: Option<crate::config::Protocol>,
    pub context: Value,
    pub write_mode: WriteMode,
    pub write_until: Option<chrono::DateTime<Utc>>,
    pub has_secret: bool,
    #[serde(default)]
    pub tags: Vec<String>,
}
impl EnvironmentView {
    /// View with the effective mode at the vault expiry clock's `now`.
    pub(crate) fn new(e: &Environment, now: chrono::DateTime<Utc>) -> Self {
        Self {
            id: e.id.clone(),
            name: e.name.clone(),
            url: e.url.clone(),
            database: e.database.clone(),
            login: e.login.clone(),
            protocol: e.protocol,
            detected_protocol: e.detected_protocol,
            context: e.context.clone(),
            write_mode: e.effective_write_mode(now),
            write_until: e.write_until,
            has_secret: !e.secret.is_empty(),
            tags: e.tags.clone(),
        }
    }
}
/// Environment connectivity without business data or secrets.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthReport {
    pub environment_id: String,
    /// `ok`, `unreachable` or `auth_failed`.
    pub status: String,
    pub server_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<crate::config::Protocol>,
    pub latency_ms: u64,
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_params: Option<Value>,
    /// HTTP error status, if Odoo responded.
    pub http_status: Option<u16>,
    /// Network failure kind: timeout, dns, tls, refused, unreachable, reset or network.
    pub network_error: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingApproval {
    pub id: String,
    /// Originating API/MCP client; absent for local administration.
    #[serde(default)]
    pub client_id: Option<String>,
    #[serde(default)]
    pub client_label: Option<String>,
    pub environment_id: String,
    pub environment_name: String,
    pub model: String,
    pub method: String,
    pub args: Value,
    pub kwargs: Value,
    pub context: Value,
    pub created_at: chrono::DateTime<Utc>,
    pub expires_at: chrono::DateTime<Utc>,
}
#[derive(Debug, Clone, Serialize)]
pub struct SettingsView {
    pub settings: Settings,
    pub defaults: Settings,
    pub bounds: std::collections::BTreeMap<&'static str, Bound>,
}
impl SettingsView {
    fn new(settings: Settings) -> Self {
        Self {
            settings,
            defaults: Settings::default(),
            bounds: Settings::bounds(),
        }
    }
}
pub(crate) struct Inner {
    pub(crate) vault: Option<Vault>,
    pub(crate) pending: HashMap<String, Approval>,
    /// Clients awaiting approval decisions.
    pub(crate) waiters: HashMap<String, oneshot::Sender<ApprovalEnd>>,
    pub(crate) audit: Option<Arc<AuditLog>>,
    pub(crate) audit_failed: bool,
    pub(crate) session: u64,
}
/// Odoo calls and received bytes, including login performed by this call, plus the start of `upstream_ms`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Upstream {
    pub(crate) calls: usize,
    pub(crate) bytes: usize,
    pub(crate) started: Instant,
}
impl Upstream {
    pub(crate) fn start() -> Self {
        Self {
            calls: 0,
            bytes: 0,
            started: Instant::now(),
        }
    }
    /// Current `upstream_calls`, `upstream_bytes` and `upstream_ms` metadata.
    pub(crate) fn write_meta(&self, meta: &mut serde_json::Map<String, Value>) {
        meta.insert("upstream_calls".into(), json!(self.calls));
        meta.insert("upstream_bytes".into(), json!(self.bytes));
        meta.insert(
            "upstream_ms".into(),
            json!(self.started.elapsed().as_millis() as u64),
        );
    }
}
/// Final outcome delivered to an approval's waiting client.
#[derive(Debug)]
pub(crate) enum ApprovalEnd {
    Executed(Value, Upstream),
    /// Optional human reason, relayed to the agent.
    Rejected(Option<String>),
    Revoked,
    Expired,
    Cancelled,
    Failed(OdxError),
}
/// Unsent request awaiting outcome persistence.
pub(crate) struct Ended {
    audit: Option<Arc<AuditLog>>,
    approval: Approval,
    outcome: &'static str,
}
impl Inner {
    /// Remove unsent terminal requests and notify waiters: `revoked`, `expired` or `cancelled`.
    pub(crate) fn end(&mut self, ids: Vec<String>, outcome: &'static str) -> Vec<Ended> {
        ids.into_iter()
            .filter_map(|id| {
                let approval = self.pending.remove(&id)?;
                if let Some(tx) = self.waiters.remove(&id) {
                    let _ = tx.send(match outcome {
                        "expired" => ApprovalEnd::Expired,
                        "cancelled" => ApprovalEnd::Cancelled,
                        _ => ApprovalEnd::Revoked,
                    });
                }
                Some(Ended {
                    audit: self.audit.clone(),
                    approval,
                    outcome,
                })
            })
            .collect()
    }
}
/// Revoke unsent requests matching `filter`; return those awaiting a decision.
fn revoke_where(
    pending: &mut HashMap<String, Approval>,
    filter: impl Fn(&Approval) -> bool,
) -> Vec<String> {
    pending
        .iter_mut()
        .filter(|(_, approval)| filter(approval))
        .filter_map(|(id, approval)| approval.revoke().then(|| id.clone()))
        .collect()
}
pub(crate) fn revoke_environment(
    pending: &mut HashMap<String, Approval>,
    environment_id: &str,
) -> Vec<String> {
    revoke_where(pending, |a| a.call.environment_id == environment_id)
}
fn revoke_client(pending: &mut HashMap<String, Approval>, client_id: &str) -> Vec<String> {
    revoke_where(pending, |a| a.client_id.as_deref() == Some(client_id))
}
/// Revalidate authorization immediately before dispatch.
fn authorization_current(
    v: &Vault,
    env: &Environment,
    call: &crate::approval::ApprovalCall,
    authorization: &policy::Authorization,
) -> bool {
    let now = v.now();
    match authorization {
        // Human approval remains valid while the environment permits writes.
        policy::Authorization::Individual => policy::authorize(env, Effect::Write, now).is_ok(),
        policy::Authorization::Free => {
            matches!(policy::authorize(env, Effect::Write, now), Ok(false))
        }
        policy::Authorization::Rule {
            rule_id,
            rule_revision,
        } => {
            let (Some(args), Some(kwargs)) = (call.args.as_array(), call.kwargs.as_object()) else {
                return false;
            };
            matches!(policy::authorize(env, Effect::Write, now), Ok(true))
                && v.data().rules.iter().any(|rule| {
                    rule.id == *rule_id
                        && rule.revision == *rule_revision
                        && rule.matches(
                            &policy::EffectiveCall {
                                environment_id: &env.id,
                                model: &call.model,
                                method: &call.method,
                                args,
                                kwargs,
                            },
                            &Effect::Write,
                            now,
                        )
                })
        }
    }
}
/// Locking or another unlock during the KDF invalidates this vault installation.
fn superseded(s: &Inner, session: u64) -> Result<()> {
    if s.session != session || s.vault.is_some() {
        return Err(OdxError::Locked);
    }
    Ok(())
}
fn revoke_all(pending: &mut HashMap<String, Approval>) -> Vec<String> {
    revoke_where(pending, |_| true)
}
/// New client request notice for desktop notifications.
#[derive(Debug, Clone, Serialize)]
pub struct ApprovalNotice {
    pub approval_id: String,
    pub client_label: Option<String>,
    pub environment_name: String,
    pub model: String,
    pub method: String,
}

/// In-memory last use and open MCP connections.
#[derive(Default)]
pub(crate) struct Usage {
    last_used: HashMap<String, chrono::DateTime<Utc>>,
    /// Vault session whose history initialized `last_used`.
    seeded_session: Option<u64>,
    connections: HashMap<u64, (String, chrono::DateTime<Utc>)>,
    next_connection: u64,
}

/// Open MCP connection shown in the app.
#[derive(Debug, Clone, Serialize)]
pub struct McpConnectionInfo {
    pub client_id: String,
    pub client_label: Option<String>,
    pub since: chrono::DateTime<Utc>,
}

/// Count the connection as open while this guard exists.
pub struct ConnectionGuard {
    core: Arc<Core>,
    id: u64,
}
impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        if let Ok(mut usage) = self.core.usage.lock() {
            usage.connections.remove(&self.id);
        }
    }
}

pub struct Core {
    path: PathBuf,
    pub(crate) usage: std::sync::Mutex<Usage>,
    pub(crate) notices: tokio::sync::broadcast::Sender<ApprovalNotice>,
    /// Client whose capabilities were saved, for `list_changed` notices.
    pub(crate) grant_changes: tokio::sync::broadcast::Sender<String>,
    pub(crate) inner: Mutex<Inner>,
    pub(crate) odoo: OdooClient,
    pub(crate) limits: Limits,
}
impl Core {
    pub fn new(path: impl AsRef<Path>) -> Result<Arc<Self>> {
        Ok(Arc::new(Self {
            path: Vault::database_path(path.as_ref()),
            notices: tokio::sync::broadcast::channel(32).0,
            grant_changes: tokio::sync::broadcast::channel(32).0,
            usage: std::sync::Mutex::new(Usage::default()),
            inner: Mutex::new(Inner {
                vault: None,
                pending: HashMap::new(),
                waiters: HashMap::new(),
                audit: None,
                audit_failed: false,
                session: 0,
            }),
            odoo: OdooClient::new()?,
            limits: Limits::default(),
        }))
    }
    /// Managed `download`/`to_file` directory beside the vault.
    pub(crate) fn outputs_dir(&self) -> PathBuf {
        self.path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("outputs")
    }
    /// Slow notice receivers lose old notices without blocking producers.
    pub fn approval_notices(&self) -> tokio::sync::broadcast::Receiver<ApprovalNotice> {
        self.notices.subscribe()
    }
    pub async fn status(&self) -> &'static str {
        if Vault::exists(&self.path) {
            if self.inner.lock().await.vault.is_some() {
                "unlocked"
            } else {
                "locked"
            }
        } else {
            "absent"
        }
    }
    pub async fn create(&self, password: &str) -> Result<()> {
        self.odoo.clear_auth().await;
        let session = {
            let s = self.inner.lock().await;
            if s.vault.is_some() {
                return Err(OdxError::validation(
                    "validation.vault.already_unlocked",
                    "vault already unlocked",
                ));
            }
            s.session
        };
        // Run KDF and fsync off worker threads without holding shared state.
        let (path, password) = (
            self.path.clone(),
            zeroize::Zeroizing::new(password.to_owned()),
        );
        let vault = tokio::task::spawn_blocking(move || Vault::create(&path, &password))
            .await
            .map_err(|_| OdxError::Persistence)??;
        let mut s = self.inner.lock().await;
        superseded(&s, session)?;
        s.audit = Some(Arc::new(AuditLog::new(vault.database.clone())));
        s.audit_failed = false;
        s.session = s.session.wrapping_add(1);
        let settings = vault.data().settings;
        s.vault = Some(vault);
        s.pending.clear();
        self.apply_settings(settings, s.audit.as_deref());
        Ok(())
    }
    /// Set a locked vault aside (see `Vault::reset`) so the user can start over after forgetting the password.
    pub async fn reset(&self) -> Result<()> {
        let mut s = self.inner.lock().await;
        if s.vault.is_some() {
            return Err(OdxError::validation(
                "validation.vault.already_unlocked",
                "vault already unlocked",
            ));
        }
        // Supersede an unlock still running its KDF on the old files.
        s.session = s.session.wrapping_add(1);
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || Vault::reset(&path))
            .await
            .map_err(|_| OdxError::Persistence)?
    }
    pub async fn unlock(&self, password: &str) -> Result<()> {
        self.odoo.clear_auth().await;
        let session = self.inner.lock().await.session;
        // Run KDF, migration and pruning off worker threads without holding shared state.
        let (path, password) = (
            self.path.clone(),
            zeroize::Zeroizing::new(password.to_owned()),
        );
        let vault = tokio::task::spawn_blocking(move || Vault::unlock(&path, &password))
            .await
            .map_err(|_| OdxError::Persistence)??;
        let mut s = self.inner.lock().await;
        superseded(&s, session)?;
        s.audit = Some(Arc::new(AuditLog::new(vault.database.clone())));
        s.audit_failed = false;
        s.session = s.session.wrapping_add(1);
        s.pending.clear();
        let settings = vault.data().settings;
        s.vault = Some(vault);
        self.apply_settings(settings, s.audit.as_deref());
        drop(s);
        self.sweep_outputs_in_background(settings);
        Ok(())
    }
    /// Settings with defaults and allowed ranges.
    pub async fn settings(&self) -> Result<SettingsView> {
        let s = self.inner.lock().await;
        let v = s.vault.as_ref().ok_or(OdxError::Locked)?;
        Ok(SettingsView::new(v.data().settings))
    }
    /// Validate, encrypt and apply without restart. Preserve pending approval deadlines; reduce concurrency as calls finish and prune excess history/output immediately.
    pub async fn save_settings(&self, settings: Settings) -> Result<SettingsView> {
        settings.validate()?;
        let mut s = self.inner.lock().await;
        let v = s.vault.as_mut().ok_or(OdxError::Locked)?;
        v.update(|data| {
            data.settings = settings;
            Ok(())
        })?;
        let audit = s.audit.clone();
        self.apply_settings(settings, audit.as_deref());
        drop(s);
        if let Some(audit) = audit {
            tokio::task::spawn_blocking(move || audit.prune_now())
                .await
                .map_err(|_| OdxError::Persistence)??;
        }
        self.sweep_outputs_in_background(settings);
        Ok(SettingsView::new(settings))
    }
    fn apply_settings(&self, settings: Settings, audit: Option<&AuditLog>) {
        self.odoo.set_timeout(settings.upstream_timeout_secs);
        if let Some(audit) = audit {
            audit.set_retention(settings.activity_days, settings.activity_mib);
        }
        tokio::spawn(self.limits.set_concurrency(
            settings.global_concurrency as usize,
            settings.environment_concurrency as usize,
        ));
    }
    pub async fn lock(&self) {
        self.odoo.clear_auth().await;
        self.odoo.clear_clients();
        let mut s = self.inner.lock().await;
        let ids = revoke_all(&mut s.pending);
        let ended = s.end(ids, "revoked");
        s.audit = None;
        s.vault = None;
        s.session = s.session.wrapping_add(1);
        drop(s);
        self.record(ended).await;
    }
    /// Drain admitted operations, including sent calls, up to `within` without cancelling them. Return false on timeout.
    pub async fn drain(&self, within: std::time::Duration) -> bool {
        self.limits.wait_idle(within).await
    }
    pub async fn change_password(&self, current: &str, new: &str) -> Result<()> {
        let database = {
            let s = self.inner.lock().await;
            s.vault.as_ref().ok_or(OdxError::Locked)?.database.clone()
        };
        // Rekey rewrites the database; the connection mutex serializes access.
        let (current, new) = (
            zeroize::Zeroizing::new(current.to_owned()),
            zeroize::Zeroizing::new(new.to_owned()),
        );
        tokio::task::spawn_blocking(move || database.rekey(&current, &new))
            .await
            .map_err(|_| OdxError::Persistence)?
    }
    pub async fn environments(&self) -> Result<Vec<EnvironmentView>> {
        let s = self.inner.lock().await;
        let v = s.vault.as_ref().ok_or(OdxError::Locked)?;
        let now = v.now();
        Ok(v.data()
            .environments
            .iter()
            .map(|e| EnvironmentView::new(e, now))
            .collect())
    }
    pub async fn save_environment(&self, draft: EnvironmentDraft) -> Result<EnvironmentView> {
        let mut s = self.inner.lock().await;
        let v = s.vault.as_mut().ok_or(OdxError::Locked)?;
        draft.validate()?;
        if v.data().environments.iter().any(|e| e.name == draft.name) {
            return Err(OdxError::validation(
                "validation.environment.name_exists",
                "an environment with this name already exists",
            ));
        }
        let env = draft.into_environment()?;
        let view = EnvironmentView::new(&env, v.now());
        v.update(|data| {
            data.environments.push(env);
            Ok(())
        })?;
        Ok(view)
    }
    pub async fn update_environment(
        &self,
        id: &str,
        mut draft: EnvironmentDraft,
    ) -> Result<EnvironmentView> {
        let mut s = self.inner.lock().await;
        let v = s.vault.as_mut().ok_or(OdxError::Locked)?;
        let old = v
            .data()
            .environments
            .iter()
            .find(|e| e.id == id)
            .cloned()
            .ok_or(OdxError::EnvironmentNotFound)?;
        if draft.secret.is_empty() {
            draft.secret = old.secret.as_str().to_owned();
        }
        draft.validate()?;
        if v.data()
            .environments
            .iter()
            .any(|e| e.id != id && e.name == draft.name)
        {
            return Err(OdxError::validation(
                "validation.environment.name_exists",
                "an environment with this name already exists",
            ));
        }
        let mut updated = draft.into_environment()?;
        // Context, headers and proxy have their own editors; connection updates preserve them.
        updated.context = old.context.clone();
        updated.disabled_context = old.disabled_context.clone();
        updated.headers = old.headers.clone();
        updated.proxy = old.proxy.clone();
        updated.tags = old.tags.clone();
        updated.id = old.id.clone();
        updated.revision = old.revision + 1;
        updated.write_mode = old.write_mode;
        updated.write_until = old.write_until;
        let view = EnvironmentView::new(&updated, v.now());
        // Do not transfer rules to a different server, database or user.
        let retarget = (&old.url, &old.database, &old.login)
            != (&updated.url, &updated.database, &updated.login);
        v.update(|data| {
            let env = data
                .environments
                .iter_mut()
                .find(|e| e.id == id)
                .ok_or(OdxError::EnvironmentNotFound)?;
            *env = updated;
            if retarget {
                data.rules.retain(|r| r.environment_id != id);
            }
            Ok(())
        })?;
        self.environment_changed(s, &[id.to_owned()]).await;
        Ok(view)
    }
    pub async fn rename_environment(&self, id: &str, new_name: &str) -> Result<()> {
        if new_name.trim().is_empty() {
            return Err(OdxError::validation(
                "validation.environment.name_required",
                "nonempty name required",
            ));
        }
        let mut s = self.inner.lock().await;
        let v = s.vault.as_mut().ok_or(OdxError::Locked)?;
        if v.data()
            .environments
            .iter()
            .any(|e| e.id != id && e.name == new_name)
        {
            return Err(OdxError::validation(
                "validation.environment.name_exists",
                "an environment with this name already exists",
            ));
        }
        v.update(|data| {
            let env = data
                .environments
                .iter_mut()
                .find(|e| e.id == id)
                .ok_or(OdxError::EnvironmentNotFound)?;
            env.name = new_name.to_owned();
            env.revision += 1;
            Ok(())
        })?;
        self.environment_changed(s, &[id.to_owned()]).await;
        Ok(())
    }
    /// Tags only organize the list; they keep the revision and pending approvals.
    pub async fn set_environment_tags(&self, id: &str, tags: &[String]) -> Result<EnvironmentView> {
        let tags = crate::config::normalize_tags(tags)?;
        let mut s = self.inner.lock().await;
        let v = s.vault.as_mut().ok_or(OdxError::Locked)?;
        let now = v.now();
        let mut view = None;
        v.update(|data| {
            let env = data
                .environments
                .iter_mut()
                .find(|e| e.id == id)
                .ok_or(OdxError::EnvironmentNotFound)?;
            env.tags = tags;
            view = Some(EnvironmentView::new(env, now));
            Ok(())
        })?;
        view.ok_or(OdxError::EnvironmentNotFound)
    }
    pub async fn clone_environment(&self, id: &str, new_name: &str) -> Result<EnvironmentView> {
        if new_name.trim().is_empty() {
            return Err(OdxError::validation(
                "validation.environment.name_required",
                "nonempty name required",
            ));
        }
        let mut s = self.inner.lock().await;
        let v = s.vault.as_mut().ok_or(OdxError::Locked)?;
        if v.data().environments.iter().any(|e| e.name == new_name) {
            return Err(OdxError::validation(
                "validation.environment.name_exists",
                "an environment with this name already exists",
            ));
        }
        let mut clone = v
            .data()
            .environments
            .iter()
            .find(|e| e.id == id)
            .cloned()
            .ok_or(OdxError::EnvironmentNotFound)?;
        clone.id = Uuid::new_v4().to_string();
        clone.name = new_name.into();
        clone.write_mode = WriteMode::ReadOnly;
        clone.write_until = None;
        clone.revision = 1;
        let view = EnvironmentView::new(&clone, v.now());
        v.update(|data| {
            data.environments.push(clone);
            Ok(())
        })?;
        Ok(view)
    }
    pub async fn delete_environment(&self, id: &str, confirmed: bool) -> Result<()> {
        if !confirmed {
            return Err(OdxError::validation(
                "validation.environment.delete_confirmation",
                "confirm environment deletion",
            ));
        }
        let mut s = self.inner.lock().await;
        let v = s.vault.as_mut().ok_or(OdxError::Locked)?;
        v.update(|data| {
            let before = data.environments.len();
            data.environments.retain(|e| e.id != id);
            if before == data.environments.len() {
                return Err(OdxError::EnvironmentNotFound);
            }
            // Recreating an environment name inherits neither grants nor rules.
            data.rules.retain(|r| r.environment_id != id);
            for grant in &mut data.clients {
                // Keep the token revision unchanged to preserve other connections.
                grant.environment_ids.retain(|e| e != id);
            }
            Ok(())
        })?;
        self.environment_changed(s, &[id.to_owned()]).await;
        Ok(())
    }
    pub async fn set_mode(&self, id: &str, mode: WriteMode) -> Result<()> {
        if mode == WriteMode::Temporary {
            return Err(OdxError::validation(
                "validation.environment.temporary_deadline_required",
                "temporary writes require a future deadline",
            ));
        }
        self.configure_mode(id, mode, None).await
    }
    pub async fn set_temporary_write(&self, id: &str, minutes: u32) -> Result<()> {
        if minutes == 0 {
            return Err(OdxError::validation(
                "validation.environment.duration_positive",
                "duration must be positive",
            ));
        }
        self.configure_mode(id, WriteMode::Temporary, Some(minutes))
            .await
    }
    /// Measure temporary access with the nondecreasing expiry clock.
    async fn configure_mode(&self, id: &str, mode: WriteMode, minutes: Option<u32>) -> Result<()> {
        let mut s = self.inner.lock().await;
        let v = s.vault.as_mut().ok_or(OdxError::Locked)?;
        let until = match minutes {
            Some(minutes) => Some(
                v.now()
                    .checked_add_signed(chrono::Duration::minutes(i64::from(minutes)))
                    .ok_or_else(|| {
                        OdxError::validation(
                            "validation.environment.duration_out_of_range",
                            "duration out of range",
                        )
                    })?,
            ),
            None => None,
        };
        v.update(|data| {
            let env = data
                .environments
                .iter_mut()
                .find(|e| e.id == id)
                .ok_or(OdxError::EnvironmentNotFound)?;
            env.write_mode = mode;
            env.write_until = until;
            env.revision += 1;
            Ok(())
        })?;
        self.environment_changed(s, &[id.to_owned()]).await;
        Ok(())
    }
    /// After environment changes, revoke and record unsent requests and invalidate cached authentication.
    pub(crate) async fn environment_changed(
        &self,
        mut s: tokio::sync::MutexGuard<'_, Inner>,
        ids: &[String],
    ) {
        let revoked = ids
            .iter()
            .flat_map(|id| revoke_environment(&mut s.pending, id))
            .collect();
        let ended = s.end(revoked, "revoked");
        drop(s);
        self.record(ended).await;
        for id in ids {
            self.odoo.invalidate_environment(id).await;
        }
    }
    /// After client changes, revoke and record its unsent requests.
    async fn client_changed(&self, mut s: tokio::sync::MutexGuard<'_, Inner>, id: &str) {
        let revoked = revoke_client(&mut s.pending, id);
        let ended = s.end(revoked, "revoked");
        drop(s);
        self.record(ended).await;
    }
    /// Record unsent outcomes. Audit failure marks audit unhealthy without reopening terminal requests.
    pub(crate) async fn record(&self, ended: Vec<Ended>) {
        for Ended {
            audit,
            approval,
            outcome,
        } in ended
        {
            let Some(log) = audit else { continue };
            let request = AuditRequest {
                log,
                id: approval.id,
                details: approval.details,
                started: std::time::Instant::now(),
            };
            let operation = format!("{}:{}", approval.call.method, approval.call.model);
            let _ = self
                .log_outcome_as(
                    request,
                    approval.client_id.as_deref(),
                    &approval.call.environment_id,
                    &operation,
                    outcome,
                )
                .await;
        }
    }
    pub async fn clients(&self) -> Result<Vec<ClientView>> {
        let (mut views, audit, session) = {
            let s = self.inner.lock().await;
            let v = s.vault.as_ref().ok_or(OdxError::Locked)?;
            let views: Vec<ClientView> = v
                .data()
                .clients
                .iter()
                .filter(|c| !c.revoked)
                .map(ClientView::from)
                .collect();
            (views, s.audit.clone(), s.session)
        };
        // Initialize last use from history once per session, then track it in memory.
        let seeded = self
            .usage
            .lock()
            .map(|u| u.seeded_session == Some(session))
            .unwrap_or(true);
        if !seeded && let Some(audit) = audit {
            let history = tokio::task::spawn_blocking(move || audit.last_used())
                .await
                .ok()
                .and_then(|r| r.ok());
            if let (Some(events), Ok(mut usage)) = (history, self.usage.lock()) {
                for (id, at) in events {
                    let last = usage.last_used.entry(id).or_insert(at);
                    if at > *last {
                        *last = at;
                    }
                }
                usage.seeded_session = Some(session);
            }
        }
        if let Ok(usage) = self.usage.lock() {
            for view in &mut views {
                view.last_used_at = usage.last_used.get(&view.id).copied();
            }
        }
        Ok(views)
    }
    pub(crate) fn mark_used(&self, client_id: &str) {
        if let Ok(mut usage) = self.usage.lock() {
            usage.last_used.insert(client_id.into(), Utc::now());
        }
    }
    /// Register an authenticated MCP connection until its guard drops.
    pub fn register_connection(self: &Arc<Self>, client_id: &str) -> ConnectionGuard {
        let mut usage = self.usage.lock().expect("usage");
        usage.next_connection += 1;
        let id = usage.next_connection;
        usage.connections.insert(id, (client_id.into(), Utc::now()));
        ConnectionGuard {
            core: self.clone(),
            id,
        }
    }
    pub async fn mcp_connections(&self) -> Vec<McpConnectionInfo> {
        let labels: HashMap<String, String> = {
            let s = self.inner.lock().await;
            s.vault
                .as_ref()
                .map(|v| {
                    v.data()
                        .clients
                        .iter()
                        .map(|c| (c.id.clone(), c.label.clone()))
                        .collect()
                })
                .unwrap_or_default()
        };
        let usage = self.usage.lock().expect("usage");
        let mut list: Vec<McpConnectionInfo> = usage
            .connections
            .values()
            .map(|(client_id, since)| McpConnectionInfo {
                client_label: labels.get(client_id).cloned(),
                client_id: client_id.clone(),
                since: *since,
            })
            .collect();
        list.sort_by_key(|c| c.since);
        list
    }
    /// Create a client; expose plaintext only in the returned value.
    pub async fn create_client(
        &self,
        label: &str,
        scope: impl Into<clients::ClientScope>,
        capabilities: Vec<Capability>,
    ) -> Result<IssuedToken> {
        let scope = scope.into();
        let mut s = self.inner.lock().await;
        let v = s.vault.as_mut().ok_or(OdxError::Locked)?;
        let data = v.data();
        clients::validate_grant(label, &scope, &capabilities, |id| {
            data.environments.iter().any(|e| e.id == id)
        })?;
        if data
            .clients
            .iter()
            .any(|c| !c.revoked && c.label.eq_ignore_ascii_case(label.trim()))
        {
            return Err(OdxError::validation(
                "validation.client.name_exists",
                "a client with this name already exists",
            ));
        }
        let (token, hash) = clients::issue_token()?;
        let grant = clients::new_grant(label, scope, capabilities, hash);
        let client = ClientView::from(&grant);
        v.update(|data| {
            data.clients.push(grant);
            Ok(())
        })?;
        Ok(IssuedToken { client, token })
    }
    /// Update label, scope and capabilities without changing the token; the local MCP label is fixed. Revoke requests made under the old grant.
    pub async fn update_client(
        &self,
        id: &str,
        label: &str,
        scope: impl Into<clients::ClientScope>,
        capabilities: Vec<Capability>,
    ) -> Result<ClientView> {
        if id == clients::STDIO_CLIENT_ID && label != clients::STDIO_CLIENT_LABEL {
            return Err(OdxError::validation(
                "validation.client.local_profile_name_fixed",
                "the local MCP profile name cannot change",
            ));
        }
        let scope = scope.into();
        let mut s = self.inner.lock().await;
        let v = s.vault.as_mut().ok_or(OdxError::Locked)?;
        let data = v.data();
        clients::validate_grant(label, &scope, &capabilities, |env| {
            data.environments.iter().any(|e| e.id == env)
        })?;
        if data
            .clients
            .iter()
            .any(|c| c.id != id && !c.revoked && c.label.eq_ignore_ascii_case(label.trim()))
        {
            return Err(OdxError::validation(
                "validation.client.name_exists",
                "a client with this name already exists",
            ));
        }
        let updated = clients::new_grant(label, scope, capabilities, String::new());
        let view = v.update(|data| {
            let grant = data
                .clients
                .iter_mut()
                .find(|c| c.id == id && !c.revoked)
                .ok_or(OdxError::ClientNotFound)?;
            grant.label = updated.label;
            grant.all_environments = updated.all_environments;
            grant.environment_ids = updated.environment_ids;
            grant.capabilities = updated.capabilities;
            Ok(ClientView::from(&*grant))
        })?;
        self.client_changed(s, id).await;
        let _ = self.grant_changes.send(id.to_string());
        Ok(view)
    }
    /// Replace the token, invalidate the old one and revoke pending requests.
    pub async fn replace_client_token(&self, id: &str) -> Result<IssuedToken> {
        if id == clients::STDIO_CLIENT_ID {
            return Err(OdxError::validation(
                "validation.client.local_profile_no_token",
                "the MCP stdio profile does not use a token",
            ));
        }
        let mut s = self.inner.lock().await;
        let v = s.vault.as_mut().ok_or(OdxError::Locked)?;
        let (token, hash) = clients::issue_token()?;
        let client = v.update(|data| {
            let grant = data
                .clients
                .iter_mut()
                .find(|c| c.id == id && !c.revoked)
                .ok_or(OdxError::ClientNotFound)?;
            grant.token_hash = hash;
            grant.revision += 1;
            grant.token_issued_at = Utc::now();
            Ok(ClientView::from(&*grant))
        })?;
        self.client_changed(s, id).await;
        Ok(IssuedToken { client, token })
    }
    /// Delete the client permanently, invalidate its token and revoke pending requests. Retain its ID without a label in activity.
    pub async fn revoke_client(&self, id: &str) -> Result<()> {
        if id == clients::STDIO_CLIENT_ID {
            return Err(OdxError::validation(
                "validation.client.local_profile_shared",
                "the MCP stdio profile is shared; edit its scope and capabilities or lock the app",
            ));
        }
        let mut s = self.inner.lock().await;
        let v = s.vault.as_mut().ok_or(OdxError::Locked)?;
        // Deleting the client invalidates its token and removes it from the list. Also purge legacy revoked grants.
        v.update(|data| {
            if !data.clients.iter().any(|c| c.id == id && !c.revoked) {
                return Err(OdxError::ClientNotFound);
            }
            data.clients.retain(|c| c.id != id && !c.revoked);
            Ok(())
        })?;
        self.client_changed(s, id).await;
        Ok(())
    }
    /// Shared stdio profile authenticated by socket UID, never HTTP token.
    pub(crate) async fn stdio_identity(&self) -> Result<ClientIdentity> {
        let s = self.inner.lock().await;
        let vault = s.vault.as_ref().ok_or(OdxError::Locked)?;
        let grant = vault
            .data()
            .clients
            .iter()
            .find(|g| g.id == clients::STDIO_CLIENT_ID && !g.revoked)
            .ok_or(OdxError::Unauthorized)?;
        Ok(ClientIdentity {
            client_id: grant.id.clone(),
            revision: grant.revision,
            environment_ids: if grant.all_environments {
                vault
                    .data()
                    .environments
                    .iter()
                    .map(|e| e.id.clone())
                    .collect()
            } else {
                grant.environment_ids.clone()
            },
            capabilities: grant.capabilities.clone(),
        })
    }
    /// Authenticate a client token; reject all clients while locked.
    pub async fn authenticate_client(&self, token: &str) -> Result<ClientIdentity> {
        let s = self.inner.lock().await;
        let v = s.vault.as_ref().ok_or(OdxError::Locked)?;
        let grant = clients::find_grant(&v.data().clients, token).ok_or(OdxError::Unauthorized)?;
        Ok(ClientIdentity {
            client_id: grant.id.clone(),
            revision: grant.revision,
            environment_ids: if grant.all_environments {
                v.data().environments.iter().map(|e| e.id.clone()).collect()
            } else {
                grant.environment_ids.clone()
            },
            capabilities: grant.capabilities.clone(),
        })
    }
    /// Check reachability (`common.version`) and credentials (revision-cached login), without accessing business data.
    pub async fn check_environment(&self, id: &str) -> Result<HealthReport> {
        let (env, session) = {
            let s = self.inner.lock().await;
            let v = s.vault.as_ref().ok_or(OdxError::Locked)?;
            let env = v.data().connection_environment(id)?;
            (env, s.session)
        };
        let started = std::time::Instant::now();
        let resolved = self.resolve_protocol(env.clone(), session).await;
        let permits = self.limits.acquire(&env.id).await?;
        self.assert_environment_current(&env.id, env.revision, session)
            .await?;
        let report = self.health(&env, resolved, started).await;
        drop(permits);
        Ok(report)
    }
    /// Check an unsaved draft like a new environment (global headers and proxy) without persisting anything.
    pub async fn check_draft(&self, draft: EnvironmentDraft) -> Result<HealthReport> {
        let env = self.draft_connection(draft.into_environment()?).await?;
        let started = std::time::Instant::now();
        let permits = self.limits.acquire(&env.id).await?;
        let resolved = match env.protocol {
            crate::config::Protocol::Auto => self
                .odoo
                .detect_protocol(&env, || async { Ok(()) })
                .await
                .map(|protocol| Environment {
                    protocol,
                    ..env.clone()
                }),
            _ => Ok(env.clone()),
        };
        let report = self.health(&env, resolved, started).await;
        drop(permits);
        self.odoo.invalidate_environment(&env.id).await;
        Ok(report)
    }
    /// Databases listed by an Odoo server, unauthenticated; servers with `list_db = False` refuse.
    pub async fn list_databases(&self, url: &str) -> Result<Vec<String>> {
        crate::config::validate_url(url)?;
        let env = self
            .draft_connection(Environment {
                id: Uuid::new_v4().to_string(),
                url: url.trim_end_matches('/').into(),
                ..Default::default()
            })
            .await?;
        let _permits = self.limits.acquire(&env.id).await?;
        self.odoo.list_databases(&env).await
    }
    /// Apply global headers and the inherited proxy to an environment that is not in the vault.
    async fn draft_connection(&self, mut env: Environment) -> Result<Environment> {
        let s = self.inner.lock().await;
        let data = s.vault.as_ref().ok_or(OdxError::Locked)?.data();
        env.headers = crate::headers::merge(&data.global_headers, &[]);
        env.proxy = data.effective_proxy(&env);
        Ok(env)
    }
    async fn health(
        &self,
        env: &Environment,
        resolved: Result<Environment>,
        started: std::time::Instant,
    ) -> HealthReport {
        let version = match &resolved {
            Ok(env) => self.odoo.version(env).await,
            Err(_) => Ok(None),
        };
        let latency_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
        let protocol = resolved.as_ref().ok().map(|e| e.protocol);
        let report = |status: &str, server_version, error: Option<OdxError>| HealthReport {
            environment_id: env.id.clone(),
            status: status.into(),
            server_version,
            protocol,
            latency_ms,
            http_status: match &error {
                Some(OdxError::HttpStatus { status }) => Some(*status),
                _ => None,
            },
            network_error: match &error {
                Some(OdxError::Network { kind, .. }) => Some((*kind).into()),
                _ => None,
            },
            error: error
                .as_ref()
                .and_then(OdxError::safe_detail)
                .map(str::to_owned),
            error_code: error.as_ref().map(|e| e.code().to_owned()),
            error_params: error.as_ref().map(OdxError::ui_params),
        };
        match resolved {
            Err(error @ OdxError::AuthenticationRejected) => {
                report("auth_failed", None, Some(error))
            }
            Err(error) => report("unreachable", None, Some(error)),
            Ok(env) => match version {
                Err(error) => report("unreachable", None, Some(error)),
                Ok(server_version) => match self.odoo.authenticate(&env).await {
                    Ok(_) => report("ok", server_version, None),
                    Err(error) => report("auth_failed", server_version, Some(error)),
                },
            },
        }
    }
    /// Resolve transport before argument validation. Persist configuration only, without changing revision or permissions.
    pub(crate) async fn resolve_protocol(
        &self,
        mut env: Environment,
        session: u64,
    ) -> Result<Environment> {
        if env.protocol != crate::config::Protocol::Auto {
            return Ok(env);
        }
        let _permit = self.limits.acquire(&env.id).await?;
        let protocol = self
            .odoo
            .detect_protocol(&env, || {
                self.assert_environment_current(&env.id, env.revision, session)
            })
            .await?;
        self.assert_environment_current(&env.id, env.revision, session)
            .await?;
        let mut state = self.inner.lock().await;
        if state.session != session {
            return Err(OdxError::ApprovalUnavailable);
        }
        let vault = state.vault.as_mut().ok_or(OdxError::Locked)?;
        let current = vault
            .data()
            .environments
            .iter()
            .find(|e| e.id == env.id)
            .ok_or(OdxError::EnvironmentNotFound)?;
        if current.revision != env.revision {
            return Err(OdxError::ApprovalUnavailable);
        }
        if current.detected_protocol != Some(protocol) {
            vault.update(|data| {
                let stored = data
                    .environments
                    .iter_mut()
                    .find(|e| e.id == env.id)
                    .ok_or(OdxError::EnvironmentNotFound)?;
                stored.detected_protocol = Some(protocol);
                Ok(())
            })?;
        }
        env.protocol = protocol;
        env.detected_protocol = Some(protocol);
        Ok(env)
    }
    pub async fn approvals(&self) -> Result<Vec<PendingApproval>> {
        let mut s = self.inner.lock().await;
        let now = s.vault.as_ref().ok_or(OdxError::Locked)?.now();
        for approval in s.pending.values_mut() {
            approval.expire_if_due(now);
        }
        let _v = s.vault.as_ref().ok_or(OdxError::Locked)?;
        Ok(s.pending
            .values()
            .filter(|p| p.state == crate::approval::State::Pending)
            .filter_map(|p| {
                let e = _v
                    .data()
                    .environments
                    .iter()
                    .find(|e| e.id == p.call.environment_id)?;
                Some(PendingApproval {
                    id: p.id.clone(),
                    client_id: p.client_id.clone(),
                    client_label: p.client_id.as_ref().and_then(|c| {
                        _v.data()
                            .clients
                            .iter()
                            .find(|g| &g.id == c)
                            .map(|g| g.label.clone())
                    }),
                    environment_id: e.id.clone(),
                    environment_name: e.name.clone(),
                    model: p.call.model.clone(),
                    method: p.call.method.clone(),
                    args: p.call.args.clone(),
                    kwargs: p.call.kwargs.clone(),
                    context: p.call.kwargs.get("context").cloned().unwrap_or(json!({})),
                    created_at: p.created_at,
                    expires_at: p.call.expires_at,
                })
            })
            .collect())
    }
    /// Cancellation by the human or a client abandoning its wait.
    pub async fn cancel_approval(&self, id: &str) -> Result<()> {
        self.withdraw(id, false).await.map(drop)
    }
    /// Remove an undecided request and notify its waiter as cancelled or expired. Reject already approved or sent requests.
    pub(crate) async fn withdraw(&self, id: &str, expire: bool) -> Result<&'static str> {
        let mut s = self.inner.lock().await;
        let now = s.vault.as_ref().map_or_else(Utc::now, |v| v.now());
        let p = s.pending.get_mut(id).ok_or(OdxError::ApprovalUnavailable)?;
        p.expire_if_due(now);
        let outcome = match p.state {
            State::Pending if !expire => "cancelled",
            State::Pending | State::Expired => "expired",
            _ => return Err(OdxError::ApprovalUnavailable),
        };
        let ended = s.end(vec![id.to_owned()], outcome);
        drop(s);
        self.record(ended).await;
        Ok(outcome)
    }
    /// Deliver the human decision and outcome to any waiting client.
    pub async fn decide(&self, id: &str, approve: bool) -> Result<Value> {
        self.decide_measured(id, approve, None)
            .await
            .map(|(value, _)| value)
    }
    /// Reject and tell the waiting client why; blank reasons are dropped, long ones cut to 500 characters.
    pub async fn reject(&self, id: &str, reason: Option<String>) -> Result<Value> {
        let reason = reason
            .map(|r| r.trim().chars().take(500).collect::<String>())
            .filter(|r| !r.is_empty());
        self.decide_measured(id, false, reason)
            .await
            .map(|(value, _)| value)
    }
    /// Like `decide`, with upstream metrics; rejection has none. Waiters receive their metrics through the channel.
    pub(crate) async fn decide_measured(
        &self,
        id: &str,
        approve: bool,
        reason: Option<String>,
    ) -> Result<(Value, Upstream)> {
        // Only the winning decision closes the request.
        let (env, call, client, authorization) = self.claim(id, approve).await?;
        let result = self
            .dispatch(id, approve, env, call, client, authorization)
            .await;
        let mut s = self.inner.lock().await;
        if let Some(tx) = s.waiters.remove(id) {
            let end = match &result {
                Ok(_) if !approve => ApprovalEnd::Rejected(reason),
                Ok((value, up)) => ApprovalEnd::Executed(value.clone(), *up),
                Err(error) => ApprovalEnd::Failed(error.duplicate()),
            };
            let _ = tx.send(end);
        }
        s.pending.remove(id);
        result
    }
    async fn claim(
        &self,
        id: &str,
        approve: bool,
    ) -> Result<(
        Environment,
        crate::approval::ApprovalCall,
        Option<String>,
        policy::Authorization,
    )> {
        let mut s = self.inner.lock().await;
        let v = s.vault.as_ref().ok_or(OdxError::Locked)?;
        let env_id = s
            .pending
            .get(id)
            .ok_or(OdxError::ApprovalUnavailable)?
            .call
            .environment_id
            .clone();
        let env = v.data().connection_environment(&env_id)?;
        let now = v.now();
        let p = s.pending.get_mut(id).ok_or(OdxError::ApprovalUnavailable)?;
        p.decide(
            if approve {
                Decision::Approve
            } else {
                Decision::Reject
            },
            now,
        )?;
        Ok((
            env,
            p.call.clone(),
            p.client_id.clone(),
            p.authorization.clone(),
        ))
    }
    async fn dispatch(
        &self,
        id: &str,
        approve: bool,
        env: Environment,
        call: crate::approval::ApprovalCall,
        client: Option<String>,
        authorization: policy::Authorization,
    ) -> Result<(Value, Upstream)> {
        // Measure `upstream_ms` after the decision: admission, audit, login and dispatch.
        let mut up = Upstream::start();
        let client = client.as_deref();
        let operation = format!("{}:{}", call.method, call.model);
        let mut details =
            AuditDetails::call(&env, &call.model, &call.method, &call.args, &call.kwargs);
        details.authorization = Some(authorization.clone());
        let mut audit = self.audit_request(id, details).await?;
        if !approve {
            self.log_outcome_as(audit, client, &env.id, &operation, "rejected")
                .await?;
            return Ok((json!({"status":"rejected"}), up));
        }
        let session = self.inner.lock().await.session;
        let env = self.resolve_protocol(env, session).await?;
        // Errors before dispatch close the request as not sent.
        let (target, op) = (&env, operation.as_str());
        let not_sent = move |mut audit: AuditRequest, error: OdxError| async move {
            audit.finish_odx_error(target, &error);
            let _ = self
                .log_outcome_as(audit, client, &target.id, op, "not_sent")
                .await;
            Err(error)
        };
        let permits = match self.limits.acquire(&env.id).await {
            Ok(permits) => permits,
            Err(error) => return not_sent(audit, error).await,
        };
        // The wait for a slot may outlast a lock or an edit; never log in with a stale secret.
        if let Err(error) = self
            .assert_environment_current(&env.id, call.revision, session)
            .await
        {
            return not_sent(audit, error).await;
        }
        if let Err(error) = self
            .log_open(audit.clone(), client, &env.id, &operation, "attempted")
            .await
        {
            return not_sent(audit, error).await;
        }
        let uid = match self.odoo.authenticate_measured(&env).await {
            Ok((uid, login)) => {
                // Only the call that logged in counts login in its metrics.
                if let Some(bytes) = login {
                    up.calls += 1;
                    up.bytes += bytes;
                }
                uid
            }
            Err(error) => {
                audit.finish_odx_error(&env, &error);
                self.log_outcome_as(audit, client, &env.id, &operation, "failed")
                    .await?;
                return Err(error);
            }
        };
        // This in-memory sent transition shares the lock used by vault_lock and revision changes.
        let checked: Result<_> = async {
            let mut s = self.inner.lock().await;
            if s.audit_failed {
                return Err(OdxError::Persistence);
            }
            let v = s.vault.as_ref().ok_or(OdxError::Locked)?;
            let latest = v.data().connection_environment(&env.id)?;
            // Revalidate against current mode, deadline and rule.
            let authorized = authorization_current(v, &latest, &call, &authorization);
            let now = v.now();
            let pending = s.pending.get_mut(id).ok_or(OdxError::ApprovalUnavailable)?;
            if pending.state != State::Approved
                || latest.revision != call.revision
                || call.expires_at <= now
                || !authorized
            {
                return Err(OdxError::ApprovalUnavailable);
            }
            // Sent requests remain until the outcome is known, regardless of revocation or expiry.
            pending.state = State::Sent;
            Ok(latest)
        }
        .await;
        let current = match checked {
            Ok(current) => current,
            // Close the audited attempt as not sent.
            Err(error) => return not_sent(audit, error).await,
        };
        let result = self
            .odoo
            .execute_kw_measured(
                &current,
                uid,
                &call.model,
                &call.method,
                call.args,
                call.kwargs,
            )
            .await
            .map_err(crate::odoo::jsonrpc::sent_error);
        drop(permits);
        up.calls += 1;
        match &result {
            Ok((value, _)) => audit.finish(&env, Ok(value)),
            Err(error) => audit.finish_odx_error(&env, error),
        }
        let outcome = match &result {
            Ok(_) => "succeeded",
            Err(OdxError::Uncertain) => "uncertain",
            Err(_) => "failed",
        };
        // Already sent: an audit failure must not turn the real outcome into "not sent". append() sets audit_failed, which blocks new operations.
        let _ = self
            .log_outcome_as(audit, client, &env.id, &operation, outcome)
            .await;
        let (value, bytes) = result?;
        up.bytes += bytes;
        Ok((value, up))
    }
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
    async fn audit_request(&self, id: &str, details: AuditDetails) -> Result<AuditRequest> {
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
    async fn log_open(
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
        let v = s.vault.as_ref().ok_or(OdxError::Locked)?;
        let env = v
            .data()
            .environments
            .iter()
            .find(|e| e.id == id)
            .ok_or(OdxError::EnvironmentNotFound)?;
        if env.revision != revision {
            return Err(OdxError::ApprovalUnavailable);
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../tests/support/mod.rs"]
mod mode_test_support;

#[cfg(test)]
mod mode_tests {
    use super::*;
    use crate::Protocol;

    #[tokio::test]
    async fn temporary_expiry_blocks_a_write_waiting_for_capacity() {
        let tmp = tempfile::tempdir().unwrap();
        let sim = super::mode_test_support::simulator().await;
        let core = Core::new(tmp.path().join("vault.enc")).unwrap();
        core.create("fictional master password").await.unwrap();
        let env = core
            .save_environment(EnvironmentDraft {
                name: "fixture".into(),
                url: sim.url,
                database: "base-a".into(),
                login: "fixture-user".into(),
                protocol: Protocol::LegacyJsonrpc,
                context: json!({}),
                secret: "fixture-secret".into(),
            })
            .await
            .unwrap();
        let issued = core
            .create_client(
                "fixture-client",
                crate::ClientScope::All,
                vec![Capability::OdooWriteRequest],
            )
            .await
            .unwrap();
        let client = core.authenticate_client(&issued.token).await.unwrap();
        core.set_temporary_write(&env.id, 1).await.unwrap();
        let mut permits = Vec::new();
        for _ in 0..4 {
            permits.push(core.limits.acquire(&env.id).await.unwrap());
        }
        let writing = core.clone();
        let task = tokio::spawn(async move {
            writing
                .run_tool(
                    &client,
                    "write",
                    json!({"env":"fixture","model":"res.partner","ids":[1],"values":{"name":"must not be sent"}}),
                )
                .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if core.limits.queue_depth(&env.id).unwrap().environment == 1 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        {
            let mut state = core.inner.lock().await;
            state.vault.as_mut().unwrap().data_mut().environments[0].write_until =
                Some(Utc::now() - chrono::Duration::seconds(1));
        }
        drop(permits);
        assert!(task.await.unwrap().is_err());
        assert_eq!(sim.counts.writes(), 0);
        assert_eq!(
            core.environments().await.unwrap()[0].write_mode,
            WriteMode::ReadOnly
        );
    }
}

#[cfg(test)]
mod client_revocation_tests {
    use super::*;
    use crate::{approval::State, config::Environment};

    #[test]
    fn revoking_a_client_only_revokes_its_own_pending_requests() {
        let env = Environment::default();
        let mut pending = HashMap::new();
        for owner in [Some("c1"), Some("c2"), None] {
            let mut approval = Approval::new(
                &env,
                "res.partner".into(),
                "write".into(),
                json!([]),
                json!({}),
                Utc::now(),
            );
            approval.client_id = owner.map(Into::into);
            pending.insert(approval.id.clone(), approval);
        }
        revoke_client(&mut pending, "c1");
        let states: Vec<_> = pending
            .values()
            .map(|a| (a.client_id.clone(), a.state))
            .collect();
        for (owner, state) in states {
            let expected = if owner.as_deref() == Some("c1") {
                State::Revoked
            } else {
                State::Pending
            };
            assert_eq!(state, expected);
        }
    }
}

#[cfg(test)]
mod approval_end_tests {
    use super::mode_test_support as support;
    use super::*;
    use crate::{ActivityFilter, Protocol, approval::TTL};
    use std::time::Duration;

    struct Fixture {
        sim: support::Simulator,
        core: Arc<Core>,
        env: String,
        client: ClientIdentity,
        _tmp: tempfile::TempDir,
    }
    async fn fixture(mode: WriteMode) -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let sim = support::simulator().await;
        let core = Core::new(tmp.path().join("vault.sqlite3")).unwrap();
        core.create("fictional master password").await.unwrap();
        let env = core
            .save_environment(EnvironmentDraft {
                name: "A".into(),
                url: sim.url.clone(),
                database: "base-a".into(),
                login: "fixture-user".into(),
                protocol: Protocol::LegacyJsonrpc,
                context: json!({}),
                secret: "fixture-secret".into(),
            })
            .await
            .unwrap()
            .id;
        if mode != WriteMode::ReadOnly {
            core.set_mode(&env, mode).await.unwrap();
        }
        let issued = core
            .create_client(
                "fixture-client",
                crate::ClientScope::All,
                vec![Capability::OdooRead, Capability::OdooWriteRequest],
            )
            .await
            .unwrap();
        let client = core.authenticate_client(&issued.token).await.unwrap();
        Fixture {
            sim,
            core,
            env,
            client,
            _tmp: tmp,
        }
    }
    fn spawn_tool(
        core: &Arc<Core>,
        client: &ClientIdentity,
        tool: &'static str,
        arguments: Value,
    ) -> tokio::task::JoinHandle<std::result::Result<crate::ToolOutput, crate::ToolError>> {
        let (core, client) = (core.clone(), client.clone());
        tokio::spawn(async move { core.run_tool(&client, tool, arguments).await })
    }
    fn write() -> Value {
        json!({"env":"A","model":"res.partner","ids":[1],"values":{"name":"fixture"}})
    }
    fn read() -> Value {
        json!({"env":"A","model":"res.partner"})
    }
    async fn pending(core: &Core) -> String {
        for _ in 0..200 {
            if let Some(p) = core.approvals().await.unwrap().into_iter().next() {
                return p.id;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("pending approval did not appear");
    }
    async fn outcomes(core: &Core) -> Vec<(Option<String>, String)> {
        core.activity(None, ActivityFilter::default())
            .await
            .unwrap()
            .events
            .into_iter()
            .map(|e| (e.request_id, e.outcome))
            .collect()
    }
    async fn outcome(core: &Core, id: &str) -> String {
        outcomes(core)
            .await
            .into_iter()
            .find(|(r, _)| r.as_deref() == Some(id))
            .unwrap()
            .1
    }
    async fn audit_queue(core: &Core, queued: usize) {
        let audit = core.inner.lock().await.audit.clone().unwrap();
        audit
            .queued()
            .store(queued, std::sync::atomic::Ordering::SeqCst);
    }

    #[tokio::test]
    async fn expiry_during_the_send_still_returns_the_real_result() {
        let f = fixture(WriteMode::Approval).await;
        TTL.with(|t| t.set(chrono::Duration::seconds(1)));
        let call = spawn_tool(&f.core, &f.client, "write", write());
        let id = pending(&f.core).await;
        f.sim.counts.pause_next_write();
        let decision = {
            let (core, id) = (f.core.clone(), id.clone());
            tokio::spawn(async move { core.decide(&id, true).await })
        };
        f.sim.counts.wait_for_write().await;
        tokio::time::sleep(Duration::from_millis(1500)).await;
        f.sim.counts.release_write();
        assert_eq!(decision.await.unwrap().unwrap(), json!(true));
        assert_eq!(call.await.unwrap().unwrap().data, json!(true));
        assert_eq!(f.sim.counts.writes(), 1);
        assert_eq!(outcome(&f.core, &id).await, "succeeded");
    }

    #[tokio::test]
    async fn an_expired_request_is_recorded_as_expired() {
        let f = fixture(WriteMode::Approval).await;
        TTL.with(|t| t.set(chrono::Duration::milliseconds(300)));
        let call = spawn_tool(&f.core, &f.client, "write", write());
        let id = pending(&f.core).await;
        assert_eq!(call.await.unwrap().unwrap_err().code, "approval_expired");
        assert_eq!(outcome(&f.core, &id).await, "expired");
        assert!(f.core.inner.lock().await.pending.is_empty());
    }

    #[tokio::test]
    async fn an_external_cancellation_wakes_the_waiting_client() {
        let f = fixture(WriteMode::Approval).await;
        let call = spawn_tool(&f.core, &f.client, "write", write());
        let id = pending(&f.core).await;
        f.core.cancel_approval(&id).await.unwrap();
        let ended = tokio::time::timeout(Duration::from_secs(2), call).await;
        assert_eq!(ended.unwrap().unwrap().unwrap_err().code, "cancelled");
        assert_eq!(outcome(&f.core, &id).await, "cancelled");
    }

    #[tokio::test]
    async fn a_full_audit_queue_is_a_capacity_error_not_an_audit_failure() {
        let f = fixture(WriteMode::ReadOnly).await;
        audit_queue(&f.core, 128).await;
        let error = f
            .core
            .run_tool(&f.client, "search_read", read())
            .await
            .unwrap_err();
        assert_eq!(error.code, "capacity_exceeded");
        audit_queue(&f.core, 0).await;
        assert!(
            f.core
                .run_tool(&f.client, "search_read", read())
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn closing_events_after_the_send_are_not_capped_by_the_queue() {
        let f = fixture(WriteMode::Permanent).await;
        f.sim.counts.pause_next_write();
        let call = spawn_tool(&f.core, &f.client, "write", write());
        f.sim.counts.wait_for_write().await;
        audit_queue(&f.core, 128).await;
        f.sim.counts.release_write();
        assert_eq!(call.await.unwrap().unwrap().data, json!(true));
        audit_queue(&f.core, 0).await;
        assert_eq!(outcomes(&f.core).await[0].1, "succeeded");
        assert!(
            f.core
                .run_tool(&f.client, "search_read", read())
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn an_error_between_approval_and_send_closes_the_request() {
        let f = fixture(WriteMode::Approval).await;
        let call = spawn_tool(&f.core, &f.client, "write", write());
        let id = pending(&f.core).await;
        // Environment capacity: 4 active calls and 16 queued.
        let mut held = Vec::new();
        for _ in 0..4 {
            held.push(f.core.limits.acquire(&f.env).await.unwrap());
        }
        let queued: Vec<_> = (0..16)
            .map(|_| {
                let (core, env) = (f.core.clone(), f.env.clone());
                tokio::spawn(async move { core.limits.acquire(&env).await.map(drop) })
            })
            .collect();
        while f.core.limits.queue_depth(&f.env).unwrap().environment < 16 {
            tokio::task::yield_now().await;
        }
        assert!(matches!(
            f.core.decide(&id, true).await,
            Err(OdxError::Capacity)
        ));
        let ended = tokio::time::timeout(Duration::from_secs(2), call).await;
        assert!(ended.unwrap().unwrap().is_err());
        assert!(f.core.inner.lock().await.pending.is_empty());
        assert_eq!(outcome(&f.core, &id).await, "not_sent");
        drop(held);
        for task in queued {
            let _ = task.await;
        }
        assert_eq!(f.sim.counts.writes(), 0);
    }

    #[tokio::test]
    async fn a_shell_command_whose_environment_changed_while_queued_leaves_no_attempt() {
        let f = fixture(WriteMode::ReadOnly).await;
        let mut held = Vec::new();
        for _ in 0..4 {
            held.push(f.core.limits.acquire(&f.env).await.unwrap());
        }
        let shell = {
            let (core, env) = (f.core.clone(), f.env.clone());
            tokio::spawn(async move {
                core.shell_call(crate::ShellCall {
                    environment_id: env,
                    model: "res.partner".into(),
                    method: "search_read".into(),
                    args: vec![],
                    kwargs: Default::default(),
                    context: Default::default(),
                    confirmed: false,
                })
                .await
            })
        };
        while f.core.limits.queue_depth(&f.env).unwrap().environment < 1 {
            tokio::task::yield_now().await;
        }
        f.core
            .set_context(
                Some(&f.env),
                vec![crate::context::ContextEntry {
                    name: "lang".into(),
                    value: json!("es_ES"),
                    enabled: true,
                }],
            )
            .await
            .unwrap();
        drop(held);
        assert!(shell.await.unwrap().is_err());
        assert!(
            outcomes(&f.core)
                .await
                .iter()
                .all(|(_, outcome)| outcome != "attempted")
        );
    }
}

#[cfg(test)]
mod reset_tests {
    use super::*;

    #[tokio::test]
    async fn reset_sets_a_locked_vault_aside_and_allows_a_new_one() {
        let tmp = tempfile::tempdir().unwrap();
        let core = Core::new(tmp.path().join("vault.sqlite3")).unwrap();
        core.create("fixture password").await.unwrap();
        assert!(
            core.reset().await.is_err(),
            "an unlocked vault must not be reset"
        );
        core.lock().await;
        core.reset().await.unwrap();
        assert_eq!(core.status().await, "absent");
        let aside: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| {
                p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("reset-")
            })
            .collect();
        assert_eq!(aside.len(), 1);
        assert!(aside[0].join("vault.sqlite3").exists());
        core.create("another password").await.unwrap();
        assert_eq!(core.status().await, "unlocked");
    }

    #[tokio::test]
    async fn reset_also_sets_a_legacy_vault_aside() {
        let tmp = tempfile::tempdir().unwrap();
        drop(
            crate::legacy_vault::Vault::create(tmp.path().join("vault.enc"), "fixture password")
                .unwrap(),
        );
        let core = Core::new(tmp.path().join("vault.sqlite3")).unwrap();
        assert_eq!(core.status().await, "locked");
        core.reset().await.unwrap();
        assert_eq!(core.status().await, "absent");
    }
}
