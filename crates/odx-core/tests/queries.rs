mod flow;
mod support;
use odx_core::{Core, EnvironmentDraft, Protocol, WriteMode};
use serde_json::json;

#[tokio::test]
async fn two_simulated_environments_are_isolated_and_locked_core_sends_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let sim = support::simulator().await;
    let core = Core::new(tmp.path().join("vault.sqlite3")).unwrap();
    assert_eq!(core.status().await, "absent");
    core.create("fictional master password").await.unwrap();
    for (name, db, lang) in [("North", "base-a", "es_ES"), ("South", "base-b", "en_US")] {
        core.save_environment(EnvironmentDraft {
            name: name.into(),
            url: sim.url.clone(),
            database: db.into(),
            login: "fixture-user".into(),
            protocol: Protocol::LegacyJsonrpc,
            context: json!({"lang":lang}),
            secret: "fixture-secret".into(),
        })
        .await
        .unwrap();
    }
    let envs = core.environments().await.unwrap();
    assert_eq!(envs.len(), 2);
    assert!(envs.iter().all(|e| e.has_secret));
    assert!(envs.iter().all(|e| e.write_mode == WriteMode::ReadOnly));
    assert_ne!(envs[0].id, envs[1].id);
    core.lock().await;
    assert!(core.unlock("incorrect fictional password").await.is_err());
    assert_eq!(core.status().await, "locked");
    assert_eq!(sim.counts.logins(), 0);
    core.unlock("fictional master password").await.unwrap();
    let client = flow::client(&core).await;
    let read = |env: &'static str, extra: serde_json::Value| {
        let mut args = json!({"env":env,"model":"res.partner"});
        args.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        core.run_tool(&client, "search_read", args)
    };
    let a = read("North", json!({"context":{"tz":"Europe/Madrid"}}))
        .await
        .unwrap();
    let b = read("South", json!({})).await.unwrap();
    assert_eq!(a.data[0]["display_name"], "contact-base-a");
    assert_eq!(b.data[0]["display_name"], "contact-base-b");
    assert_eq!(a.data[0]["context"]["lang"], "es_ES");
    assert_eq!(a.data[0]["context"]["tz"], "Europe/Madrid");
    assert_eq!(b.data[0]["context"]["lang"], "en_US");
    let default_page = read("North", json!({})).await.unwrap();
    assert_eq!(
        default_page.meta["page"]["limit"], 50,
        "use the default limit when omitted"
    );
    read("North", json!({"limit":50})).await.unwrap();
    assert_eq!(
        sim.counts.logins(),
        2,
        "reuse authentication by environment and revision"
    );
    core.update_environment(
        &envs[0].id,
        EnvironmentDraft {
            name: "North".into(),
            url: sim.url.clone(),
            database: "base-a".into(),
            login: "fixture-user".into(),
            protocol: Protocol::LegacyJsonrpc,
            context: json!({"lang":"es_ES","company_id":1}),
            secret: "fixture-secret".into(),
        },
    )
    .await
    .unwrap();
    read("North", json!({})).await.unwrap();
    assert_eq!(
        sim.counts.logins(),
        3,
        "revision changes invalidate only that environment's authentication"
    );
    assert_eq!(sim.counts.queries(), 5);
    assert!(sim.counts.request_bytes() > 0);
    core.lock().await;
    assert_eq!(read("North", json!({})).await.unwrap_err().code, "locked");
    assert_eq!(sim.counts.writes(), 0);
    core.unlock("fictional master password").await.unwrap();
    assert_eq!(core.status().await, "unlocked");
    core.set_mode(&envs[0].id, WriteMode::ReadOnly)
        .await
        .unwrap();
}

#[tokio::test]
async fn deleting_environment_revokes_access_without_upstream_calls() {
    let tmp = tempfile::tempdir().unwrap();
    let sim = support::simulator().await;
    let core = support::unlocked_core(&tmp).await;
    let env = core
        .save_environment(support::draft("Disposable fixture", &sim.url))
        .await
        .unwrap();
    let clone = core
        .clone_environment(&env.id, "Disposable clone")
        .await
        .unwrap();
    assert_ne!(clone.id, env.id);
    assert_eq!(clone.write_mode, WriteMode::ReadOnly);
    assert!(
        core.save_environment(EnvironmentDraft {
            name: "Disposable fixture".into(),
            url: "http://127.0.0.1:1".into(),
            database: "duplicate".into(),
            login: "fixture-user".into(),
            protocol: Protocol::LegacyJsonrpc,
            context: json!({}),
            secret: "fixture-secret".into(),
        })
        .await
        .is_err()
    );
    let client = flow::client(&core).await;
    core.delete_environment(&env.id, true).await.unwrap();
    assert_eq!(core.environments().await.unwrap().len(), 1);
    let gone = core
        .run_tool(
            &client,
            "search_read",
            json!({"env":"Disposable fixture","model":"res.partner"}),
        )
        .await
        .unwrap_err();
    assert_eq!(gone.code, "environment_unavailable");
    assert_eq!(sim.counts.logins(), 0);
}
