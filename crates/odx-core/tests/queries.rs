mod flow;
mod support;
use odx_core::{ActivityFilter, Core, EnvironmentDraft, Protocol, RuleDraft, WriteMode};
use serde_json::json;

#[tokio::test]
async fn related_domain_rechecks_new_rule_before_return() {
    let tmp = tempfile::tempdir().unwrap();
    let sim = support::simulator().await;
    let core = support::unlocked_core(&tmp).await;
    core.save_environment(support::draft("Protected", &sim.url))
        .await
        .unwrap();
    let make_rule = |field| {
        serde_json::from_value::<RuleDraft>(json!({"environment_id":"", "model":"res.users", "method":"all", "kind":"fields", "action":"block", "scope":{"kind":"global"}, "operations":["read"], "model_selector":{"kind":"exact","value":"res.users"}, "fields":[field], "context":{}, "max_records":0})).unwrap()
    };
    core.save_rule(make_rule("password")).await.unwrap();
    let client = flow::client(&core).await;
    sim.counts.pause_next_search();
    let task = {
        let core = core.clone();
        tokio::spawn(async move {
            core.run_tool(&client, "search_read", json!({"env":"Protected", "model":"res.partner", "fields":["id"], "domain":[["user_id.name", "=", "test"]]})).await
        })
    };
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        sim.counts.wait_for_search(),
    )
    .await
    .unwrap();
    core.save_rule(make_rule("name")).await.unwrap();
    sim.counts.release_search();
    assert!(task.await.unwrap().is_err());
    assert_eq!(
        sim.counts
            .calls()
            .iter()
            .filter(|call| call[4] == "search_read")
            .count(),
        1
    );
    let event = core
        .activity(None, ActivityFilter::default())
        .await
        .unwrap()
        .events
        .into_iter()
        .find(|event| event.operation == "search_read:res.partner")
        .unwrap();
    let details = core
        .activity_details(event.request_id.unwrap())
        .await
        .unwrap()
        .unwrap();
    assert!(details.response.is_none());
}

#[tokio::test]
async fn related_domain_uses_target_model_rules_and_metadata() {
    let tmp = tempfile::tempdir().unwrap();
    let sim = support::simulator().await;
    let core = support::unlocked_core(&tmp).await;
    core.save_environment(support::draft("Protected", &sim.url))
        .await
        .unwrap();
    let rule: RuleDraft = serde_json::from_value(json!({"environment_id":"", "model":"res.users", "method":"all", "kind":"fields", "action":"block", "scope":{"kind":"global"}, "operations":["read"], "model_selector":{"kind":"exact","value":"res.users"}, "fields":["password"], "context":{}, "max_records":0})).unwrap();
    core.save_rule(rule).await.unwrap();
    let client = flow::client(&core).await;
    let query = |field| json!({"env":"Protected", "model":"res.partner", "fields":["id"], "domain":[[field,"=","x"]]});
    core.run_tool(&client, "search_read", query("user_id.name"))
        .await
        .unwrap();
    let count = sim
        .counts
        .calls()
        .iter()
        .filter(|call| call[4] == "search_read")
        .count();
    assert_eq!(count, 1);
    assert!(
        sim.counts
            .calls()
            .iter()
            .any(|call| call[4] == "fields_get" && call[3] == "res.partner")
    );
    assert!(
        sim.counts
            .calls()
            .iter()
            .any(|call| call[4] == "fields_get" && call[3] == "res.users")
    );
    assert!(
        core.run_tool(&client, "search_read", query("user_id.password"))
            .await
            .is_err()
    );
    assert_eq!(
        sim.counts
            .calls()
            .iter()
            .filter(|call| call[4] == "search_read")
            .count(),
        count
    );
}

