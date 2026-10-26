use crate::{
    approval::{Approval, ApprovalEnd, ApprovalNotice, revoke_all},
    audit::{AuditLog, AuditRequest},
    clients::Usage,
    config::{Bound, Environment, Settings},
    error::{OdxError, Result},
    limits::Limits,
    odoo::jsonrpc::OdooClient,
    vault::Vault,
};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};
use tokio::sync::{Mutex, oneshot};

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
impl Inner {
    /// The unlocked vault; `Locked` otherwise.
    pub(crate) fn vault(&self) -> Result<&Vault> {
        self.vault.as_ref().ok_or(OdxError::Locked)
    }
    pub(crate) fn vault_mut(&mut self) -> Result<&mut Vault> {
        self.vault.as_mut().ok_or(OdxError::Locked)
    }
}
/// Locking or another unlock during the KDF invalidates this vault installation.
fn superseded(s: &Inner, session: u64) -> Result<()> {
    if s.session != session || s.vault.is_some() {
        return Err(OdxError::Locked);
    }
    Ok(())
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
    /// The desktop window, for tools that show or run things in it; unset without the app.
    pub(crate) desktop: std::sync::Mutex<
        Option<tokio::sync::mpsc::UnboundedSender<crate::tools::terminal::DesktopRequest>>,
    >,
}
impl Core {
    pub fn new(path: impl AsRef<Path>) -> Result<Arc<Self>> {
        Ok(Arc::new(Self {
            path: path.as_ref().to_owned(),
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
            desktop: std::sync::Mutex::new(None),
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
        // Vault::unlock pruned with the 30-day ceiling; apply the user's own retention too.
        if let Some(audit) = s.audit.clone() {
            tokio::task::spawn_blocking(move || audit.prune_now());
        }
        drop(s);
        self.sweep_outputs_in_background(settings);
        Ok(())
    }
    /// Settings with defaults and allowed ranges.
    pub async fn settings(&self) -> Result<SettingsView> {
        let s = self.inner.lock().await;
        let v = s.vault()?;
        Ok(SettingsView::new(v.data().settings))
    }
    /// Validate, encrypt and apply without restart. Preserve pending approval deadlines; reduce concurrency as calls finish and prune excess history/output immediately.
    pub async fn save_settings(&self, settings: Settings) -> Result<SettingsView> {
        settings.validate()?;
        let mut s = self.inner.lock().await;
        let v = s.vault_mut()?;
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
    /// Shorten how long a call waits for a slot (10 s), so tests of the deadline do not wait in real time.
    pub fn set_queue_wait(&self, wait: std::time::Duration) {
        self.limits.set_queue_wait(wait);
    }
    pub async fn drain(&self, within: std::time::Duration) -> bool {
        self.limits.wait_idle(within).await
    }
    pub async fn change_password(&self, current: &str, new: &str) -> Result<()> {
        let database = {
            let s = self.inner.lock().await;
            s.vault()?.database.clone()
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
}

/// One audited Odoo call, as reads, approved writes and the Terminal make it.
pub(crate) struct AuditedCall<'a, A> {
    pub(crate) actor: Option<&'a str>,
    pub(crate) env: &'a Environment,
    /// The environment revision and vault session the call was prepared for.
    pub(crate) revision: u64,
    pub(crate) session: u64,
    pub(crate) operation: &'a str,
    pub(crate) model: &'a str,
    pub(crate) method: &'a str,
    /// Positional arguments for the logged-in uid.
    pub(crate) args: A,
    pub(crate) kwargs: Value,
    /// Errors after sending may mean the write applied ("uncertain"), and its real outcome is kept even if
    /// logging it then fails. A read fails when its outcome cannot be logged.
    pub(crate) write: bool,
    /// An earlier event opened this request (an approval): failures before sending close it as "not_sent".
    pub(crate) recorded: bool,
    pub(crate) max_response: usize,
}

impl Core {
    /// Take a slot, record "attempted", check the environment is unchanged (else "not_sent"), log in (counted
    /// in `up`), run `before_send` (else "not_sent"; it returns the environment to send with), execute, and close
    /// the record with the outcome. The slot is released after the outcome is logged, so drain() waits for it.
    pub(crate) async fn audited_call<A, F, Fut>(
        &self,
        mut audit: AuditRequest,
        call: AuditedCall<'_, A>,
        up: &mut Upstream,
        before_send: F,
    ) -> Result<(Value, usize)>
    where
        A: FnOnce(i64) -> Value,
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<Environment>>,
    {
        let (env, operation, actor) = (call.env, call.operation, call.actor);
        let not_sent = |mut audit: AuditRequest, error: OdxError| async move {
            audit.finish_odx_error(env, &error);
            let _ = self
                .log_outcome_as(audit, actor, &env.id, operation, "not_sent")
                .await;
            Err(error)
        };
        let permits = match self.limits.acquire(&env.id).await {
            Ok(permits) => permits,
            Err(error) if call.recorded => return not_sent(audit, error).await,
            Err(error) => return Err(error),
        };
        if let Err(error) = self
            .log_open(audit.clone(), actor, &env.id, operation, "attempted")
            .await
        {
            return if call.recorded {
                not_sent(audit, error).await
            } else {
                Err(error)
            };
        }
        // The wait for a slot may outlast a lock or an edit; never log in with a stale secret.
        if let Err(error) = self
            .assert_environment_current(&env.id, call.revision, call.session)
            .await
        {
            return not_sent(audit, error).await;
        }
        let uid = match self.odoo.authenticate_measured(env).await {
            Ok((uid, login)) => {
                // Only the call that logged in counts login in its metrics.
                if let Some(bytes) = login {
                    up.calls += 1;
                    up.bytes += bytes;
                }
                uid
            }
            Err(error) => {
                audit.finish_odx_error(env, &error);
                self.log_outcome_as(audit, actor, &env.id, operation, "failed")
                    .await?;
                return Err(error);
            }
        };
        let send_with = match before_send().await {
            Ok(env) => env,
            Err(error) => return not_sent(audit, error).await,
        };
        let result = self
            .odoo
            .execute_kw_limited(
                &send_with,
                uid,
                call.model,
                call.method,
                (call.args)(uid),
                call.kwargs,
                call.max_response,
            )
            .await;
        let result = if call.write {
            result.map_err(crate::odoo::jsonrpc::sent_error)
        } else {
            result
        };
        up.calls += 1;
        match &result {
            Ok((value, _)) => audit.finish(env, Ok(value)),
            Err(error) => audit.finish_odx_error(env, error),
        }
        let outcome = match &result {
            Ok(_) => "succeeded",
            Err(OdxError::Uncertain) => "uncertain",
            Err(_) => "failed",
        };
        let logged = self
            .log_outcome_as(audit, actor, &env.id, operation, outcome)
            .await;
        // A sent write keeps its real outcome; append() set audit_failed, which blocks new operations.
        if !call.write {
            logged?;
        }
        drop(permits);
        let (value, bytes) = result?;
        up.bytes += bytes;
        Ok((value, bytes))
    }
}

#[cfg(test)]
#[path = "../tests/support/mod.rs"]
mod mode_test_support;

#[cfg(test)]
mod mode_tests {
    use super::*;
    use crate::Protocol;
    use crate::{
        clients::Capability,
        config::{EnvironmentDraft, WriteMode},
    };
    use chrono::Utc;

    #[tokio::test]
    async fn temporary_expiry_blocks_a_write_waiting_for_capacity() {
        let tmp = tempfile::tempdir().unwrap();
        let sim = super::mode_test_support::simulator().await;
        let core = Core::new(tmp.path().join("vault.sqlite3")).unwrap();
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
    use crate::approval::revoke_client;
    use crate::{approval::State, config::Environment};
    use chrono::Utc;

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
    use crate::{
        clients::{Capability, ClientIdentity},
        config::{EnvironmentDraft, WriteMode},
    };

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
    async fn an_agent_read_whose_environment_changed_while_queued_is_closed_not_sent() {
        let f = fixture(WriteMode::ReadOnly).await;
        let mut held = Vec::new();
        for _ in 0..4 {
            held.push(f.core.limits.acquire(&f.env).await.unwrap());
        }
        let task = spawn_tool(&f.core, &f.client, "search_read", read());
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
        assert!(task.await.unwrap().is_err());
        let outcomes = outcomes(&f.core).await;
        assert!(
            outcomes
                .iter()
                .all(|(_, outcome)| outcome != "attempted" && outcome != "succeeded"),
            "{outcomes:?}"
        );
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
                    client_id: None,
                    client_revision: None,
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
}
