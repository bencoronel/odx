mod support;
use odx_core::{ActivityFilter, Core, EnvironmentDraft, Protocol, ShellCall, WriteMode};
use serde_json::{Map, json};

fn call(
    env: &str,
    model: &str,
    method: &str,
    args: serde_json::Value,
    confirmed: bool,
) -> ShellCall {
    ShellCall {
        environment_id: env.into(),
        model: model.into(),
        method: method.into(),
        args: args.as_array().cloned().unwrap_or_default(),
        kwargs: Map::new(),
        context: Map::new(),
        confirmed,
        client_id: None,
        client_revision: None,
    }
}

#[tokio::test]
async fn shell_requires_confirmation_and_respects_read_only() {
    let tmp = tempfile::tempdir().unwrap();
    let sim = support::simulator().await;
    let core = support::unlocked_core(&tmp).await;
    let env = core
        .save_environment(EnvironmentDraft {
            name: "Prod".into(),
            url: sim.url.clone(),
            database: "base-a".into(),
            login: "fixture-user".into(),
            protocol: Protocol::LegacyJsonrpc,
            context: json!({"lang": "es_ES"}),
            secret: "fixture-secret".into(),
        })
        .await
        .unwrap();
    // Shell reads in the default read-only mode; confirmed writes still need a writable mode.
    let count = core
        .shell_call(call(
            &env.id,
            "res.partner",
            "search_count",
            json!([[]]),
            false,
        ))
        .await
        .unwrap();
    assert_eq!(count.data, 5);
    assert!(!count.write);

    let mut session = call(&env.id, "res.partner", "search_read", json!([]), false);
    session.context.insert("tz".into(), json!("UTC"));
    let rows = core.shell_call(session).await.unwrap();
    assert_eq!(rows.data[0]["context"], json!({"lang":"es_ES","tz":"UTC"}));

    let unconfirmed = core
        .shell_call(call(
            &env.id,
            "res.partner",
            "write",
            json!([[1], {"name":"x"}]),
            false,
        ))
        .await
        .unwrap_err();
    assert_eq!(unconfirmed.code, "confirmation_required");
    let read_only = core
        .shell_call(call(
            &env.id,
            "res.partner",
            "write",
            json!([[1], {"name":"x"}]),
            true,
        ))
        .await
        .unwrap_err();
    assert_eq!(read_only.code, "forbidden");
    assert_eq!(sim.counts.writes(), 0);
    // Confirmation stands in for individual approval.
    core.set_mode(&env.id, WriteMode::Approval).await.unwrap();
    let done = core
        .shell_call(call(
            &env.id,
            "res.partner",
            "write",
            json!([[1], {"name":"x"}]),
            true,
        ))
        .await
        .unwrap();
    assert!(done.write && done.data == json!(true));
    assert_eq!(
        core.shell_call(call(
            &env.id,
            "sale.order",
            "action_confirm",
            json!([[1]]),
            false
        ))
        .await
        .unwrap_err()
        .code,
        "confirmation_required"
    );
    assert_eq!(sim.counts.writes(), 1);
    // Every unknown-read call must return the UI's confirmation_required code before contacting Odoo, regardless of environment or method.
    let (logins, queries) = (sim.counts.logins(), sim.counts.queries());
    for (env_id, method, args) in [
        (env.id.as_str(), "create", json!([{"name":"x"}])),
        (env.id.as_str(), "unlink", json!([[1]])),
        (env.id.as_str(), "web_save", json!([[1], {"name":"x"}])),
        (env.id.as_str(), "copy", json!([[1]])),
        (env.id.as_str(), "action_custom", json!([])),
        (env.id.as_str(), "read_group_custom", json!([])),
        ("missing", "write", json!([[1], {"name":"x"}])),
    ] {
        assert_eq!(
            core.shell_call(call(env_id, "res.partner", method, args, false))
                .await
                .unwrap_err()
                .code,
            "confirmation_required",
            "{method}"
        );
    }
    assert_eq!(
        (
            sim.counts.writes(),
            sim.counts.logins(),
            sim.counts.queries()
        ),
        (1, logins, queries)
    );

    assert_eq!(
        core.shell_call(call(&env.id, "res.partner", "_private", json!([]), true))
            .await
            .unwrap_err()
            .code,
        "invalid_arguments"
    );
    assert_eq!(
        core.shell_call(call(&env.id, "res partner", "read", json!([]), true))
            .await
            .unwrap_err()
            .code,
        "invalid_arguments"
    );
    let mut both = call(&env.id, "res.partner", "search_count", json!([[]]), false);
    both.context.insert("a".into(), json!(1));
    both.kwargs.insert("context".into(), json!({}));
    assert_eq!(
        core.shell_call(both).await.unwrap_err().code,
        "invalid_arguments"
    );
    let remote = core
        .shell_call(call(
            &env.id,
            "broken.model",
            "search_count",
            json!([[]]),
            false,
        ))
        .await
        .unwrap_err();
    assert_eq!(remote.code, "upstream_error");

    let activity = core
        .activity(None, ActivityFilter::default())
        .await
        .unwrap();
    assert!(
        activity
            .events
            .iter()
            .any(|e| e.operation == "write:res.partner"
                && e.client_id.is_none()
                && e.outcome == "succeeded")
    );

    core.lock().await;
    assert_eq!(
        core.shell_call(call(
            &env.id,
            "res.partner",
            "search_count",
            json!([[]]),
            false
        ))
        .await
        .unwrap_err()
        .code,
        "locked"
    );
}

