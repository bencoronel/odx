mod lifecycle;
pub(crate) use lifecycle::{ApprovalEnd, revoke_all, revoke_client, revoke_environment};
pub use lifecycle::{ApprovalNotice, PendingApproval};

use crate::{
    audit::AuditDetails,
    config::Environment,
    error::{OdxError, Result},
};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[cfg(test)]
thread_local! {
    /// Request lifetime for single-threaded crate tests.
    pub(crate) static TTL: std::cell::Cell<Duration> = const { std::cell::Cell::new(Duration::minutes(5)) };
}
fn ttl(configured: Duration) -> Duration {
    #[cfg(test)]
    return TTL.with(|t| t.get()).min(configured);
    #[cfg(not(test))]
    configured
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalCall {
    pub environment_id: String,
    pub model: String,
    pub method: String,
    pub args: Value,
    pub kwargs: Value,
    pub revision: u64,
    pub expires_at: DateTime<Utc>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Approve,
    Reject,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Pending,
    Approved,
    /// Sent to Odoo; cannot be revoked or expire.
    Sent,
    Rejected,
    Expired,
    Revoked,
}
#[derive(Debug, Clone)]
pub struct Approval {
    pub id: String,
    /// Originating client; `None` for local administration.
    pub client_id: Option<String>,
    pub call: ApprovalCall,
    pub created_at: DateTime<Utc>,
    pub state: State,
    /// Redacted audit request, including requests closed without sending.
    pub details: AuditDetails,
    /// Require a human decision unless a rule or free-write mode authorizes the call.
    pub authorization: crate::policy::Authorization,
}
impl Approval {
    #[cfg(test)]
    pub fn new(
        env: &Environment,
        model: String,
        method: String,
        args: Value,
        kwargs: Value,
        now: DateTime<Utc>,
    ) -> Self {
        Self::with_ttl(env, model, method, args, kwargs, Duration::minutes(5), now)
    }
    /// Use the configured lifetime (default 5 minutes). `now` comes from the vault clock, which never goes back.
    pub(crate) fn with_ttl(
        env: &Environment,
        model: String,
        method: String,
        args: Value,
        kwargs: Value,
        ttl_configured: Duration,
        now: DateTime<Utc>,
    ) -> Self {
        let details = AuditDetails::call(env, &model, &method, &args, &kwargs);
        Self {
            id: Uuid::new_v4().to_string(),
            client_id: None,
            details,
            call: ApprovalCall {
                environment_id: env.id.clone(),
                model,
                method,
                args,
                kwargs,
                revision: env.revision,
                expires_at: now + ttl(ttl_configured),
            },
            created_at: now,
            state: State::Pending,
            authorization: crate::policy::Authorization::Individual,
        }
    }
    pub fn decide(&mut self, d: Decision, now: DateTime<Utc>) -> Result<()> {
        if self.state != State::Pending {
            return Err(OdxError::ApprovalUnavailable);
        }
        if now >= self.call.expires_at {
            self.state = State::Expired;
            return Err(OdxError::ApprovalUnavailable);
        }
        self.state = match d {
            Decision::Approve => State::Approved,
            Decision::Reject => State::Rejected,
        };
        Ok(())
    }
    /// Revoke unsent requests; return whether a decision was pending. Approved requests close through dispatch as not sent.
    pub fn revoke(&mut self) -> bool {
        let pending = self.state == State::Pending;
        if pending || self.state == State::Approved {
            self.state = State::Revoked;
        }
        pending
    }
    pub fn expire_if_due(&mut self, now: DateTime<Utc>) {
        if self.state == State::Pending && now >= self.call.expires_at {
            self.state = State::Expired;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Environment, Protocol, WriteMode};
    use serde_json::json;
    #[test]
    fn decision_expires_and_cannot_be_repeated() {
        let env = Environment {
            disabled_context: Default::default(),
            headers: vec![],
            id: "e".into(),
            name: "fixture".into(),
            url: "http://localhost".into(),
            database: "db".into(),
            login: "user".into(),
            protocol: Protocol::LegacyJsonrpc,
            detected_protocol: None,
            context: json!({}),
            write_mode: WriteMode::Approval,
            write_until: None,
            revision: 1,
            secret: crate::security::Secret::new("fixture-only").unwrap(),
            tags: vec![],
            proxy: Default::default(),
        };
        let mut approval = Approval::new(
            &env,
            "res.partner".into(),
            "write".into(),
            json!([[1],{"name":"x"}]),
            json!({}),
            Utc::now(),
        );
        approval.call.expires_at = Utc::now() - Duration::seconds(1);
        assert!(approval.decide(Decision::Approve, Utc::now()).is_err());
        assert_eq!(approval.state, State::Expired);
        assert!(approval.decide(Decision::Reject, Utc::now()).is_err());
    }
    #[test]
    fn expiry_follows_the_given_clock_not_the_system_clock() {
        let env = Environment::default();
        let created = Utc::now();
        let call = |now| {
            let mut a = Approval::new(
                &env,
                "m".into(),
                "write".into(),
                json!([]),
                json!({}),
                created,
            );
            a.decide(Decision::Approve, now)
        };
        // A system clock moved back an hour would still be "before expiry"; the vault clock is not.
        assert!(call(created + Duration::minutes(4)).is_ok());
        assert!(call(created + Duration::minutes(5)).is_err());
    }
    #[test]
    fn revocation_is_an_explicit_terminal_state() {
        let env = Environment {
            disabled_context: Default::default(),
            headers: vec![],
            id: "e".into(),
            name: "fixture".into(),
            url: "http://localhost".into(),
            database: "db".into(),
            login: "user".into(),
            protocol: Protocol::LegacyJsonrpc,
            detected_protocol: None,
            context: json!({}),
            write_mode: WriteMode::Approval,
            write_until: None,
            revision: 1,
            secret: crate::security::Secret::new("fixture-only").unwrap(),
            tags: vec![],
            proxy: Default::default(),
        };
        let mut approval = Approval::new(
            &env,
            "res.partner".into(),
            "write".into(),
            json!([]),
            json!({}),
            Utc::now(),
        );
        assert!(approval.revoke());
        assert_eq!(approval.state, State::Revoked);
        approval.state = State::Sent;
        assert!(!approval.revoke());
        assert_eq!(approval.state, State::Sent);
        assert!(approval.decide(Decision::Approve, Utc::now()).is_err());
    }
}
