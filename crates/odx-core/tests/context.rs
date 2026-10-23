mod flow;
mod support;
use odx_core::{Core, EnvironmentDraft, Protocol, ShellCall, WriteMode, context::ContextEntry};
use serde_json::{Value, json};

fn entry(name: &str, value: Value, enabled: bool) -> ContextEntry {
    ContextEntry {
        name: name.into(),
        value,
        enabled,
    }
}
fn draft(url: &str) -> EnvironmentDraft {
    EnvironmentDraft {
        name: "A".into(),
        url: url.into(),
        database: "base-a".into(),
        login: "fixture-user".into(),
        protocol: Protocol::LegacyJsonrpc,
        context: json!({}),
        secret: "fixture-secret".into(),
    }
}

#[tokio::test]
async fn typed_global_environment_and_call_contexts_are_merged_and_disabled_entries_persist() {
    let tmp = tempfile::tempdir().unwrap();
    let sim = support::simulator().await;
    let path = tmp.path().join("vault.sqlite3");
    let core = Core::new(&path).unwrap();
    assert!(core.context_settings(None).await.is_err());
    core.create("fictional master password").await.unwrap();
    let env = core.save_environment(draft(&sim.url)).await.unwrap();
    core.set_context(
        None,
        vec![
            entry("lang", json!("es_ES"), true),
            entry("active_test", json!(false), true),
            entry("allowed_company_ids", json!([1, 2]), true),
            entry("unused", json!(3), false),
        ],
    )
    .await
    .unwrap();
    core.set_context(
        Some(&env.id),
        vec![
            entry("lang", json!("fr_FR"), true),
            entry("allowed_company_ids", json!([2]), true),
            entry("local", json!({"flag":true}), true),
        ],
    )
    .await
    .unwrap();
    let client = flow::client(&core).await;
    let query = |context| {
        core.run_tool(
            &client,
            "search_read",
            json!({"env":"A","model":"res.partner","limit":1,"context":context}),
        )
    };
    let rows = query(json!({})).await.unwrap();
    assert_eq!(
        rows.data[0]["context"],
        json!({"lang":"fr_FR","active_test":false,"allowed_company_ids":[2],"local":{"flag":true}})
    );
    let rows = query(json!({"lang":"pt_PT","active_test":true}))
        .await
        .unwrap();
    assert_eq!(rows.data[0]["context"]["lang"], "pt_PT");
    assert_eq!(rows.data[0]["context"]["active_test"], true);
    core.set_context(Some(&env.id), vec![entry("lang", json!("fr_FR"), false)])
        .await
        .unwrap();
    let rows = query(json!({})).await.unwrap();
    assert_eq!(rows.data[0]["context"]["lang"], "es_ES");
    let shell = core
        .shell_call(ShellCall {
            environment_id: env.id.clone(),
            model: "res.partner".into(),
            method: "search_read".into(),
            args: vec![json!([])],
            kwargs: Default::default(),
            context: serde_json::from_value(json!({"lang":"de_DE"})).unwrap(),
            confirmed: false,
            client_id: None,
        })
        .await
        .unwrap();
    assert_eq!(shell.data[0]["context"]["lang"], "de_DE");
    // Connection edits preserve context, including disabled rows.
    let mut updated = draft(&sim.url);
    updated.secret.clear();
    updated.context = json!({"stale":true});
    core.update_environment(&env.id, updated).await.unwrap();
    let settings = core.context_settings(Some(&env.id)).await.unwrap();
    assert_eq!(settings.entries.len(), 1);
    assert!(settings.entries[0].name == "lang" && !settings.entries[0].enabled);
    let clone = core.clone_environment(&env.id, "Copy").await.unwrap();
    assert!(
        !core
            .context_settings(Some(&clone.id))
            .await
            .unwrap()
            .entries[0]
            .enabled
    );
    core.lock().await;
    assert!(core.set_context(None, vec![]).await.is_err());
    let reopened = Core::new(&path).unwrap();
    reopened.unlock("fictional master password").await.unwrap();
    let settings = reopened.context_settings(None).await.unwrap();
    assert!(
        settings
            .entries
            .iter()
            .any(|e| e.name == "unused" && !e.enabled && e.value == json!(3))
    );
    let settings = reopened.context_settings(Some(&env.id)).await.unwrap();
    assert_eq!(settings.inherited["active_test"], false);
    assert!(
        settings
            .entries
            .iter()
            .any(|e| e.name == "lang" && !e.enabled && e.value == json!("fr_FR"))
    );
    assert_eq!(reopened.environments().await.unwrap()[0].context, json!({}));
}
#[tokio::test]
async fn invalid_context_is_atomic_and_changes_revoke_pending_approvals() {
    let tmp = tempfile::tempdir().unwrap();
    let sim = support::simulator().await;
    let core = Core::new(tmp.path().join("vault.sqlite3")).unwrap();
    core.create("fictional master password").await.unwrap();
    let env = core.save_environment(draft(&sim.url)).await.unwrap();
    core.set_context(
        None,
        vec![
            entry("lang", json!("es_ES"), true),
            entry("Lang", json!("case-sensitive"), true),
        ],
    )
    .await
    .unwrap();
    for invalid in [
        vec![entry("", json!(1), true)],
        vec![entry("a", json!(1), true), entry("a", json!(2), false)],
        vec![entry("id", json!(9_007_199_254_740_992u64), true)],
        vec![entry("big", json!("a".repeat(1_048_577)), true)],
        vec![entry(" lang", json!("es_ES"), true)],
        vec![entry("lang ", json!("es_ES"), false)],
    ] {
        assert!(core.set_context(None, invalid).await.is_err());
        assert_eq!(core.context_settings(None).await.unwrap().entries.len(), 2);
    }
    core.set_mode(&env.id, WriteMode::Approval).await.unwrap();
    let client = flow::client(&core).await;
    let request = || json!({"env":"A","model":"res.partner","ids":[1],"values":{"name":"fixture"}});
    let (call, pending) = flow::queue(&core, &client, "write", request()).await;
    core.set_context(None, vec![entry("lang", json!("en_US"), true)])
        .await
        .unwrap();
    assert!(core.decide(&pending.id, true).await.is_err());
    assert_eq!(flow::error_code(call).await, "approval_revoked");
    let (call, pending) = flow::queue(&core, &client, "write", request()).await;
    core.set_context(Some(&env.id), vec![entry("lang", json!("fr_FR"), true)])
        .await
        .unwrap();
    assert!(core.decide(&pending.id, true).await.is_err());
    assert_eq!(flow::error_code(call).await, "approval_revoked");
    assert_eq!(sim.counts.writes(), 0);
}
