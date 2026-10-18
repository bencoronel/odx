//! Automatic approval rules and write modes: pure validation/matching and run_tool integration against simulated Odoo.
mod flow;
mod support;
use chrono::{Duration, Utc};
use odx_core::{
    ActivityFilter, Authorization, ClientIdentity, Core, EnvironmentDraft, Protocol, Rule,
    RuleDraft, WriteMode,
    policy::{Effect, EffectiveCall},
    tools::mutations::call_effect,
};
use serde_json::{Map, Value, json};
use std::sync::Arc;

// Validation and matching

fn merged(patch: Value) -> Value {
    let mut base = json!({
        "environment_id": "e", "model": "res.partner", "method": "write", "kind": "method",
        "context": {"lang": "es_ES"}, "max_records": 2
    });
    for (k, v) in patch.as_object().unwrap() {
        if v.is_null() {
            base.as_object_mut().unwrap().remove(k);
        } else {
            base[k] = v.clone();
        }
    }
    base
}
fn draft(patch: Value) -> Result<RuleDraft, serde_json::Error> {
    serde_json::from_value(merged(patch))
}
fn rule(patch: Value) -> Rule {
    draft(patch).unwrap().into_rule(Utc::now()).unwrap()
}
fn rejected(patch: Value) -> bool {
    draft(patch).map_or(true, |d| d.into_rule(Utc::now()).is_err())
}
/// Match the effective call with the effect assigned by real validation.
fn hits(rule: &Rule, method: &str, args: Value, mut kwargs: Value) -> bool {
    hits_in(
        rule,
        "e",
        "res.partner",
        method,
        args,
        &mut kwargs,
        json!({"lang":"es_ES"}),
    )
}
fn hits_in(
    rule: &Rule,
    env: &str,
    model: &str,
    method: &str,
    args: Value,
    kwargs: &mut Value,
    context: Value,
) -> bool {
    let named: Map<String, Value> = kwargs.as_object().unwrap().clone();
    let effect = call_effect(method, args.as_array().unwrap(), &named, &[]).unwrap();
    if !context.is_null() {
        kwargs["context"] = context;
    }
    rule.matches(
        &EffectiveCall {
            environment_id: env,
            model,
            method,
            args: args.as_array().unwrap(),
            kwargs: kwargs.as_object().unwrap(),
        },
        &effect,
        Utc::now(),
    )
}

#[test]
fn unrepresentable_rules_are_rejected_instead_of_broadened() {
    for patch in [
        // Rules never authorize deletes.
        json!({"method":"unlink"}),
        json!({"method":"write","kind":"values","values":{"tag_ids":[[3,8,0]]}}),
        json!({"method":"create","kind":"values","values":{"tag_ids":[[6,0,[1]]]}}),
        // Field/value rules apply only to create/write.
        json!({"method":"action_confirm","kind":"fields","fields":["state"]}),
        json!({"method":"action_confirm","kind":"values","values":{"state":"done"}}),
        // Empty or malformed sets.
        json!({"kind":"fields","fields":[]}),
        json!({"kind":"values","values":{}}),
        json!({"kind":"values","values":[{"name":"x"}]}),
        json!({"kind":"fields","fields":["name","name"]}),
        json!({"kind":"fields","fields":["bad field"]}),
        json!({"kind":"fields"}),
        json!({"kind":"values"}),
        // Reject ignored conditions: method rules with fields would broaden access.
        json!({"kind":"method","fields":["name"]}),
        json!({"kind":"method","values":{"name":"x"}}),
        json!({"kind":"fields","fields":["name"],"values":{"name":"x"}}),
        // Count, expiry, context and unknown fields.
        json!({"max_records":0}),
        json!({"max_records":1001}),
        json!({"max_records":-1}),
        json!({"expires_at": (Utc::now() - Duration::seconds(1)).to_rfc3339()}),
        json!({"context":null}),
        json!({"context":[]}),
        json!({"limit":5}),
        // Reads, private methods and credentials.
        json!({"method":"search_read"}),
        json!({"method":"_write"}),
        json!({"model":"res.users","method":"api_key_wizard"}),
        json!({"model":"res.users.apikeys.description","method":"make_key"}),
        json!({"model":"res..partner"}),
    ] {
        assert!(rejected(patch.clone()), "{patch}");
    }
    let fields = rule(json!({"kind":"fields","fields":["name","email"]}));
    assert_eq!(fields.fields, Some(vec!["email".into(), "name".into()]));
    assert_eq!(fields.revision, 1);
    assert!(fields.values.is_none());
}

