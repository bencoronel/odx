//! Approval lifecycle on the core: register a write and wait (`submit`), list pending requests, decide,
//! and dispatch an approved write exactly once; plus the revocations that end pending requests.
use crate::{
    approval::{Approval, Decision, State},
    audit::{AuditDetails, AuditLog, AuditRequest},
    clients::ClientIdentity,
    config::Environment,
    error::{OdxError, Result},
    execution::{AuditedCall, Core, Inner, Upstream},
    output::{Outcome, ToolError},
    policy::{self, Effect},
    tools::catalog::Target,
    vault::Vault,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::oneshot;

/// Revoke unsent requests matching `filter`; return those awaiting a decision.
pub(crate) fn revoke_where(
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

pub(crate) fn revoke_client(
    pending: &mut HashMap<String, Approval>,
    client_id: &str,
) -> Vec<String> {
    revoke_where(pending, |a| a.client_id.as_deref() == Some(client_id))
}

/// Revalidate authorization immediately before dispatch.
pub(crate) fn authorization_current(
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

pub(crate) fn revoke_all(pending: &mut HashMap<String, Approval>) -> Vec<String> {
    revoke_where(pending, |_| true)
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

/// New client request notice for desktop notifications.
#[derive(Debug, Clone, Serialize)]
pub struct ApprovalNotice {
    pub approval_id: String,
    pub client_label: Option<String>,
    pub environment_name: String,
    pub model: String,
    pub method: String,
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

impl Core {
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
    /// Oldest first: a stable order, so a new request never moves into the row the user is about to decide.
    pub async fn approvals(&self) -> Result<Vec<PendingApproval>> {
        let mut guard = self.inner.lock().await;
        let s = &mut *guard;
        // Field access, not vault(): pending is borrowed mutably alongside it.
        let v = s.vault.as_ref().ok_or(OdxError::Locked)?;
        let now = v.now();
        for approval in s.pending.values_mut() {
            approval.expire_if_due(now);
        }
        let mut list: Vec<PendingApproval> = s
            .pending
            .values()
            .filter(|p| p.state == crate::approval::State::Pending)
            .filter_map(|p| {
                let e = v
                    .data()
                    .environments
                    .iter()
                    .find(|e| e.id == p.call.environment_id)?;
                Some(PendingApproval {
                    id: p.id.clone(),
                    client_id: p.client_id.clone(),
                    client_label: p.client_id.as_ref().and_then(|c| {
                        v.data()
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
            .collect();
        list.sort_by(|a, b| (a.created_at, &a.id).cmp(&(b.created_at, &b.id)));
        Ok(list)
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
        let v = s.vault()?;
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
        let audit = self.audit_request(id, details).await?;
        if !approve {
            self.log_outcome_as(audit, client, &env.id, &operation, "rejected")
                .await?;
            return Ok((json!({"status":"rejected"}), up));
        }
        let session = self.inner.lock().await.session;
        let env = self.resolve_protocol(env, session).await?;
        let (args, kwargs) = (call.args.clone(), call.kwargs.clone());
        let audited = AuditedCall {
            actor: client,
            env: &env,
            revision: call.revision,
            session,
            operation: &operation,
            model: &call.model,
            method: &call.method,
            args: move |_| args,
            kwargs,
            write: true,
            recorded: true,
            max_response: crate::odoo::jsonrpc::MAX_RESPONSE,
        };
        // The sent transition shares the lock used by vault_lock and revision changes.
        let (value, _) = self
            .audited_call(audit, audited, &mut up, || async {
                let mut s = self.inner.lock().await;
                if s.audit_failed {
                    return Err(OdxError::Persistence);
                }
                let v = s.vault()?;
                let latest = v.data().connection_environment(&env.id)?;
                // Revalidate against current mode, deadline and rule.
                let authorized = authorization_current(v, &latest, &call, &authorization);
                let now = v.now();
                let pending = s.pending.get_mut(id).ok_or(OdxError::ApprovalUnavailable)?;
                // A human decision was already checked against the deadline when it was taken; a slow
                // login or a wait for a slot after it must not void it. Free and rule authorizations still expire.
                let individual = matches!(authorization, policy::Authorization::Individual);
                if pending.state != State::Approved
                    || latest.revision != call.revision
                    || (call.expires_at <= now && !individual)
                    || !authorized
                {
                    return Err(OdxError::ApprovalUnavailable);
                }
                // Sent requests remain until the outcome is known, regardless of revocation or expiry.
                pending.state = State::Sent;
                Ok(latest)
            })
            .await?;
        Ok((value, up))
    }
    /// Register, execute if permitted or await human approval. Return result, upstream metrics and human wait time (zero in free-write mode).
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn submit(
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
            let v = s.vault()?;
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
                let _ = self.notices.send(ApprovalNotice {
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