#[tokio::test]
async fn shell_history_is_encrypted_capped_and_survives_unlock() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("vault.sqlite3");
    let core = Core::new(&path).unwrap();
    core.create("fictional master password").await.unwrap();
    core.shell_history_add(
        "search res.partner -d [('name','ilike','business-data-test')]",
        Some("Prod".into()),
    )
    .await
    .unwrap();
    core.shell_history_add(
        "search res.partner -d [('name','ilike','business-data-test')]",
        Some("Prod".into()),
    )
    .await
    .unwrap();
    core.shell_history_add("   ", None).await.unwrap();
    core.shell_history_add(&"x".repeat(5000), None)
        .await
        .unwrap();
    assert_eq!(
        core.shell_history().await.unwrap().len(),
        1,
        "exclude consecutive duplicates, empty and oversized entries"
    );
    let raw = std::fs::read(&path).unwrap();
    assert!(
        !raw.windows(18).any(|w| w == b"business-data-test"),
        "encrypted on disk"
    );

    for i in 0..510 {
        core.shell_history_add(&format!("count model{i}"), None)
            .await
            .unwrap();
    }
    let history = core.shell_history().await.unwrap();
    assert_eq!(history.len(), 500);
    assert_eq!(history.last().unwrap().command, "count model509");

    core.lock().await;
    assert!(core.shell_history().await.is_err());
    core.unlock("fictional master password").await.unwrap();
    assert_eq!(core.shell_history().await.unwrap().len(), 500);
    core.shell_history_clear().await.unwrap();
    assert!(core.shell_history().await.unwrap().is_empty());
}

#[tokio::test]
async fn shell_never_generates_or_shows_api_keys() {
    let tmp = tempfile::tempdir().unwrap();
    let sim = support::simulator().await;
    let core = support::unlocked_core(&tmp).await;
    let env = core
        .save_environment(support::draft("Prod", &sim.url))
        .await
        .unwrap();
    for (model, method) in [
        ("res.users.apikeys", "generate"),
        ("res.users.apikeys.description", "make_key"),
    ] {
        let err = core
            .shell_call(call(&env.id, model, method, json!([]), true))
            .await
            .unwrap_err();
        assert_eq!(err.code, "forbidden");
    }
    assert_eq!(sim.counts.logins(), 0);
    let activity = core
        .activity(None, ActivityFilter::default())
        .await
        .unwrap();
    assert_eq!(
        activity
            .events
            .iter()
            .filter(|e| e.outcome == "denied")
            .count(),
        2
    );
}