#[test]
fn model_method_environment_and_count_must_match() {
    let r = rule(json!({}));
    assert!(hits(&r, "write", json!([[1,2],{"name":"x"}]), json!({})));
    assert!(!hits(&r, "write", json!([[1,2,3],{"name":"x"}]), json!({})));
    let mut kw = json!({});
    let ctx = json!({"lang":"es_ES"});
    assert!(!hits_in(
        &r,
        "other",
        "res.partner",
        "write",
        json!([[1],{"name":"x"}]),
        &mut kw,
        ctx.clone()
    ));
    let mut kw = json!({});
    assert!(!hits_in(
        &r,
        "e",
        "res.users",
        "write",
        json!([[1],{"name":"x"}]),
        &mut kw,
        ctx
    ));
    assert!(!hits(&r, "create", json!([{"name":"x"}]), json!({})));
    // create counts submitted records.
    let c = rule(json!({"method":"create"}));
    assert!(hits(
        &c,
        "create",
        json!([[{"name":"a"},{"name":"b"}]]),
        json!({})
    ));
    assert!(!hits(
        &c,
        "create",
        json!([[{"name":"a"},{"name":"b"},{"name":"c"}]]),
        json!({})
    ));
}

#[test]
fn fields_and_values_rules_compare_validated_payloads() {
    let f = rule(json!({"kind":"fields","fields":["name","email"]}));
    assert!(hits(&f, "write", json!([[1],{"name":"x"}]), json!({})));
    assert!(hits(
        &f,
        "write",
        json!([[1],{"name":"x","email":"e"}]),
        json!({})
    ));
    assert!(!hits(
        &f,
        "write",
        json!([[1],{"name":"x","phone":"1"}]),
        json!({})
    ));
    let fc = rule(json!({"method":"create","kind":"fields","fields":["name"]}));
    assert!(hits(
        &fc,
        "create",
        json!([[{"name":"a"},{"name":"b"}]]),
        json!({})
    ));
    assert!(!hits(
        &fc,
        "create",
        json!([[{"name":"a"},{"email":"b"}]]),
        json!({})
    ));

    let v = rule(json!({"kind":"values","values":{"active":false,"ref":1}}));
    assert!(hits(
        &v,
        "write",
        json!([[1],{"ref":1,"active":false}]),
        json!({})
    ));
    assert!(!hits(&v, "write", json!([[1],{"active":false}]), json!({})));
    assert!(!hits(
        &v,
        "write",
        json!([[1],{"active":true,"ref":1}]),
        json!({})
    ));
    // Exact JSON comparison distinguishes 1.0, 1 and "1".
    assert!(!hits(
        &v,
        "write",
        json!([[1],{"active":false,"ref":1.0}]),
        json!({})
    ));
    assert!(!hits(
        &v,
        "write",
        json!([[1],{"active":false,"ref":"1"}]),
        json!({})
    ));
    let vc = rule(json!({"method":"create","kind":"values","values":{"name":"a"}}));
    assert!(hits(
        &vc,
        "create",
        json!([[{"name":"a"},{"name":"a"}]]),
        json!({})
    ));
    assert!(!hits(
        &vc,
        "create",
        json!([[{"name":"a"},{"name":"b"}]]),
        json!({})
    ));
}

#[test]
fn kwargs_and_positional_arguments_are_the_same_effective_call() {
    let f = rule(json!({"kind":"fields","fields":["name"]}));
    assert!(hits(
        &f,
        "write",
        json!([]),
        json!({"ids":[1],"vals":{"name":"x"}})
    ));
    assert!(hits(
        &f,
        "write",
        json!([[1]]),
        json!({"vals":{"name":"x"}})
    ));
    assert!(!hits(
        &f,
        "write",
        json!([]),
        json!({"ids":[1],"vals":{"email":"x"}})
    ));
    let c = rule(json!({"method":"create","kind":"values","values":{"name":"x"}}));
    assert!(hits(
        &c,
        "create",
        json!([]),
        json!({"vals_list":[{"name":"x"}]})
    ));
    assert!(hits(&c, "create", json!([{"name":"x"}]), json!({})));
    // Unknown named arguments make effects indeterminate.
    assert!(!hits(
        &f,
        "write",
        json!([[1],{"name":"x"}]),
        json!({"extra":true})
    ));
    let m = rule(json!({}));
    assert!(hits(
        &m,
        "write",
        json!([]),
        json!({"ids":[1,2],"vals":{"name":"x"}})
    ));
    assert!(!hits(
        &m,
        "write",
        json!([]),
        json!({"ids":[1,2,3],"vals":{"name":"x"}})
    ));
}

