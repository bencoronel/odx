//! Exercise Core::run_tool through the same path as MCP and HTTP clients.
#![allow(dead_code)]
use odx_core::{
    ActivityFilter, Capability, ClientIdentity, ClientScope, Core, PendingApproval, ToolError,
    ToolOutput,
};
use serde_json::Value;
use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::task::JoinHandle;

pub type Call = JoinHandle<Result<ToolOutput, ToolError>>;

/// Client with all-environment read and write-request access.
pub async fn client(core: &Arc<Core>) -> ClientIdentity {
    static N: AtomicU64 = AtomicU64::new(0);
    let label = format!("fixture-{}", N.fetch_add(1, Ordering::SeqCst));
    let issued = core
        .create_client(
            &label,
            ClientScope::All,
            vec![Capability::OdooRead, Capability::OdooWriteRequest],
        )
        .await
        .unwrap();
    core.authenticate_client(&issued.token).await.unwrap()
}

/// Launch a tool and await its recorded pending approval.
pub async fn queue(
    core: &Arc<Core>,
    client: &ClientIdentity,
    tool: &str,
    arguments: Value,
) -> (Call, PendingApproval) {
    let before: Vec<String> = core
        .approvals()
        .await
        .unwrap()
        .into_iter()
        .map(|p| p.id)
        .collect();
    let call = {
        let (core, client, tool) = (core.clone(), client.clone(), tool.to_string());
        tokio::spawn(async move { core.run_tool(&client, &tool, arguments).await })
    };
    for _ in 0..200 {
        let new = core
            .approvals()
            .await
            .unwrap()
            .into_iter()
            .find(|p| !before.contains(&p.id));
        if let Some(pending) = new {
            let logged = core
                .activity(None, ActivityFilter::default())
                .await
                .unwrap()
                .events
                .iter()
                .any(|e| e.request_id.as_deref() == Some(&pending.id) && e.outcome == "pending");
            if logged {
                return (call, pending);
            }
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("pending approval did not appear");
}

/// Error code from a call launched with queue.
pub async fn error_code(call: Call) -> &'static str {
    call.await.unwrap().unwrap_err().code
}

/// Abandon the wait, as in MCP cancellation; the request must cancel.
pub async fn abandon(core: &Arc<Core>, call: Call, id: &str) {
    call.abort();
    for _ in 0..200 {
        let cancelled = core
            .activity(None, ActivityFilter::default())
            .await
            .unwrap()
            .events
            .iter()
            .any(|e| e.request_id.as_deref() == Some(id) && e.outcome == "cancelled");
        if cancelled {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("abandoned request was not cancelled");
}
