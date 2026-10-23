mod support;
use odx_core::{
    ActivityFilter, Capability, ClientScope, Core, DesktopRequest, EnvironmentDraft, Protocol,
    ShellCall, WriteMode,
};
use serde_json::{Map, json};

async fn env(core: &Core, name: &str, url: &str) -> String {
    core.save_environment(EnvironmentDraft {
        name: name.into(),
        url: url.into(),
        database: "base-a".into(),
        login: "fixture-user".into(),
        protocol: Protocol::LegacyJsonrpc,
        context: json!({}),
        secret: "fixture-secret".into(),
    })
    .await
    .unwrap()
    .id
}

#[tokio::test]
async fn agents_see_terminals_of_their_environments_and_run_commands_through_the_app() {
    let tmp = tempfile::tempdir().unwrap();
    let sim = support::simulator().await;
    let core = Core::new(tmp.path().join("vault.sqlite3")).unwrap();
    core.create("fictional master password").await.unwrap();
    let prod = env(&core, "Prod", &sim.url).await;
    let other = env(&core, "Other", &sim.url).await;
    let issued = core
        .create_client(
            "Claude",
            ClientScope::Only(vec![prod.clone()]),
            vec![Capability::OdooRead],
        )
        .await
        .unwrap();
    let agent = core.authenticate_client(&issued.token).await.unwrap();
    core.shell_sessions_save(
        &json!({ "version": 1, "activeId": "t1", "tabs": [
            { "id": "t1", "name": "", "defaultName": true, "envId": prod, "envName": "Prod", "session": { "entries": [
                { "id": 1, "kind": "input", "prompt": "ben>", "text": "count res.partner", "command": true },
                { "id": 2, "kind": "text", "text": "5" },
            ] } },
            { "id": "t2", "name": "", "defaultName": true, "envId": other, "envName": "Other", "session": { "entries": [] } },
        ] })
        .to_string(),
    )
    .await
    .unwrap();

    // Only tabs of granted environments exist for the agent.
    let listed = core
        .run_tool(&agent, "terminals", json!({"format": "json"}))
        .await
        .unwrap();
    assert_eq!(
        listed.data,
        json!([{ "terminal": "t1", "name": "Prod", "env": "Prod", "active": true, "commands": 1, "last_command": "count res.partner" }])
    );
    let read = core
        .run_tool(&agent, "terminal_read", json!({"terminal": "t1"}))
        .await
        .unwrap();
    assert_eq!(
        read.data,
        json!([{ "command": "count res.partner", "output": [{ "kind": "text", "text": "5" }] }])
    );
    let hidden = core
        .run_tool(&agent, "terminal_read", json!({"terminal": "t2"}))
        .await
        .unwrap_err();
    assert_eq!(hidden.code, "not_found");

    // Without the app there is nobody to run or show anything.
    let alone = core
        .run_tool(
            &agent,
            "terminal_run",
            json!({"terminal": "t1", "command": "count res.partner"}),
        )
        .await
        .unwrap_err();
    assert_eq!(alone.code, "unavailable");

    // The app gets the request with the client's name and rights, and its answer is the tool's result.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    core.set_desktop(tx);
    let app = tokio::spawn(async move {
        match rx.recv().await.unwrap() {
            DesktopRequest::Run {
                terminal,
                env_id,
                command,
                client_label,
                allow_writes,
                reply,
                ..
            } => {
                assert_eq!(
                    (
                        terminal.as_deref(),
                        command.as_str(),
                        client_label.as_str(),
                        allow_writes
                    ),
                    (Some("t1"), "count res.partner", "Claude", false)
                );
                reply.send(Ok(json!({ "terminal": "t1", "output": [{ "kind": "text", "text": "5" }], "env_id": env_id }))).unwrap();
            }
            other => panic!("unexpected {other:?}"),
        }
        match rx.recv().await.unwrap() {
            DesktopRequest::Show { terminal, reply } => {
                assert_eq!(terminal, "t1");
                reply.send(Ok(json!(null))).unwrap();
            }
            other => panic!("unexpected {other:?}"),
        }
    });
    let ran = core
        .run_tool(
            &agent,
            "terminal_run",
            json!({"terminal": "t1", "command": "count res.partner", "format": "json"}),
        )
        .await
        .unwrap();
    assert_eq!(ran.data["output"], json!([{ "kind": "text", "text": "5" }]));
    assert_eq!(ran.data["env_id"], json!(prod));
    core.run_tool(&agent, "show", json!({"terminal": "t1"}))
        .await
        .unwrap();
    app.await.unwrap();
    // An environment outside the grant is refused before reaching the app.
    let outside = core
        .run_tool(
            &agent,
            "terminal_run",
            json!({"env": "Other", "command": "count res.partner"}),
        )
        .await
        .unwrap_err();
    assert_eq!(outside.code, "environment_unavailable");

    // What the app runs for the agent is checked again and audited under the agent.
    let call = |environment_id: &str, method: &str| ShellCall {
        environment_id: environment_id.into(),
        model: "res.partner".into(),
        method: method.into(),
        args: vec![json!([])],
        kwargs: Map::new(),
        context: Map::new(),
        confirmed: true,
        client_id: Some(agent.client_id.clone()),
    };
    core.shell_call(call(&prod, "search_count")).await.unwrap();
    assert_eq!(
        core.shell_call(call(&other, "search_count"))
            .await
            .unwrap_err()
            .code,
        "forbidden"
    );
    core.set_mode(&prod, WriteMode::Permanent).await.unwrap();
    assert_eq!(
        core.shell_call(call(&prod, "write"))
            .await
            .unwrap_err()
            .code,
        "forbidden",
        "no odoo:write-request"
    );
    let events = core
        .activity(
            None,
            ActivityFilter {
                client_id: Some(agent.client_id.clone()),
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .events;
    assert!(
        events
            .iter()
            .any(|e| e.operation == "search_count:res.partner")
    );
}