#[tokio::test]
async fn protected_related_domain_never_sends_data_query() {
    let tmp = tempfile::tempdir().unwrap();
    let sim = support::simulator().await;
    let core = support::unlocked_core(&tmp).await;
    core.save_environment(support::draft("Protected", &sim.url))
        .await
        .unwrap();
    let rule: RuleDraft = serde_json::from_value(json!({"environment_id":"", "model":"res.users", "method":"all", "kind":"fields", "action":"block", "scope":{"kind":"global"}, "operations":["read"], "model_selector":{"kind":"exact","value":"res.users"}, "fields":["password"], "context":{}, "max_records":0})).unwrap();
    core.save_rule(rule).await.unwrap();
    let client = flow::client(&core).await;
    let error = core.run_tool(&client, "search_read", json!({"env":"Protected", "model":"res.partner", "fields":["id"], "domain":[["user_id.password", "=", "secret"]]})).await.unwrap_err();
    assert_eq!(error.outcome, odx_core::Outcome::NotSent);
    assert!(
        sim.counts
            .calls()
            .iter()
            .any(|call| call[4] == "fields_get")
    );
    assert!(
        !sim.counts
            .calls()
            .iter()
            .any(|call| call[4] == "search_read")
    );
}

#[tokio::test]
async fn deleted_read_rule_still_protects_response_and_activity() {
    for method in ["read", "fields_get"] {
        let tmp = tempfile::tempdir().unwrap();
        let sim = support::simulator().await;
        let core = support::unlocked_core(&tmp).await;
        core.save_environment(support::draft("Protected", &sim.url))
            .await
            .unwrap();
        let protected = if method == "fields_get" { "name" } else { "id" };
        let rule: RuleDraft = serde_json::from_value(json!({
            "environment_id":"", "model":"", "method":"all", "kind":"fields",
            "action":"block", "scope":{"kind":"global"}, "operations":["read"],
            "model_selector":{"kind":"all"}, "fields":[protected], "context":{}, "max_records":0
        }))
        .unwrap();
        let saved = core.save_rule(rule).await.unwrap();
        let client = flow::client(&core).await;
        sim.counts.pause_next_read();
        let mut task = {
            let core = core.clone();
            tokio::spawn(async move {
                let args = if method == "fields_get" {
                    json!({"env":"Protected", "model":"res.partner", "method":"fields_get", "args":[], "kwargs":{}})
                } else {
                    json!({"env":"Protected", "model":"res.partner", "method":"read", "args":[[1]], "kwargs":{"fields":["id", "display_name"]}})
                };
                core.run_tool(&client, "call", args).await
            })
        };
        tokio::select! {
            _ = sim.counts.wait_for_read() => {},
            result = &mut task => panic!("{method} completed before reaching simulator: {result:?}"),
            _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => panic!("read did not reach simulator"),
        }
        core.delete_rule(&saved.rule.id).await.unwrap();
        sim.counts.release_read();
        let data = task.await.unwrap().unwrap().data;
        if method == "fields_get" {
            assert!(data.get("name").is_none());
        } else {
            assert_eq!(data[0]["id"], "[REDACTED]");
        }
        let event = core
            .activity(None, ActivityFilter::default())
            .await
            .unwrap()
            .events
            .into_iter()
            .find(|e| e.operation == format!("{method}:res.partner") && e.outcome == "succeeded")
            .unwrap();
        let details = core
            .activity_details(event.request_id.unwrap())
            .await
            .unwrap()
            .unwrap();
        let response = details.response.unwrap().text;
        if method == "fields_get" {
            assert!(!response.contains("\"name\""));
        } else {
            assert!(!response.contains("\"id\":1"));
        }
        let sent = sim
            .counts
            .calls()
            .into_iter()
            .find(|a| a[4] == method)
            .unwrap();
        if method == "read" {
            assert!(!sent[6]["fields"].as_array().unwrap().contains(&json!("id")));
        }
    }
}

