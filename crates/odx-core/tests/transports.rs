//! MCP stdio (connector→socket→app) and HTTP share the core.
mod support;
use odx_core::{
    Capability, Core,
    transports::{connector, http, socket},
};
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const META: &str = r#""_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{},"io.modelcontextprotocol/clientInfo":{"name":"admin","version":"1"}}"#;

struct App {
    core: Arc<Core>,
    sim: support::Simulator,
    socket: PathBuf,
    reader_token: String,
    _tmp: tempfile::TempDir,
}

async fn app() -> App {
    let tmp = tempfile::tempdir().unwrap();
    let sim = support::simulator().await;
    let core = support::unlocked_core(&tmp).await;
    let mut ids = vec![];
    for name in ["A", "B"] {
        ids.push(
            core.save_environment(support::draft(name, &sim.url))
                .await
                .unwrap()
                .id,
        );
    }
    let reader = core
        .create_client("lector", vec![ids[0].clone()], vec![Capability::OdooRead])
        .await
        .unwrap();
    let socket = tmp.path().join("run").join("core.sock");
    let listener = socket::bind(&socket).await.unwrap();
    tokio::spawn(socket::serve(core.clone(), listener));
    let mut stream = tokio::net::UnixStream::connect(&socket).await.unwrap();
    socket::write_frame(
        &mut stream,
        &json!({"v":socket::WIRE_VERSION,"kind":"hello"}),
    )
    .await
    .unwrap();
    assert_eq!(
        socket::read_frame(&mut stream).await.unwrap().unwrap()["kind"],
        "ready"
    );
    drop(stream);
    for _ in 0..50 {
        if core.mcp_connections().await.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let local = core
        .clients()
        .await
        .unwrap()
        .into_iter()
        .find(|c| c.local_stdio)
        .unwrap();
    core.update_client(
        &local.id,
        "MCP local",
        vec![ids[0].clone()],
        vec![Capability::OdooRead],
    )
    .await
    .unwrap();
    App {
        core,
        sim,
        socket,
        reader_token: reader.token.clone(),
        _tmp: tmp,
    }
}

/// Run the connector with in-memory stdin/stdout; return input, output lines and task.
async fn connect(
    app: &App,
) -> (
    tokio::io::DuplexStream,
    tokio::io::Lines<BufReader<tokio::io::DuplexStream>>,
    tokio::task::JoinHandle<Result<(), String>>,
) {
    let (stdin_w, stdin_r) = tokio::io::duplex(1 << 20);
    let (stdout_w, stdout_r) = tokio::io::duplex(1 << 20);
    let path = app.socket.clone();
    let task =
        tokio::spawn(async move { connector::run(&path, BufReader::new(stdin_r), stdout_w).await });
    (stdin_w, BufReader::new(stdout_r).lines(), task)
}

async fn send(stdin: &mut tokio::io::DuplexStream, line: &str) {
    stdin.write_all(line.as_bytes()).await.unwrap();
    stdin.write_all(b"\n").await.unwrap();
}
async fn next(lines: &mut tokio::io::Lines<BufReader<tokio::io::DuplexStream>>) -> Value {
    let line = tokio::time::timeout(std::time::Duration::from_secs(5), lines.next_line())
        .await
        .expect("response within deadline")
        .unwrap()
        .expect("line");
    serde_json::from_str(&line).expect("stdout contains JSON-RPC only")
}

#[tokio::test]
async fn mcp_over_connector_is_stateless_scoped_and_strict() {
    let app = app().await;
    let (mut stdin, mut out, task) = connect(&app).await;

    send(
        &mut stdin,
        &format!(r#"{{"jsonrpc":"2.0","id":1,"method":"server/discover","params":{{{META}}}}}"#),
    )
    .await;
    let r = next(&mut out).await;
    assert_eq!(r["result"]["supportedVersions"], json!(["2026-07-28"]));
    assert_eq!(r["result"]["resultType"], "complete");
    assert_eq!(
        r["result"]["capabilities"],
        json!({"tools":{"listChanged":true}})
    );
    assert!(r["result"]["ttlMs"].is_number() && r["result"]["cacheScope"].is_string());

    // Missing _meta, unsupported version and invalid JSON.
    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
    )
    .await;
    assert_eq!(next(&mut out).await["error"]["code"], -32602);
    send(&mut stdin, r#"{"jsonrpc":"2.0","id":3,"method":"tools/list","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2025-06-18","io.modelcontextprotocol/clientCapabilities":{}}}}"#).await;
    let r = next(&mut out).await;
    assert_eq!(r["error"]["code"], -32022);
    assert_eq!(r["error"]["data"]["supported"], json!(["2026-07-28"]));
    send(&mut stdin, "{not json").await;
    assert_eq!(next(&mut out).await["error"]["code"], -32700);
    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":5,"id":6,"method":"tools/list"}"#,
    )
    .await;
    assert_eq!(
        next(&mut out).await["error"]["code"],
        -32700,
        "duplicate keys"
    );

    // Grant-filtered lists hide write tools from readers.
    send(
        &mut stdin,
        &format!(r#"{{"jsonrpc":"2.0","id":"l","method":"tools/list","params":{{{META}}}}}"#),
    )
    .await;
    let r = next(&mut out).await;
    let names: Vec<_> = r["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    assert!(names.contains(&"search_read".into()) && names.contains(&"call".into()));
    assert!(!names.contains(&"write".into()) && !names.contains(&"unlink".into()));
    assert_eq!(r["result"]["cacheScope"], "private");

    // Compact success, structured JSON, tool errors and unknown tools.
    send(&mut stdin, &format!(r#"{{"jsonrpc":"2.0","id":10,"method":"tools/call","params":{{{META},"name":"search","arguments":{{"env":"A","model":"res.partner","limit":2}}}}}}"#)).await;
    let r = next(&mut out).await;
    assert_eq!(r["result"]["isError"], false);
    assert!(
        r["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("next_offset=2")
    );
    send(&mut stdin, &format!(r#"{{"jsonrpc":"2.0","id":11,"method":"tools/call","params":{{{META},"name":"search_count","arguments":{{"env":"A","model":"res.partner","format":"json"}}}}}}"#)).await;
    let r = next(&mut out).await;
    assert_eq!(r["result"]["structuredContent"]["data"], 5);
    assert_eq!(
        serde_json::from_str::<Value>(r["result"]["content"][0]["text"].as_str().unwrap()).unwrap(),
        r["result"]["structuredContent"]
    );
    send(&mut stdin, &format!(r#"{{"jsonrpc":"2.0","id":12,"method":"tools/call","params":{{{META},"name":"search","arguments":{{"env":"B","model":"res.partner"}}}}}}"#)).await;
    let r = next(&mut out).await;
    assert_eq!(r["result"]["isError"], true);
    assert!(
        r["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("environment_unavailable")
    );
    send(&mut stdin, &format!(r#"{{"jsonrpc":"2.0","id":13,"method":"tools/call","params":{{{META},"name":"nope","arguments":{{}}}}}}"#)).await;
    assert_eq!(
        next(&mut out).await["error"]["code"],
        -32602,
        "unknown tool"
    );

    // clientInfo cannot change identity or grant access to B.
    let spoof = r#""_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{},"io.modelcontextprotocol/clientInfo":{"name":"escritor","version":"1"}}"#;
    send(&mut stdin, &format!(r#"{{"jsonrpc":"2.0","id":14,"method":"tools/call","params":{{{spoof},"name":"envs","arguments":{{"format":"json"}}}}}}"#)).await;
    let r = next(&mut out).await;
    assert_eq!(
        r["result"]["structuredContent"]["data"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    let open = app.core.mcp_connections().await;
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].client_label.as_deref(), Some("MCP local"));
    assert!(open[0].calls > 0, "tool calls counted");
    // The connector runs in this test process, so the agent is its parent.
    #[cfg(target_os = "linux")]
    assert_eq!(open[0].agent.pid, Some(std::os::unix::process::parent_id()));
    let reader = app
        .core
        .clients()
        .await
        .unwrap()
        .into_iter()
        .find(|c| c.local_stdio)
        .unwrap();
    assert!(reader.last_used_at.is_some(), "last use recorded");

    drop(stdin); // EOF ends the connector without closing the app.
    assert!(task.await.unwrap().is_ok());
    assert_eq!(app.core.status().await, "unlocked");
    for _ in 0..50 {
        if app.core.mcp_connections().await.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(
        app.core.mcp_connections().await.is_empty(),
        "connection removed after EOF"
    );
}

#[tokio::test]
async fn the_app_can_close_one_connection() {
    let app = app().await;
    let (_stdin, _out, task) = connect(&app).await;
    let (_stdin2, _out2, task2) = connect(&app).await;
    for _ in 0..200 {
        if app.core.mcp_connections().await.len() == 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let open = app.core.mcp_connections().await;
    assert!(app.core.close_connection(open[0].id));
    assert!(!app.core.close_connection(u64::MAX), "unknown id");
    // Only one connector exits, with the reason shown to its agent.
    let ended = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        tokio::select! {
            r = task => (0, r),
            r = task2 => (1, r),
        }
    })
    .await
    .expect("a connector exits");
    assert!(
        ended
            .1
            .unwrap()
            .unwrap_err()
            .contains("closed the connection")
    );
    for _ in 0..50 {
        if app.core.mcp_connections().await.len() == 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let left = app.core.mcp_connections().await;
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].id, open[1].id);
}

#[tokio::test]
async fn cancelling_or_disconnecting_during_approval_sends_nothing() {
    let app = app().await;
    let profile = app
        .core
        .clients()
        .await
        .unwrap()
        .into_iter()
        .find(|c| c.local_stdio)
        .unwrap();
    app.core
        .update_client(
            &profile.id,
            &profile.label,
            odx_core::ClientScope::All,
            vec![Capability::OdooRead, Capability::OdooWriteRequest],
        )
        .await
        .unwrap();
    let (mut stdin, mut out, task) = connect(&app).await;
    let env_a = app.core.environments().await.unwrap()[0].id.clone();
    app.core
        .set_mode(&env_a, odx_core::WriteMode::Approval)
        .await
        .unwrap();
    let write = |id: u32| {
        format!(
            r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{{META},"name":"write","arguments":{{"env":"A","model":"res.partner","ids":[1],"values":{{"name":"x"}}}}}}}}"#
        )
    };

    send(&mut stdin, &write(1)).await;
    wait_pending(&app.core).await;
    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":1}}"#,
    )
    .await;
    wait_empty(&app.core).await;

    send(&mut stdin, &write(2)).await;
    wait_pending(&app.core).await;
    drop(stdin);
    assert!(task.await.unwrap().is_ok());
    wait_empty(&app.core).await;
    assert_eq!(app.sim.counts.writes(), 0);
    // Cancelled requests produce no stdout output.
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), out.next_line())
            .await
            .map_or(true, |l| l.unwrap().is_none())
    );
}

#[tokio::test]
async fn connector_needs_no_token_and_fails_finitely_with_locked_or_absent_app() {
    let app = app().await;
    let mut old = tokio::net::UnixStream::connect(&app.socket).await.unwrap();
    socket::write_frame(&mut old, &json!({"v":1,"kind":"auth","token":"ignored"}))
        .await
        .unwrap();
    assert_eq!(
        socket::read_frame(&mut old).await.unwrap().unwrap()["code"],
        "incompatible_connector"
    );
    app.core.lock().await;
    let (mut stdin, mut out, task) = connect(&app).await;
    // The client sees the reason in its first request's error, not just a closed process.
    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
    )
    .await;
    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":"i","method":"initialize","params":{}}"#,
    )
    .await;
    let r = next(&mut out).await;
    assert_eq!(
        (r["id"].as_str(), r["error"]["code"].as_i64()),
        (Some("i"), Some(-32000))
    );
    assert!(
        r["error"]["message"].as_str().unwrap().contains("locked"),
        "{r}"
    );
    assert!(task.await.unwrap().unwrap_err().contains("locked"));
    let missing = app.socket.with_file_name("other.sock");
    let (w, r) = tokio::io::duplex(64);
    drop(w); // EOF: nothing to answer.
    let (w2, _r2) = tokio::io::duplex(64);
    let err = connector::run(&missing, BufReader::new(r), w2)
        .await
        .unwrap_err();
    assert!(err.contains("is not open"));
}

#[tokio::test]
async fn http_api_checks_host_origin_bearer_and_matches_mcp() {
    let app = app().await;
    let listener = http::bind(0).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(http::serve(app.core.clone(), listener, async {
        let _ = stop_rx.await;
    }));
    let base = format!("http://127.0.0.1:{port}/api/v1");
    let client = reqwest::Client::new();
    let body = json!({"env":"A","model":"res.partner","limit":2});

    let r = client
        .post(format!("{base}/search"))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    assert_eq!(
        r.json::<Value>().await.unwrap()["error"]["outcome"],
        "not_sent"
    );
    let r = client
        .post(format!("{base}/search"))
        .bearer_auth("odx_x")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    let r = client
        .post(format!("{base}/search"))
        .bearer_auth(&app.reader_token)
        .header("Origin", "https://evil.example")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    let r = client
        .post(format!("{base}/search"))
        .bearer_auth(&app.reader_token)
        .header("Origin", "null")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    let r = client
        .post(format!("{base}/search"))
        .bearer_auth(&app.reader_token)
        .header("Host", format!("evil.example:{port}"))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    let r = client
        .get(format!("{base}/search"))
        .bearer_auth(&app.reader_token)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 405);
    let r = client
        .post(format!("{base}/search"))
        .bearer_auth(&app.reader_token)
        .header("Content-Type", "text/plain")
        .body(body.to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);
    let r = client
        .post(format!("{base}/nope"))
        .bearer_auth(&app.reader_token)
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);
    let r = client
        .post(format!("{base}/write"))
        .bearer_auth(&app.reader_token)
        .json(&json!({"env":"A","model":"res.partner","ids":[1],"values":{"a":1}}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    assert_eq!(
        app.sim.counts.queries() + app.sim.counts.writes(),
        0,
        "rejected calls do not reach Odoo"
    );

    let r = client
        .post(format!("{base}/search"))
        .bearer_auth(&app.reader_token)
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let api: Value = r.json().await.unwrap();
    assert_eq!(api["data"], json!([1, 2]));
    // The scheme is case-insensitive.
    let r = client
        .post(format!("{base}/search"))
        .header("Authorization", format!("bearer {}", app.reader_token))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);

    // MCP parity for the same operation and grant.
    let (mut stdin, mut out, _task) = connect(&app).await;
    send(&mut stdin, &format!(r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{{META},"name":"search","arguments":{{"env":"A","model":"res.partner","limit":2,"format":"json"}}}}}}"#)).await;
    let mcp = next(&mut out).await;
    let mut a = api.clone();
    let mut m = mcp["result"]["structuredContent"].clone();
    // The first call counts login; MCP reuses the session.
    assert_eq!(
        (&a["meta"]["upstream_calls"], &m["meta"]["upstream_calls"]),
        (&json!(2), &json!(1))
    );
    for v in [&mut a, &mut m] {
        let meta = v["meta"].as_object_mut().unwrap();
        for key in ["upstream_ms", "upstream_calls", "upstream_bytes"] {
            meta.remove(key);
        }
    }
    assert_eq!(a, m);

    // Lock invalidates tokens; stopping the listener closes it.
    app.core.lock().await;
    let r = client
        .post(format!("{base}/search"))
        .bearer_auth(&app.reader_token)
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 503);
    let _ = stop_tx.send(());
    server.await.unwrap();
    assert!(
        client
            .post(format!("{base}/search"))
            .bearer_auth(&app.reader_token)
            .json(&body)
            .send()
            .await
            .is_err()
    );
}

#[tokio::test]
async fn connector_rejects_line_whose_frame_exceeds_limit_and_keeps_session() {
    let app = app().await;
    let (mut stdin, mut out, task) = connect(&app).await;
    // The line fits 1 MiB, but its socket envelope does not.
    let prefix = r#"{"jsonrpc":"2.0","id":5,"method":"server/discover","params":{"pad":""#;
    let pad = "a".repeat(socket::MAX_FRAME - prefix.len() - 3);
    send(&mut stdin, &format!("{prefix}{pad}\"}}}}")).await;
    let r = next(&mut out).await;
    assert_eq!(r["id"], 5);
    assert_eq!(r["error"]["code"], -32600);
    send(
        &mut stdin,
        &format!(r#"{{"jsonrpc":"2.0","id":6,"method":"server/discover","params":{{{META}}}}}"#),
    )
    .await;
    assert_eq!(next(&mut out).await["id"], 6);
    drop(stdin);
    assert!(task.await.unwrap().is_ok());
}

async fn wait_pending(core: &Arc<Core>) {
    for _ in 0..200 {
        if !core.approvals().await.unwrap().is_empty() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("no pending approval");
}
async fn wait_empty(core: &Arc<Core>) {
    for _ in 0..200 {
        if core.approvals().await.unwrap().is_empty() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("approval is still pending");
}

async fn tool_names(
    stdin: &mut tokio::io::DuplexStream,
    out: &mut tokio::io::Lines<BufReader<tokio::io::DuplexStream>>,
) -> Vec<String> {
    send(
        stdin,
        &format!(r#"{{"jsonrpc":"2.0","id":"l","method":"tools/list","params":{{{META}}}}}"#),
    )
    .await;
    next(out).await["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn mcp_tools_follow_the_current_grant_without_reconnecting() {
    use Capability::{OdooRead, OdooWriteRequest};
    let app = app().await;
    let profile = app
        .core
        .clients()
        .await
        .unwrap()
        .into_iter()
        .find(|c| c.local_stdio)
        .unwrap();
    let grant = |caps: Vec<Capability>| {
        let core = app.core.clone();
        let (id, label) = (profile.id.clone(), profile.label.clone());
        async move {
            core.update_client(&id, &label, odx_core::ClientScope::All, caps)
                .await
                .unwrap();
        }
    };
    let env_a = app.core.environments().await.unwrap()[0].id.clone();
    app.core
        .set_mode(&env_a, odx_core::WriteMode::Permanent)
        .await
        .unwrap();
    grant(vec![OdooRead, OdooWriteRequest]).await;
    let (mut stdin, mut out, _task) = connect(&app).await;
    send(
        &mut stdin,
        &format!(
            r#"{{"jsonrpc":"2.0","id":"annotations","method":"tools/list","params":{{{META}}}}}"#
        ),
    )
    .await;
    let published = next(&mut out).await;
    let published = published["result"]["tools"].as_array().unwrap();
    let write_definition = published
        .iter()
        .find(|tool| tool["name"] == "write")
        .unwrap();
    assert_eq!(write_definition["annotations"]["destructiveHint"], true);
    assert_eq!(write_definition["annotations"]["readOnlyHint"], false);
    let fields_definition = published
        .iter()
        .find(|tool| tool["name"] == "fields")
        .unwrap();
    assert!(fields_definition["inputSchema"]["properties"]["names"]["description"].is_string());
    let write = |id: u32| {
        format!(
            r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{{META},"name":"write","arguments":{{"env":"A","model":"res.partner","ids":[1],"values":{{"name":"x"}}}}}}}}"#
        )
    };
    assert!(
        tool_names(&mut stdin, &mut out)
            .await
            .contains(&"write".into())
    );

    grant(vec![OdooRead]).await;
    assert!(
        !tool_names(&mut stdin, &mut out)
            .await
            .contains(&"write".into())
    );
    send(&mut stdin, &write(1)).await;
    let r = next(&mut out).await;
    assert_eq!(
        r["result"]["isError"], true,
        "rejected by the core before transport"
    );
    assert!(
        r["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("forbidden")
    );
    assert_eq!(app.sim.counts.writes(), 0);

    grant(vec![OdooRead, OdooWriteRequest]).await;
    assert!(
        tool_names(&mut stdin, &mut out)
            .await
            .contains(&"write".into())
    );
    send(&mut stdin, &write(2)).await;
    assert_eq!(next(&mut out).await["result"]["isError"], false);
    assert_eq!(app.sim.counts.writes(), 1);
}

#[tokio::test]
async fn initialize_opens_a_2025_06_18_session_with_the_same_tools_and_grant() {
    use Capability::{OdooRead, OdooWriteRequest};
    let app = app().await;
    let (mut stdin, mut out, _task) = connect(&app).await;
    send(&mut stdin, r#"{"jsonrpc":"2.0","id":0,"method":"ping"}"#).await;
    assert_eq!(
        next(&mut out).await["result"],
        json!({}),
        "ping before initialize"
    );
    // Before initialize, requests keep the 2026-07-28 rules.
    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
    )
    .await;
    assert_eq!(next(&mut out).await["error"]["code"], -32602);

    send(&mut stdin, r#"{"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{"elicitation":{}},"clientInfo":{"name":"escritor","version":"1"}}}"#).await;
    let r = next(&mut out).await;
    assert_eq!(r["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(
        r["result"]["capabilities"],
        json!({"tools":{"listChanged":true}})
    );
    assert_eq!(r["result"]["serverInfo"]["name"], "odx");
    assert!(r["result"]["instructions"].is_string());
    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
    )
    .await;
    assert!(silent(&mut out).await, "notifications get no response");

    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/list"}"#,
    )
    .await;
    let r = next(&mut out).await;
    let names: Vec<_> = r["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    assert!(names.contains(&"search_read".into()) && !names.contains(&"write".into()));
    for field in ["resultType", "_meta", "ttlMs", "cacheScope"] {
        assert!(
            r["result"].get(field).is_none(),
            "no 2026-07-28 field {field}"
        );
    }

    // clientInfo cannot widen the grant: only environment A is visible.
    send(&mut stdin, r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"envs","arguments":{"format":"json"}}}"#).await;
    let r = next(&mut out).await;
    assert_eq!(r["result"]["isError"], false);
    assert!(r["result"].get("resultType").is_none());
    assert_eq!(
        r["result"]["structuredContent"]["data"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    send(&mut stdin, r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"search","arguments":{"env":"B","model":"res.partner"}}}"#).await;
    assert_eq!(next(&mut out).await["result"]["isError"], true);

    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":6,"method":"server/discover","params":{}}"#,
    )
    .await;
    assert_eq!(
        next(&mut out).await["error"]["code"],
        -32601,
        "2026-07-28 methods stay out of the session"
    );
    send(&mut stdin, r#"{"jsonrpc":"2.0","id":7,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"x","version":"1"}}}"#).await;
    assert_eq!(next(&mut out).await["error"]["code"], -32600);

    // Saved permission changes announce a new list without a subscription.
    let profile = app
        .core
        .clients()
        .await
        .unwrap()
        .into_iter()
        .find(|c| c.local_stdio)
        .unwrap();
    app.core
        .update_client(
            &profile.id,
            &profile.label,
            odx_core::ClientScope::All,
            vec![OdooRead, OdooWriteRequest],
        )
        .await
        .unwrap();
    assert_eq!(
        next(&mut out).await,
        json!({"jsonrpc":"2.0","method":"notifications/tools/list_changed"})
    );
    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":8,"method":"tools/list"}"#,
    )
    .await;
    assert!(
        next(&mut out).await["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["name"] == "write")
    );
}

#[tokio::test]
async fn http_body_over_limit_gets_json_413_before_authentication() {
    let app = app().await;
    let listener = http::bind(0).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(http::serve(
        app.core.clone(),
        listener,
        std::future::pending(),
    ));
    let r = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{port}/api/v1/search"))
        .header("Content-Type", "application/json")
        .body(vec![b' '; 3 * 1024 * 1024 / 2])
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 413);
    assert_eq!(
        r.json::<Value>().await.unwrap()["error"]["code"],
        "request_too_large"
    );
}

#[tokio::test]
async fn busy_http_port_is_a_clear_error_and_leaves_the_owner_serving() {
    let app = app().await;
    let listener = http::bind(0).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(http::serve(
        app.core.clone(),
        listener,
        std::future::pending(),
    ));
    let err = http::bind(port).await.unwrap_err();
    assert_eq!(err, format!("port {port} is already in use"));
    // Leave occupied ports untouched without choosing another.
    let r = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{port}/api/v1/search"))
        .bearer_auth(&app.reader_token)
        .json(&json!({"env":"A","model":"res.partner","limit":1}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
}

/// Write-request stdio profile with environment A in approval mode.
async fn local_profile_write_request(app: &App) {
    let profile = app
        .core
        .clients()
        .await
        .unwrap()
        .into_iter()
        .find(|c| c.local_stdio)
        .unwrap();
    app.core
        .update_client(
            &profile.id,
            &profile.label,
            odx_core::ClientScope::All,
            vec![Capability::OdooRead, Capability::OdooWriteRequest],
        )
        .await
        .unwrap();
    let env_a = app.core.environments().await.unwrap()[0].id.clone();
    app.core
        .set_mode(&env_a, odx_core::WriteMode::Approval)
        .await
        .unwrap();
}
fn write_line(id: u32) -> String {
    format!(
        r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{{META},"name":"write","arguments":{{"env":"A","model":"res.partner","ids":[1],"values":{{"name":"x"}}}}}}}}"#
    )
}
fn search_line(id: u32, limit: u32) -> String {
    format!(
        r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{{META},"name":"search","arguments":{{"env":"A","model":"res.partner","limit":{limit},"format":"json"}}}}}}"#
    )
}
async fn silent(out: &mut tokio::io::Lines<BufReader<tokio::io::DuplexStream>>) -> bool {
    tokio::time::timeout(std::time::Duration::from_millis(150), out.next_line())
        .await
        .is_err()
}

#[tokio::test]
async fn two_connectors_and_http_share_the_core_with_isolated_replies() {
    let app = app().await;
    local_profile_write_request(&app).await;
    let listener = http::bind(0).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(http::serve(
        app.core.clone(),
        listener,
        std::future::pending(),
    ));
    let (mut in1, mut out1, task1) = connect(&app).await;
    let (mut in2, mut out2, task2) = connect(&app).await;
    for _ in 0..200 {
        if app.core.mcp_connections().await.len() == 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(app.core.mcp_connections().await.len(), 2);
    let local = app
        .core
        .clients()
        .await
        .unwrap()
        .into_iter()
        .filter(|client| client.local_stdio)
        .collect::<Vec<_>>();
    assert_eq!(local.len(), 1, "multiple connectors share one profile");
    assert!(
        app.core
            .mcp_connections()
            .await
            .iter()
            .all(|connection| connection.client_id == local[0].id)
    );

    // Connector 1 leaves write ID 1 awaiting approval.
    send(&mut in1, &write_line(1)).await;
    wait_pending(&app.core).await;
    // Connector 2 reuses the ID; cancelling it cannot affect connector 1.
    send(
        &mut in2,
        r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":1}}"#,
    )
    .await;
    // Concurrent reads reuse the same ID across both connectors and HTTP.
    let client = reqwest::Client::new();
    let api = client
        .post(format!("http://127.0.0.1:{port}/api/v1/search"))
        .bearer_auth(&app.reader_token)
        .json(&json!({"env":"A","model":"res.partner","limit":2}))
        .send();
    send(&mut in1, &search_line(7, 1)).await;
    send(&mut in2, &search_line(7, 3)).await;
    let (r1, r2, api) = tokio::join!(next(&mut out1), next(&mut out2), api);
    let api: Value = api.unwrap().json().await.unwrap();
    assert_eq!((r1["id"].clone(), r2["id"].clone()), (json!(7), json!(7)));
    assert_eq!(r1["result"]["structuredContent"]["data"], json!([1]));
    assert_eq!(r2["result"]["structuredContent"]["data"], json!([1, 2, 3]));
    assert_eq!(api["data"], json!([1, 2]));

    let pending = app.core.approvals().await.unwrap();
    assert_eq!(
        pending.len(),
        1,
        "another connector's cancellation must preserve this request"
    );
    app.core.decide(&pending[0].id, true).await.unwrap();
    let r = next(&mut out1).await;
    assert_eq!(r["id"], 1);
    assert_eq!(r["result"]["isError"], false);
    assert_eq!(app.sim.counts.writes(), 1);
    assert!(silent(&mut out1).await && silent(&mut out2).await);

    // Closing one connector leaves the other and HTTP intact.
    drop(in2);
    assert!(task2.await.unwrap().is_ok());
    send(&mut in1, &search_line(8, 1)).await;
    assert_eq!(next(&mut out1).await["id"], 8);
    drop(in1);
    assert!(task1.await.unwrap().is_ok());
}

#[tokio::test]
async fn cancel_or_eof_after_send_completes_the_write_once_and_writes_nothing() {
    let app = app().await;
    local_profile_write_request(&app).await;

    // Cancel after the approved write reaches Odoo.
    let (mut stdin, mut out, task) = connect(&app).await;
    send(&mut stdin, &write_line(1)).await;
    wait_pending(&app.core).await;
    let id = app.core.approvals().await.unwrap()[0].id.clone();
    app.sim.counts.pause_next_write();
    let core = app.core.clone();
    let decision = tokio::spawn(async move { core.decide(&id, true).await });
    app.sim.counts.wait_for_write().await;
    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":1}}"#,
    )
    .await;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    app.sim.counts.release_write();
    assert!(decision.await.unwrap().is_ok(), "write completes");
    assert_eq!(app.sim.counts.writes(), 1);
    // No cancelled response; the session continues.
    send(
        &mut stdin,
        &format!(r#"{{"jsonrpc":"2.0","id":2,"method":"server/discover","params":{{{META}}}}}"#),
    )
    .await;
    assert_eq!(next(&mut out).await["id"], 2);
    drop(stdin);
    assert!(task.await.unwrap().is_ok());

    // Client EOF ends the connector without awaiting the in-flight write.
    let (mut stdin, mut out, task) = connect(&app).await;
    send(&mut stdin, &write_line(3)).await;
    wait_pending(&app.core).await;
    let id = app.core.approvals().await.unwrap()[0].id.clone();
    app.sim.counts.pause_next_write();
    let core = app.core.clone();
    let decision = tokio::spawn(async move { core.decide(&id, true).await });
    app.sim.counts.wait_for_write().await;
    drop(stdin);
    assert!(task.await.unwrap().is_ok());
    app.sim.counts.release_write();
    assert!(decision.await.unwrap().is_ok());
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(app.sim.counts.writes(), 2, "once each, without retries");
    assert!(app.core.approvals().await.unwrap().is_empty());
    // Connector closes stdout without messages.
    assert!(out.next_line().await.unwrap().is_none());
}

#[tokio::test]
async fn oversized_echo_is_bounded_and_the_connection_keeps_answering() {
    let app = app().await;
    let (mut stdin, mut out, _task) = connect(&app).await;
    let name = "x".repeat(900_000);
    send(&mut stdin, &format!(r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{{META},"name":"{name}","arguments":{{}}}}}}"#)).await;
    let r = next(&mut out).await;
    assert_eq!(r["error"]["code"], -32602);
    assert!(r["error"]["message"].as_str().unwrap().len() < 200);
    send(&mut stdin, r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#).await;
    assert_eq!(next(&mut out).await["id"], 2);
}