#[test]
fn the_full_effective_context_must_match_exactly() {
    let r = rule(json!({}));
    let call = |context: Value| {
        let mut kw = json!({});
        hits_in(
            &r,
            "e",
            "res.partner",
            "write",
            json!([[1],{"name":"x"}]),
            &mut kw,
            context,
        )
    };
    assert!(call(json!({"lang":"es_ES"})));
    assert!(!call(json!({"lang":"es_ES","allowed_company_ids":[2]})));
    assert!(!call(json!({"lang":"en_US"})));
    assert!(!call(json!({})));
    assert!(!call(Value::Null));
    // Empty context matches only empty context.
    let empty = rule(json!({"context":{}}));
    let mut kw = json!({});
    assert!(hits_in(
        &empty,
        "e",
        "res.partner",
        "write",
        json!([[1],{"name":"x"}]),
        &mut kw,
        json!({})
    ));
    let mut kw = json!({});
    assert!(!hits_in(
        &empty,
        "e",
        "res.partner",
        "write",
        json!([[1],{"name":"x"}]),
        &mut kw,
        json!({"lang":"es_ES"})
    ));
}

#[test]
fn unknown_record_counts_and_expired_rules_never_match() {
    let m = rule(json!({"method":"action_confirm","max_records":2}));
    assert!(hits(&m, "action_confirm", json!([[3, 5]]), json!({})));
    assert!(hits(&m, "action_confirm", json!([]), json!({"ids":[3]})));
    assert!(!hits(&m, "action_confirm", json!([[3, 5, 7]]), json!({})));
    // Without explicit IDs, record count is unknown.
    assert!(!hits(&m, "action_confirm", json!([]), json!({})));
    assert!(!hits(&m, "action_confirm", json!(["x"]), json!({})));
    assert!(!hits(&m, "action_confirm", json!([[]]), json!({})));
    assert!(!hits(&m, "action_confirm", json!([[1, 1]]), json!({})));
    assert!(!hits(&m, "action_confirm", json!([[0]]), json!({})));
    assert!(!hits(
        &m,
        "action_confirm",
        json!([[1]]),
        json!({"ids":[2]})
    ));
    let mut expired = rule(json!({"method":"action_confirm"}));
    expired.expires_at = Some(Utc::now() - Duration::seconds(1));
    assert!(expired.is_expired(Utc::now()));
    assert!(!hits(&expired, "action_confirm", json!([[3]]), json!({})));
}

#[test]
fn deletions_are_never_authorized_by_a_rule() {
    let m = rule(json!({}));
    // Relational deletion commands produce Delete effects.
    for command in [
        json!([2, 8, 0]),
        json!([3, 8, 0]),
        json!([5, 0, 0]),
        json!([6, 0, [3]]),
    ] {
        assert!(!hits(
            &m,
            "write",
            json!([[1],{"tag_ids":[command]}]),
            json!({})
        ));
    }
    let custom = rule(json!({"method":"action_custom","max_records":5}));
    assert!(!hits(
        &custom,
        "action_custom",
        json!([[1],{"tags":[[2,8]]}]),
        json!({})
    ));
    // Tampered unlink rules cannot authorize calls.
    let mut forged = rule(json!({}));
    forged.method = "unlink".into();
    let call = EffectiveCall {
        environment_id: "e",
        model: "res.partner",
        method: "unlink",
        args: &[json!([1])],
        kwargs: &Map::from_iter([("context".into(), json!({"lang":"es_ES"}))]),
    };
    assert!(!forged.matches(&call, &Effect::Delete, Utc::now()));
    assert!(!forged.matches(&call, &Effect::Write, Utc::now()));
}

