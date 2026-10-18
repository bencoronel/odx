//! Local admin activity queries; requires an unlocked vault.
use crate::{
    audit::AuditDetails,
    error::{OdxError, Result},
    execution::Core,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Deserialize)]
pub struct ActivityFilter {
    #[serde(default)]
    pub environment_id: Option<String>,
    #[serde(default)]
    pub client_id: Option<String>,
    #[serde(default)]
    pub q: Option<String>,
    #[serde(default)]
    pub writes_only: bool,
    #[serde(default)]
    pub errors_only: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ActivityEvent {
    pub request_id: Option<String>,
    pub details: Option<AuditDetails>,
    pub at: DateTime<Utc>,
    pub environment_id: String,
    /// `None` if the environment was deleted.
    pub environment_name: Option<String>,
    pub client_id: Option<String>,
    pub client_label: Option<String>,
    pub operation: String,
    pub outcome: String,
    pub write: bool,
}

/// Last row cursor `(at, activity_key)`; new events cannot shift or duplicate later pages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityCursor {
    pub at: i64,
    pub key: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ActivityPage {
    pub events: Vec<ActivityEvent>,
    pub next_cursor: Option<ActivityCursor>,
    /// False if any history segment was incomplete.
    pub complete: bool,
}

/// Treat `method:model` as a write unless the exact method is a known read.
pub(crate) fn is_write(operation: &str) -> bool {
    let method = operation.split_once(':').map_or(operation, |(m, _)| m);
    !crate::tools::catalog::READ_METHODS.contains(&method)
}
impl Core {
    pub async fn activity(
        &self,
        cursor: Option<ActivityCursor>,
        filter: ActivityFilter,
    ) -> Result<ActivityPage> {
        let (audit, envs, clients) = {
            let s = self.inner.lock().await;
            let v = s.vault.as_ref().ok_or(OdxError::Locked)?;
            let audit = s.audit.clone().ok_or(OdxError::Locked)?;
            let envs: std::collections::HashMap<String, String> = v
                .data()
                .environments
                .iter()
                .map(|e| (e.id.clone(), e.name.clone()))
                .collect();
            let clients: std::collections::HashMap<String, String> = v
                .data()
                .clients
                .iter()
                .map(|c| (c.id.clone(), c.label.clone()))
                .collect();
            (audit, envs, clients)
        };
        let (events, next_cursor, complete) = tokio::task::spawn_blocking({
            let envs = envs.clone();
            let clients = clients.clone();
            move || audit.page(cursor.as_ref(), &filter, &envs, &clients)
        })
        .await
        .map_err(|_| OdxError::Persistence)??;
        let events: Vec<_> = events
            .into_iter()
            .map(|e| ActivityEvent {
                request_id: e.request_id,
                details: e.details,
                environment_name: envs.get(&e.environment_id).cloned(),
                client_label: e.client_id.as_ref().and_then(|c| clients.get(c).cloned()),
                write: is_write(&e.operation),
                at: e.at,
                environment_id: e.environment_id,
                client_id: e.client_id,
                operation: e.operation,
                outcome: e.outcome,
            })
            .collect();
        Ok(ActivityPage {
            next_cursor,
            events,
            complete,
        })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn writes_are_decided_by_the_exact_method_before_the_colon() {
        assert!(!super::is_write("search_read:res.partner"));
        assert!(!super::is_write("read:res.partner"));
        assert!(super::is_write("search_read_and_mark:res.partner"));
        assert!(super::is_write("reader:res.partner"));
        assert!(super::is_write("write:res.partner"));
    }
}
