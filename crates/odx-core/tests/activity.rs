mod flow;
mod support;
use odx_core::{ActivityFilter, Capability, Core, EnvironmentDraft, Protocol, WriteMode};
use serde_json::json;

#[tokio::test]
async fn activity_spans_sessions_names_clients_and_filters_without_secrets() {
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
    let issued = core
        .create_client(
            "agente",
            vec![env.id.clone()],
            vec![Capability::OdooRead, Capability::OdooWriteRequest],
        )
        .await
        .unwrap();
    let client = core.authenticate_client(&issued.token).await.unwrap();
    core.run_tool(
        &client,
        "search_count",
        json!({"env":"A","model":"res.partner"}),
    )
    .await
    .unwrap();

    // Previous history remains visible after reopening.
    core.lock().await;
    assert!(
        core.activity(None, ActivityFilter::default())
            .await
            .is_err(),
        "locked vault prevents queries"
    );
    core.unlock("fictional master password").await.unwrap();
    core.set_mode(&env.id, WriteMode::Permanent).await.unwrap();
    core.run_tool(
        &client,
        "write",
        json!({"env":"A","model":"res.partner","ids":[1],"values":{"name":"x"}}),
    )
    .await
    .unwrap();
    core.run_tool(
        &client,
        "search_count",
        json!({"env":"A","model":"broken.model"}),
    )
    .await
    .unwrap_err();

    let page = core
        .activity(None, ActivityFilter::default())
        .await
        .unwrap();
    assert!(page.complete);
    assert_eq!(
        page.events.len(),
        3,
        "one row for each of the three requests"
    );
    assert!(page.events.iter().all(|e| e.outcome != "attempted"));
    assert!(
        page.events.windows(2).all(|w| w[0].at >= w[1].at),
        "newest first"
    );
    assert!(
        page.events
            .iter()
            .any(|e| e.operation == "search_count:res.partner"
                && e.client_label.as_deref() == Some("agente"))
    );
    assert!(
        page.events
            .iter()
            .all(|e| e.environment_name.as_deref() == Some("A"))
    );
    let text = serde_json::to_string(&page).unwrap();
    assert!(!text.contains("fixture-secret") && !text.contains(&issued.token));

    let writes = core
        .activity(
            None,
            ActivityFilter {
                writes_only: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(!writes.events.is_empty() && writes.events.iter().all(|e| e.write));
    let errors = core
        .activity(
            None,
            ActivityFilter {
                errors_only: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(
        errors
            .events
            .iter()
            .any(|e| e.operation == "search_count:broken.model")
    );
    let none = core
        .activity(
            None,
            ActivityFilter {
                q: Some("no-match".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(none.events.is_empty() && none.next_cursor.is_none());

    let by_client = core
        .activity(
            None,
            ActivityFilter {
                client_id: Some(client.client_id.clone()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(
        !by_client.events.is_empty()
            && by_client
                .events
                .iter()
                .all(|e| e.client_id.as_deref() == Some(client.client_id.as_str()))
    );
    let nobody = core
        .activity(
            None,
            ActivityFilter {
                client_id: Some("other".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(nobody.events.is_empty());

    // Deleting an environment preserves its history without its name.
    core.delete_environment(&env.id, true).await.unwrap();
    let page = core
        .activity(None, ActivityFilter::default())
        .await
        .unwrap();
    assert!(page.events.iter().all(|e| e.environment_name.is_none()));
}

#[tokio::test]
async fn concurrent_calls_and_approval_transitions_keep_one_row_per_request() {
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
    let client = flow::client(&core).await;
    let read = || {
        core.run_tool(
            &client,
            "search_read",
            json!({"env":"A","model":"res.partner","limit":1}),
        )
    };
    let (a, b) = tokio::join!(read(), read());
    a.unwrap();
    b.unwrap();
    let page = core
        .activity(None, ActivityFilter::default())
        .await
        .unwrap();
    assert_eq!(
        page.events.len(),
        2,
        "identical concurrent calls remain separate"
    );
    assert!(page.events.iter().all(|e| e.outcome == "succeeded"));
    core.set_mode(&env.id, WriteMode::Approval).await.unwrap();
    for decision in ["succeeded", "rejected", "cancelled"] {
        let (call, pending) = flow::queue(
            &core,
            &client,
            "write",
            json!({"env":"A","model":"res.partner","ids":[1],"values":{"name":"fixture"}}),
        )
        .await;
        let before = core
            .activity(None, ActivityFilter::default())
            .await
            .unwrap();
        assert_eq!(before.events[0].outcome, "pending");
        assert_eq!(before.events[0].operation, "write:res.partner");
        match decision {
            "succeeded" => {
                core.decide(&pending.id, true).await.unwrap();
                call.await.unwrap().unwrap();
            }
            "rejected" => {
                core.decide(&pending.id, false).await.unwrap();
                assert_eq!(flow::error_code(call).await, "approval_rejected");
            }
            _ => flow::abandon(&core, call, &pending.id).await,
        }
        let after = core
            .activity(None, ActivityFilter::default())
            .await
            .unwrap();
        assert_eq!(before.events.len(), after.events.len());
        assert_eq!(after.events[0].outcome, decision);
    }
    let writes = core
        .activity(
            None,
            ActivityFilter {
                writes_only: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(writes.events.len(), 3);
    let errors = core
        .activity(
            None,
            ActivityFilter {
                errors_only: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(errors.events.len(), 2);
    core.lock().await;
    core.unlock("fictional master password").await.unwrap();
    assert_eq!(
        core.activity(None, ActivityFilter::default())
            .await
            .unwrap()
            .events
            .len(),
        5
    );
}

#[tokio::test]
async fn environment_filter_and_text_search_return_only_matching_rows() {
    let tmp = tempfile::tempdir().unwrap();
    let sim = support::simulator().await;
    let core = Core::new(tmp.path().join("vault.sqlite3")).unwrap();
    core.create("fictional master password").await.unwrap();
    let mut ids = Vec::new();
    for (name, database) in [("Alfa", "base-a"), ("Beta", "base-b")] {
        let env = core
            .save_environment(EnvironmentDraft {
                name: name.into(),
                url: sim.url.clone(),
                database: database.into(),
                login: "fixture-user".into(),
                protocol: Protocol::LegacyJsonrpc,
                context: json!({}),
                secret: "fixture-secret".into(),
            })
            .await
            .unwrap();
        ids.push(env.id);
    }
    let issued = core
        .create_client("agente", ids.clone(), vec![Capability::OdooRead])
        .await
        .unwrap();
    let client = core.authenticate_client(&issued.token).await.unwrap();
    for (env, model) in [
        ("Alfa", "res.partner"),
        ("Beta", "res.users"),
        ("Alfa", "res.country"),
    ] {
        core.run_tool(&client, "search_count", json!({"env":env,"model":model}))
            .await
            .unwrap();
    }
    let query = |environment_id: Option<&str>, q: Option<&str>| {
        core.activity(
            None,
            ActivityFilter {
                environment_id: environment_id.map(Into::into),
                q: q.map(Into::into),
                ..Default::default()
            },
        )
    };
    let operations = |page: odx_core::ActivityPage| {
        assert!(page.complete && page.next_cursor.is_none());
        let mut ops: Vec<_> = page.events.into_iter().map(|e| e.operation).collect();
        ops.sort();
        ops
    };

    let beta = query(Some(&ids[1]), None).await.unwrap();
    assert!(
        beta.events
            .iter()
            .all(|e| e.environment_id == ids[1] && e.environment_name.as_deref() == Some("Beta"))
    );
    assert_eq!(operations(beta), ["search_count:res.users"]);
    assert_eq!(
        operations(query(Some(&ids[0]), None).await.unwrap()),
        ["search_count:res.country", "search_count:res.partner"]
    );
    assert!(
        query(Some("no-environment"), None)
            .await
            .unwrap()
            .events
            .is_empty()
    );

    // Match operation, environment name and client label case-insensitively.
    assert_eq!(
        operations(query(None, Some("res.users")).await.unwrap()),
        ["search_count:res.users"]
    );
    assert_eq!(
        operations(query(None, Some("BETA")).await.unwrap()),
        ["search_count:res.users"]
    );
    assert_eq!(
        operations(query(None, Some("agente")).await.unwrap()).len(),
        3
    );
    // Combine filters; text cannot broaden the selected environment.
    assert_eq!(
        operations(query(Some(&ids[0]), Some("users")).await.unwrap()),
        Vec::<String>::new()
    );
    assert_eq!(
        operations(query(Some(&ids[0]), Some("country")).await.unwrap()),
        ["search_count:res.country"]
    );
}