// Simulated Odoo integration

struct Fixture {
    sim: support::Simulator,
    core: Arc<Core>,
    env: String,
    client: ClientIdentity,
    _tmp: tempfile::TempDir,
}
async fn fixture(mode: WriteMode) -> Fixture {
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
            context: json!({"lang":"es_ES"}),
            secret: "fixture-secret".into(),
        })
        .await
        .unwrap()
        .id;
    match mode {
        WriteMode::ReadOnly => {}
        WriteMode::Temporary => core.set_temporary_write(&env, 5).await.unwrap(),
        mode => core.set_mode(&env, mode).await.unwrap(),
    }
    let client = flow::client(&core).await;
    Fixture {
        sim,
        core,
        env,
        client,
        _tmp: tmp,
    }
}
fn save(f: &Fixture, patch: Value) -> RuleDraft {
    let mut v = merged(patch);
    v["environment_id"] = json!(f.env);
    serde_json::from_value(v).unwrap()
}
fn write(values: Value) -> Value {
    json!({"env":"A","model":"res.partner","ids":[1],"values":values})
}
async fn last_authorization(core: &Core) -> Option<Authorization> {
    core.activity(None, ActivityFilter::default())
        .await
        .unwrap()
        .events
        .into_iter()
        .find(|e| e.outcome == "succeeded" && e.write)
        .and_then(|e| e.details?.authorization)
}

#[tokio::test]
async fn a_matching_rule_sends_without_human_and_variants_wait_for_one() {
    let f = fixture(WriteMode::Approval).await;
    let saved = f
        .core
        .save_rule(save(&f, json!({"kind":"fields","fields":["name"]})))
        .await
        .unwrap();
    let ok = f
        .core
        .run_tool(&f.client, "write", write(json!({"name":"x"})))
        .await
        .unwrap();
    assert_eq!(ok.data, json!(true));
    assert_eq!(f.sim.counts.writes(), 1);
    assert_eq!(
        last_authorization(&f.core).await,
        Some(Authorization::Rule {
            rule_id: saved.rule.id.clone(),
            rule_revision: 1
        })
    );
    // Named arguments through call match the same rule.
    f.core
        .run_tool(
            &f.client,
            "call",
            json!({"env":"A","model":"res.partner","method":"write","kwargs":{"ids":[1],"vals":{"name":"y"}}}),
        )
        .await
        .unwrap();
    assert_eq!(f.sim.counts.writes(), 2);
    // Different fields/context or excess records require a decision before dispatch.
    for arguments in [
        write(json!({"email":"x"})),
        {
            let mut a = write(json!({"name":"x"}));
            a["context"] = json!({"allowed_company_ids":[2]});
            a
        },
        json!({"env":"A","model":"res.partner","ids":[1,2,3],"values":{"name":"x"}}),
    ] {
        let (call, pending) = flow::queue(&f.core, &f.client, "write", arguments).await;
        assert_eq!(f.sim.counts.writes(), 2);
        f.core.decide(&pending.id, false).await.unwrap();
        assert_eq!(flow::error_code(call).await, "approval_rejected");
    }
    // Audit distinguishes individual approval.
    let (call, pending) =
        flow::queue(&f.core, &f.client, "write", write(json!({"email":"x"}))).await;
    f.core.decide(&pending.id, true).await.unwrap();
    call.await.unwrap().unwrap();
    assert_eq!(
        last_authorization(&f.core).await,
        Some(Authorization::Individual)
    );
}

#[tokio::test]
async fn custom_methods_need_an_explicit_rule_and_a_known_count() {
    let f = fixture(WriteMode::Approval).await;
    f.core
        .save_rule(save(&f, json!({"model":"sale.order","method":"action_confirm","context":{"lang":"es_ES"},"max_records":2})))
        .await
        .unwrap();
    f.core
        .run_tool(
            &f.client,
            "call",
            json!({"env":"A","model":"sale.order","method":"action_confirm","args":[[3,5]]}),
        )
        .await
        .unwrap();
    assert_eq!(f.sim.counts.writes(), 1);
    for arguments in [
        // Unknown/excess record counts require individual approval.
        json!({"env":"A","model":"sale.order","method":"action_confirm","args":[]}),
        json!({"env":"A","model":"sale.order","method":"action_confirm","args":[[3,5,7]]}),
        // Other custom methods are not covered.
        json!({"env":"A","model":"sale.order","method":"action_cancel","args":[[3]]}),
    ] {
        let (call, pending) = flow::queue(&f.core, &f.client, "call", arguments).await;
        f.core.decide(&pending.id, false).await.unwrap();
        assert_eq!(flow::error_code(call).await, "approval_rejected");
    }
    assert_eq!(f.sim.counts.writes(), 1);
}