#[tokio::test]
async fn a_shell_write_rejected_before_sending_is_failed_not_uncertain() {
    let tmp = tempfile::tempdir().unwrap();
    let sim = support::protocol_simulator(19).await;
    let core = support::unlocked_core(&tmp).await;
    let env = core
        .save_environment(EnvironmentDraft {
            name: "Prod".into(),
            url: sim.url.clone(),
            database: "base-a".into(),
            login: "fixture-user".into(),
            protocol: Protocol::Xmlrpc,
            context: json!({}),
            secret: "fixture-secret".into(),
        })
        .await
        .unwrap();
    core.set_mode(&env.id, WriteMode::Permanent).await.unwrap();
    // XML-RPC cannot carry control characters: the call fails while encoding, before it is sent.
    let error = core
        .shell_call(call(
            &env.id,
            "res.partner",
            "write",
            json!([[1], {"name": "\u{1}x"}]),
            true,
        ))
        .await
        .unwrap_err();
    assert_ne!(error.code, "uncertain_result");
    assert_eq!(sim.counts.writes(), 0);
    let events = core
        .activity(None, ActivityFilter::default())
        .await
        .unwrap()
        .events;
    assert!(
        events
            .iter()
            .any(|e| e.operation == "write:res.partner" && e.outcome == "failed")
    );
}

#[tokio::test]
async fn a_confirmed_write_whose_response_is_lost_is_uncertain() {
    let tmp = tempfile::tempdir().unwrap();
    let sim = support::simulator().await;
    let core = support::unlocked_core(&tmp).await;
    let env = core
        .save_environment(support::draft("Prod", &sim.url))
        .await
        .unwrap();
    core.set_mode(&env.id, WriteMode::Permanent).await.unwrap();
    // Log in first so the lost response is the call itself.
    core.shell_call(call(
        &env.id,
        "res.partner",
        "search_count",
        json!([[]]),
        false,
    ))
    .await
    .unwrap();
    sim.counts.lose_next_response();
    let write = core
        .shell_call(call(
            &env.id,
            "res.partner",
            "write",
            json!([[1], {"name": "x"}]),
            true,
        ))
        .await
        .unwrap_err();
    assert_eq!(write.code, "uncertain_result");
    let events = core
        .activity(None, ActivityFilter::default())
        .await
        .unwrap()
        .events;
    let outcome = |op: &str| {
        events
            .iter()
            .find(|e| e.operation == op)
            .unwrap()
            .outcome
            .clone()
    };
    assert_eq!(outcome("write:res.partner"), "uncertain");
    assert_eq!(outcome("search_count:res.partner"), "succeeded");
}

#[tokio::test]
async fn web_urls_open_saved_environments_and_valid_records_only() {
    let tmp = tempfile::tempdir().unwrap();
    let core = support::unlocked_core(&tmp).await;
    let env = core
        .save_environment(support::draft("Prod", "https://odoo.test/"))
        .await
        .unwrap();
    assert_eq!(
        core.web_url(&env.id, None).await.unwrap(),
        "https://odoo.test"
    );
    assert_eq!(
        core.web_url(&env.id, Some(("res.partner", 7)))
            .await
            .unwrap(),
        "https://odoo.test/web#id=7&model=res.partner&view_type=form"
    );
    for (model, id) in [
        ("", 7),
        ("Res.Partner", 7),
        ("res.partner&x=1", 7),
        (".res", 7),
        ("res/../x", 7),
        ("res.partner", 0),
    ] {
        assert_eq!(
            core.web_url(&env.id, Some((model, id)))
                .await
                .unwrap_err()
                .code(),
            "app.invalid_input",
            "{model} {id}"
        );
    }
    assert_eq!(
        core.web_url("missing", None).await.unwrap_err().code(),
        "app.environment_not_found"
    );
}