#[tokio::test]
async fn field_block_filters_upstream_selection_and_marks_rows() {
    let tmp = tempfile::tempdir().unwrap();
    let sim = support::simulator().await;
    let core = support::unlocked_core(&tmp).await;
    core.save_environment(support::draft("Protected", &sim.url))
        .await
        .unwrap();
    let draft: RuleDraft = serde_json::from_value(json!({
        "environment_id":"", "model":"", "method":"all", "kind":"fields",
        "action":"block", "scope":{"kind":"global"}, "operations":["read"],
        "model_selector":{"kind":"all"}, "field_selectors":[{"kind":"regex","pattern":"(?i)(token|password)"}],
        "context":{}, "max_records":0
    })).unwrap();
    core.save_rule(draft).await.unwrap();
    let client = flow::client(&core).await;
    let result = core.run_tool(&client, "search_read", json!({
        "env":"Protected", "model":"res.partner", "fields":["id","name","api_token","password"]
    })).await.unwrap();
    assert_eq!(result.data[0]["api_token"], "[REDACTED]");
    assert_eq!(result.data[0]["password"], "[REDACTED]");
    assert_eq!(
        result.data[0]["__odx_redacted_fields"],
        json!(["api_token", "password"])
    );
    let event = core
        .activity(None, ActivityFilter::default())
        .await
        .unwrap()
        .events
        .into_iter()
        .find(|event| event.operation == "search_read:res.partner" && event.outcome == "succeeded")
        .unwrap();
    let details = core
        .activity_details(event.request_id.unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        details.redacted_fields,
        Some(vec!["api_token".to_owned(), "password".to_owned()])
    );
    assert_eq!(details.policy_rules.as_ref().unwrap().len(), 1);
    assert_eq!(details.policy_rules.as_ref().unwrap()[0].rule_revision, 1);
    let call = sim
        .counts
        .calls()
        .into_iter()
        .find(|a| a.get(4) == Some(&json!("search_read")))
        .unwrap();
    assert_eq!(call[6]["fields"], json!(["id", "name"]));
    let defaulted = core
        .run_tool(
            &client,
            "search_read",
            json!({"env":"Protected", "model":"res.partner"}),
        )
        .await
        .unwrap();
    assert!(defaulted.data[0].get("password").is_none());
    let default_fields = sim
        .counts
        .calls()
        .into_iter()
        .rev()
        .find(|a| a.get(4) == Some(&json!("search_read")))
        .unwrap();
    assert_eq!(default_fields[6]["fields"], json!(["id", "display_name"]));
    let literal_value_is_not_a_field_name = core
        .run_tool(
            &client,
            "search_read",
            json!({
                "env":"Protected", "model":"res.partner", "fields":["id"],
                "domain":[["name", "=", "password"]]
            }),
        )
        .await;
    assert!(literal_value_is_not_a_field_name.is_ok());
    let after_default = sim.counts.calls().len();
    let inferred = core.run_tool(&client, "search_read", json!({
        "env":"Protected", "model":"res.partner", "fields":["id"], "domain":[["password", "=", "hidden"]]
    })).await.unwrap_err();
    assert_eq!(inferred.code, "forbidden");
    let ordered_secret = core
        .run_tool(
            &client,
            "search_read",
            json!({"env":"Protected", "model":"res.partner", "fields":["id"], "order":"password desc"}),
        )
        .await
        .unwrap_err();
    assert_eq!(ordered_secret.code, "forbidden");
    let call_read = core.run_tool(&client, "call", json!({
        "env":"Protected", "model":"res.partner", "method":"search_read", "args":[], "kwargs":{"fields":["id", "password"]}
    })).await.unwrap();
    assert_eq!(call_read.data[0]["password"], "[REDACTED]");
    let call_fields = sim
        .counts
        .calls()
        .into_iter()
        .rev()
        .find(|a| a.get(4) == Some(&json!("search_read")))
        .unwrap();
    assert_eq!(call_fields[6]["fields"], json!(["id"]));
    let metadata = core
        .run_tool(
            &client,
            "call",
            json!({
                "env":"Protected", "model":"res.partner", "method":"fields_get",
                "args":[], "kwargs":{"allfields":["name", "password", "api_token"]}
            }),
        )
        .await
        .unwrap();
    assert!(metadata.data.get("name").is_some());
    assert!(metadata.data.get("password").is_none());
    assert!(metadata.data.get("api_token").is_none());
    let metadata_call = sim
        .counts
        .calls()
        .into_iter()
        .rev()
        .find(|a| a.get(4) == Some(&json!("fields_get")))
        .unwrap();
    assert_eq!(
        metadata_call[6]["allfields"],
        json!(["name", "password", "api_token"])
    );
    let opaque = core
        .run_tool(
            &client,
            "call",
            json!({"env":"Protected", "model":"res.partner", "method":"read_secret_scalar", "args":[], "kwargs":{}}),
        )
        .await
        .unwrap_err();
    assert_eq!(opaque.code, "forbidden");
    let positional_read = core
        .run_tool(
            &client,
            "call",
            json!({"env":"Protected", "model":"res.partner", "method":"search_read", "args":[[], ["id", "password"]], "kwargs":{}}),
        )
        .await
        .unwrap();
    assert_eq!(positional_read.data[0]["password"], "[REDACTED]");
    let positional_call = sim
        .counts
        .calls()
        .into_iter()
        .rev()
        .find(|a| a.get(4) == Some(&json!("search_read")))
        .unwrap();
    assert_eq!(positional_call[6]["fields"], json!(["id"]));
    let relational = core
        .run_tool(
            &client,
            "search_read",
            json!({"env":"Protected", "model":"res.partner", "fields":["id", "partner_id.password"]}),
        )
        .await
        .unwrap_err();
    assert_eq!(relational.code, "invalid_arguments");
    assert_eq!(
        sim.counts.calls().len(),
        after_default + 3,
        "blocked read queries must never reach Odoo"
    );
    // Value-free reads (`search`, `search_count`) keep working on safe domains while still
    // rejecting inference on blocked fields.
    assert!(
        core.run_tool(
            &client,
            "search",
            json!({"env":"Protected", "model":"res.partner", "domain":[["name", "=", "Ada"]]}),
        )
        .await
        .is_ok()
    );
    assert!(
        core.run_tool(
            &client,
            "search_count",
            json!({"env":"Protected", "model":"res.partner", "domain":[["name", "=", "Ada"]]}),
        )
        .await
        .is_ok()
    );
    let unsafe_search = core
        .run_tool(
            &client,
            "search",
            json!({"env":"Protected", "model":"res.partner", "domain":[["password", "=", "hidden"]]}),
        )
        .await
        .unwrap_err();
    assert_eq!(unsafe_search.code, "forbidden");
    let id_block: RuleDraft = serde_json::from_value(json!({
        "environment_id":"", "model":"", "method":"all", "kind":"fields",
        "action":"block", "scope":{"kind":"global"}, "operations":["read"],
        "model_selector":{"kind":"all"}, "field_selectors":[{"kind":"exact","value":"id"}],
        "context":{}, "max_records":0
    }))
    .unwrap();
    let id_block = core.save_rule(id_block).await.unwrap();
    let before_fully_blocked = sim.counts.calls().len();
    let error = core
        .run_tool(
            &client,
            "search_read",
            json!({
                "env":"Protected", "model":"res.partner", "fields":["api_token", "password"]
            }),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "forbidden");
    assert_eq!(sim.counts.calls().len(), before_fully_blocked);
    assert_eq!(error.outcome, odx_core::Outcome::NotSent);
    let denied_events = core
        .activity(None, ActivityFilter::default())
        .await
        .unwrap()
        .events
        .into_iter()
        .filter(|event| event.operation == "search_read:res.partner" && event.outcome == "denied")
        .collect::<Vec<_>>();
    let mut found_deciding_rule = false;
    for event in denied_events {
        let details = core
            .activity_details(event.request_id.unwrap())
            .await
            .unwrap()
            .unwrap();
        found_deciding_rule |= details.policy_rules.is_some_and(|rules| {
            rules.iter().any(|rule| {
                rule.rule_id == id_block.rule.id && rule.rule_revision == id_block.rule.revision
            })
        });
    }
    assert!(found_deciding_rule);
}