#[tokio::test]
async fn unlink_and_relational_deletes_stay_individual_in_every_writable_mode() {
    for mode in [
        WriteMode::Approval,
        WriteMode::Temporary,
        WriteMode::Permanent,
    ] {
        let f = fixture(mode).await;
        assert!(
            f.core
                .save_rule(save(&f, json!({"method":"unlink"})))
                .await
                .is_err()
        );
        f.core
            .save_rule(save(&f, json!({"max_records":5})))
            .await
            .unwrap();
        for (tool, arguments) in [
            ("unlink", json!({"env":"A","model":"res.partner","ids":[1]})),
            (
                "call",
                json!({"env":"A","model":"res.partner","method":"unlink","args":[[1]]}),
            ),
            ("write", write(json!({"tag_ids":[[3,8,0]]}))),
        ] {
            let (call, pending) = flow::queue(&f.core, &f.client, tool, arguments).await;
            assert_eq!(f.sim.counts.writes(), 0, "{mode:?} {tool}");
            f.core.decide(&pending.id, false).await.unwrap();
            assert_eq!(flow::error_code(call).await, "approval_rejected");
        }
    }
}

#[tokio::test]
async fn rules_do_not_open_read_only_and_free_mode_is_audited_as_free() {
    let f = fixture(WriteMode::ReadOnly).await;
    f.core.save_rule(save(&f, json!({}))).await.unwrap();
    let denied = f
        .core
        .run_tool(&f.client, "write", write(json!({"name":"x"})))
        .await
        .unwrap_err();
    assert_eq!(denied.code, "forbidden");
    f.core.set_mode(&f.env, WriteMode::Permanent).await.unwrap();
    f.core
        .run_tool(&f.client, "write", write(json!({"name":"x"})))
        .await
        .unwrap();
    assert_eq!(last_authorization(&f.core).await, Some(Authorization::Free));
}

#[tokio::test]
async fn deleting_or_editing_a_rule_stops_requests_not_yet_sent() {
    let f = fixture(WriteMode::Approval).await;
    let rule = f.core.save_rule(save(&f, json!({}))).await.unwrap().rule;
    // Hold rule-authorized execution during login, before dispatch.
    f.sim.counts.pause_next_login();
    let call = {
        let (core, client) = (f.core.clone(), f.client.clone());
        tokio::spawn(async move {
            core.run_tool(&client, "write", write(json!({"name":"x"})))
                .await
        })
    };
    f.sim.counts.wait_for_login().await;
    f.core.delete_rule(&rule.id).await.unwrap();
    f.sim.counts.release_login();
    assert!(call.await.unwrap().is_err());
    assert_eq!(f.sim.counts.writes(), 0);
    assert!(f.core.rules(Some(&f.env)).await.unwrap().is_empty());

    // Edits increment revision and block older authorizations.
    let rule = f.core.save_rule(save(&f, json!({}))).await.unwrap().rule;
    f.core.lock().await;
    f.core.unlock("fictional master password").await.unwrap();
    let client = flow::client(&f.core).await;
    f.sim.counts.pause_next_login();
    let call = {
        let core = f.core.clone();
        tokio::spawn(async move {
            core.run_tool(&client, "write", write(json!({"name":"x"})))
                .await
        })
    };
    f.sim.counts.wait_for_login().await;
    let mut edit = save(&f, json!({"max_records":1}));
    edit.id = Some(rule.id.clone());
    let edited = f.core.save_rule(edit).await.unwrap().rule;
    assert_eq!(edited.revision, 2);
    assert_eq!(edited.id, rule.id);
    f.sim.counts.release_login();
    assert!(call.await.unwrap().is_err());
    assert_eq!(f.sim.counts.writes(), 0);
}

