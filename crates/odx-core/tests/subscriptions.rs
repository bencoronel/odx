//! subscriptions/listen has a separate per-connection cap; capability changes trigger notifications/tools/list_changed.
mod support;
use odx_core::{
    Capability::{OdooRead, OdooWriteRequest},
    Core, EnvironmentDraft, Protocol, WriteMode,
    transports::mcp::McpConnection,
};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tokio::sync::mpsc;

const META: &str = r#""_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}"#;
const SUB: &str = "io.modelcontextprotocol/subscriptionId";

async fn recv(rx: &mut mpsc::Receiver<Value>) -> Value {
    tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("message within deadline")
        .expect("channel open")
}
async fn silent(rx: &mut mpsc::Receiver<Value>) {
    let extra = tokio::time::timeout(Duration::from_millis(150), rx.recv()).await;
    assert!(extra.is_err(), "unexpected message: {extra:?}");
}
fn listen(id: &str, filter: Value) -> Vec<u8> {
    format!(
        r#"{{"jsonrpc":"2.0","id":"{id}","method":"subscriptions/listen","params":{{{META},"notifications":{filter}}}}}"#
    )
    .into_bytes()
}
fn cancel(id: Value) -> Vec<u8> {
    json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":id}})
        .to_string()
        .into_bytes()
}
async fn wait_approvals(core: &Arc<Core>, n: usize) {
    for _ in 0..300 {
        if core.approvals().await.unwrap().len() == n {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("expected {n} pending approvals");
}

#[tokio::test]
async fn listen_is_capped_apart_from_calls_and_notifies_only_changed_lists() {
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
        .unwrap();
    core.set_mode(&env.id, WriteMode::Approval).await.unwrap();
    let writer = core
        .create_client(
            "escritor",
            vec![env.id.clone()],
            vec![OdooRead, OdooWriteRequest],
        )
        .await
        .unwrap();
    let other = core
        .create_client("other", vec![env.id.clone()], vec![OdooRead])
        .await
        .unwrap();
    let (tx, mut rx) = mpsc::channel(64);
    let conn = McpConnection::new(
        core.clone(),
        core.authenticate_client(&writer.token).await.unwrap(),
        tx,
    );
    let (otx, mut orx) = mpsc::channel(64);
    let other_conn = McpConnection::new(
        core.clone(),
        core.authenticate_client(&other.token).await.unwrap(),
        otx,
    );

    let discover =
        format!(r#"{{"jsonrpc":"2.0","id":0,"method":"server/discover","params":{{{META}}}}}"#);
    conn.on_message(discover.as_bytes()).await;
    assert_eq!(
        recv(&mut rx).await["result"]["capabilities"]["tools"]["listChanged"],
        true
    );

    // Seven tool-list subscriptions and one without it; acknowledge only toolsListChanged.
    for i in 1..=7 {
        let id = format!("s{i}");
        conn.on_message(&listen(
            &id,
            json!({"toolsListChanged": true, "promptsListChanged": true}),
        ))
        .await;
        let ack = recv(&mut rx).await;
        assert_eq!(ack["method"], "notifications/subscriptions/acknowledged");
        assert_eq!(ack["params"]["_meta"][SUB], id);
        assert_eq!(
            ack["params"]["notifications"],
            json!({"toolsListChanged": true})
        );
    }
    conn.on_message(&listen("s8", json!({"resourceSubscriptions": ["x"]})))
        .await;
    assert_eq!(recv(&mut rx).await["params"]["notifications"], json!({}));
    conn.on_message(&listen("s9", json!({"toolsListChanged": true})))
        .await;
    let r = recv(&mut rx).await;
    assert_eq!(r["id"], "s9");
    assert_eq!(
        r["error"]["code"], -31000,
        "application code outside -32768..-32000"
    );

    // Subscriptions do not consume call slots; 16 calls still fit.
    for id in 1..=17 {
        let call = format!(
            r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{{META},"name":"write","arguments":{{"env":"A","model":"res.partner","ids":[1],"values":{{"name":"x"}}}}}}}}"#
        );
        conn.on_message(call.as_bytes()).await;
    }
    let r = recv(&mut rx).await;
    assert_eq!(r["id"], 17);
    assert!(
        r["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("capacity_exceeded")
    );
    wait_approvals(&core, 16).await;

    // Saving an unchanged tool list revokes requests without notifying.
    core.update_client(
        &writer.client.id,
        "writer renamed",
        vec![env.id.clone()],
        vec![OdooRead, OdooWriteRequest],
    )
    .await
    .unwrap();
    let mut revoked = vec![];
    for _ in 1..=16 {
        let r = recv(&mut rx).await;
        assert!(
            r["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("approval_revoked"),
            "{r}"
        );
        revoked.push(r["id"].as_u64().unwrap());
    }
    revoked.sort();
    assert_eq!(revoked, (1..=16).collect::<Vec<_>>());
    silent(&mut rx).await;

    // Cancellation sends nothing and releases the subscription slot.
    conn.on_message(&cancel(json!("s1"))).await;
    silent(&mut rx).await;
    conn.on_message(&listen("s9", json!({"toolsListChanged": true})))
        .await;
    assert_eq!(recv(&mut rx).await["params"]["_meta"][SUB], "s9");

    other_conn
        .on_message(&listen("o1", json!({"toolsListChanged": true})))
        .await;
    recv(&mut orx).await;

    // Removing write access notifies only that client's interested subscribers.
    core.update_client(
        &writer.client.id,
        "writer renamed",
        vec![env.id.clone()],
        vec![OdooRead],
    )
    .await
    .unwrap();
    let mut notified = vec![];
    for _ in 0..7 {
        let n = recv(&mut rx).await;
        assert_eq!(n["method"], "notifications/tools/list_changed");
        assert!(n.get("id").is_none());
        notified.push(n["params"]["_meta"][SUB].as_str().unwrap().to_string());
    }
    notified.sort();
    assert_eq!(notified, ["s2", "s3", "s4", "s5", "s6", "s7", "s9"]);
    silent(&mut rx).await;
    silent(&mut orx).await;

    let list = format!(r#"{{"jsonrpc":"2.0","id":"l","method":"tools/list","params":{{{META}}}}}"#);
    conn.on_message(list.as_bytes()).await;
    let names: Vec<_> = recv(&mut rx).await["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    assert!(!names.contains(&"write".into()));
    assert_eq!(sim.counts.writes(), 0);
}