#[tokio::test]
async fn a_new_rule_sanitizes_an_already_in_flight_read_before_audit_and_return() {
    let tmp = tempfile::tempdir().unwrap();
    let sim = support::simulator().await;
    let core = support::unlocked_core(&tmp).await;
    core.save_environment(support::draft("Race", &sim.url))
        .await
        .unwrap();
    let client = flow::client(&core).await;
    sim.counts.hold_database("base-a");
    let (core_call, client_call) = (core.clone(), client.clone());
    let call = tokio::spawn(async move {
        core_call
            .run_tool(
                &client_call,
                "search_read",
                json!({
                    "env":"Race", "model":"res.partner", "fields":["id", "password"]
                }),
            )
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while sim.counts.in_flight("base-a") == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let draft: RuleDraft = serde_json::from_value(json!({
        "environment_id":"", "model":"", "method":"all", "kind":"fields",
        "action":"block", "scope":{"kind":"global"}, "operations":["read"],
        "model_selector":{"kind":"all"}, "field_selectors":[{"kind":"regex","pattern":"(?i)(password)"}],
        "context":{}, "max_records":0
    })).unwrap();
    let saved = core.save_rule(draft).await.unwrap();
    sim.counts.release_databases();
    let result = call.await.unwrap().unwrap();
    assert_eq!(result.data[0]["password"], "[REDACTED]");
    let event = core
        .activity(None, ActivityFilter::default())
        .await
        .unwrap()
        .events
        .into_iter()
        .find(|event| event.operation == "search_read:res.partner" && event.outcome == "succeeded")
        .unwrap();
    let details = core
        .activity_details(event.request_id.unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(details.policy_rules.unwrap()[0].rule_id, saved.rule.id);
    let response = details.response.unwrap().text;
    assert!(!response.contains("fixture-secret-password"));
}

#[tokio::test]
async fn global_field_rules_apply_to_two_and_future_environments_but_local_blocks_do_not() {
    let tmp = tempfile::tempdir().unwrap();
    let sim = support::simulator().await;
    let core = support::unlocked_core(&tmp).await;
    let first = core
        .save_environment(support::draft("First", &sim.url))
        .await
        .unwrap();
    let mut second_draft = support::draft("Second", &sim.url);
    second_draft.database = "base-b".into();
    let second = core.save_environment(second_draft).await.unwrap();

    let global: RuleDraft = serde_json::from_value(json!({
        "environment_id":"", "model":"", "method":"all", "kind":"fields",
        "action":"block", "scope":{"kind":"global"}, "operations":["read"],
        "model_selector":{"kind":"all"}, "field_selectors":[{"kind":"regex","pattern":"(?i)password"}],
        "context":{}, "max_records":0
    }))
    .unwrap();
    let global = core.save_rule(global).await.unwrap();
    let local: RuleDraft = serde_json::from_value(json!({
        "environment_id":first.id, "model":"res.partner", "method":"all", "kind":"method",
        "action":"block", "scope":{"kind":"environment"}, "operations":["read"],
        "model_selector":{"kind":"exact","value":"res.partner"}, "context":{}, "max_records":0
    }))
    .unwrap();
    let local = core.save_rule(local).await.unwrap();
    let client = flow::client(&core).await;

    let local_denied = core
        .run_tool(
            &client,
            "search_read",
            json!({"env":"First", "model":"res.partner", "fields":["id", "password"]}),
        )
        .await
        .unwrap_err();
    assert_eq!(local_denied.code, "forbidden");
    let second_read = core
        .run_tool(
            &client,
            "search_read",
            json!({"env":"Second", "model":"res.partner", "fields":["id", "password"]}),
        )
        .await
        .unwrap();
    assert_eq!(second_read.data[0]["password"], "[REDACTED]");

    let mut future_draft = support::draft("Future", &sim.url);
    future_draft.database = "base-c".into();
    core.save_environment(future_draft).await.unwrap();
    let future_read = core
        .run_tool(
            &client,
            "search_read",
            json!({"env":"Future", "model":"res.partner", "fields":["id", "password"]}),
        )
        .await
        .unwrap();
    assert_eq!(future_read.data[0]["password"], "[REDACTED]");
    let first_rules = core.rules(Some(&first.id)).await.unwrap();
    let second_rules = core.rules(Some(&second.id)).await.unwrap();
    assert_eq!(first_rules.len(), 2);
    assert_eq!(second_rules.len(), 1);
    core.delete_environment(&first.id, true).await.unwrap();
    let remaining = core.rules(Some(&second.id)).await.unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].rule.id, global.rule.id);
    assert!(
        core.rules(None)
            .await
            .unwrap()
            .iter()
            .all(|r| r.rule.id != local.rule.id)
    );
}

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