#[tokio::test]
async fn an_environment_change_revokes_rule_authorized_work_and_deletion_removes_rules() {
    let f = fixture(WriteMode::Approval).await;
    f.core.save_rule(save(&f, json!({}))).await.unwrap();
    f.sim.counts.pause_next_login();
    let call = {
        let (core, client) = (f.core.clone(), f.client.clone());
        tokio::spawn(async move {
            core.run_tool(&client, "write", write(json!({"name":"x"})))
                .await
        })
    };
    f.sim.counts.wait_for_login().await;
    f.core.rename_environment(&f.env, "A2").await.unwrap();
    f.sim.counts.release_login();
    assert!(call.await.unwrap().is_err());
    assert_eq!(f.sim.counts.writes(), 0);
    // Renaming preserves rules; deleting the environment removes them.
    assert_eq!(f.core.rules(None).await.unwrap().len(), 1);
    f.core.delete_environment(&f.env, true).await.unwrap();
    assert!(f.core.rules(None).await.unwrap().is_empty());
}

#[tokio::test]
async fn changing_the_connection_target_drops_its_rules_but_a_new_key_does_not() {
    let f = fixture(WriteMode::Approval).await;
    f.core.save_rule(save(&f, json!({}))).await.unwrap();
    let draft = |database: &str, secret: &str| EnvironmentDraft {
        name: "A".into(),
        url: f.sim.url.clone(),
        database: database.into(),
        login: "fixture-user".into(),
        protocol: Protocol::LegacyJsonrpc,
        context: json!({}),
        secret: secret.into(),
    };
    f.core
        .update_environment(&f.env, draft("base-a", "fixture-secret"))
        .await
        .unwrap();
    assert_eq!(f.core.rules(None).await.unwrap().len(), 1);
    f.core
        .update_environment(&f.env, draft("base-b", ""))
        .await
        .unwrap();
    assert!(f.core.rules(None).await.unwrap().is_empty());
}

#[tokio::test]
async fn rules_persist_encrypted_and_are_validated_by_the_backend() {
    let f = fixture(WriteMode::Approval).await;
    let saved = f
        .core
        .save_rule(save(
            &f,
            json!({"method":"create","kind":"values","values":{"name":"x"},
                   "expires_at": (Utc::now() + Duration::hours(1)).to_rfc3339()}),
        ))
        .await
        .unwrap();
    assert!(!saved.expired);
    // Missing environment/rule and environment changes during editing.
    let mut other = save(&f, json!({}));
    other.environment_id = "missing".into();
    assert!(f.core.save_rule(other).await.is_err());
    let mut ghost = save(&f, json!({}));
    ghost.id = Some("missing".into());
    assert!(f.core.save_rule(ghost).await.is_err());
    assert!(f.core.delete_rule("missing").await.is_err());
    f.core.lock().await;
    assert!(f.core.rules(None).await.is_err());
    f.core.unlock("fictional master password").await.unwrap();
    let listed = f.core.rules(Some(&f.env)).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].rule, saved.rule);
    let created = f
        .core
        .run_tool(
            &flow::client(&f.core).await,
            "create",
            json!({"env":"A","model":"res.partner","values":{"name":"x"}}),
        )
        .await
        .unwrap();
    assert_eq!(created.data, json!([7]));
}

#[tokio::test]
async fn temporary_mode_writes_freely_but_unlink_stays_individual() {
    let f = fixture(WriteMode::Temporary).await;
    assert_eq!(
        f.core.environments().await.unwrap()[0].write_mode,
        WriteMode::Temporary
    );
    f.core
        .run_tool(&f.client, "write", write(json!({"name":"x"})))
        .await
        .unwrap();
    assert!(f.core.set_temporary_write(&f.env, 0).await.is_err());
    assert!(f.core.set_mode(&f.env, WriteMode::Temporary).await.is_err());
    let unlink = flow::queue(
        &f.core,
        &f.client,
        "unlink",
        json!({"env":"A","model":"res.partner","ids":[1]}),
    )
    .await;
    f.core.decide(&unlink.1.id, false).await.unwrap();
    assert_eq!(flow::error_code(unlink.0).await, "approval_rejected");
    assert_eq!(f.sim.counts.writes(), 1);
}
