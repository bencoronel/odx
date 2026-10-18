mod flow;
mod support;
use odx_core::error::OdxError;
use odx_core::{ClientIdentity, Core, EnvironmentDraft, EnvironmentView, Protocol, WriteMode};
use serde_json::{Value, json};
use std::sync::Arc;

struct Fixture {
    sim: support::Simulator,
    core: Arc<Core>,
    env: EnvironmentView,
    client: ClientIdentity,
    tmp: tempfile::TempDir,
}

async fn fixture(context: Value) -> Fixture {
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
            context,
            secret: "fixture-secret".into(),
        })
        .await
        .unwrap();
    let client = flow::client(&core).await;
    Fixture {
        sim,
        core,
        env,
        client,
        tmp,
    }
}

fn write(values: Value) -> Value {
    json!({"env":"A","model":"res.partner","ids":[1],"values":values})
}

#[tokio::test]
async fn rejected_writes_never_reach_odoo_and_approved_request_is_single_use() {
    let Fixture {
        sim,
        core,
        env,
        client,
        tmp: _tmp,
    } = fixture(json!({})).await;
    let req = || {
        let mut args = write(json!({"name":"fictional updated contact"}));
        args["context"] = json!({"lang":"es_ES"});
        args
    };
    let denied = core.run_tool(&client, "write", req()).await.unwrap_err();
    assert_eq!(denied.code, "forbidden");
    core.set_mode(&env.id, WriteMode::Approval).await.unwrap();
    let (call, rejected) = flow::queue(&core, &client, "write", req()).await;
    assert_eq!(sim.counts.writes(), 0);
    let listed = core.approvals().await.unwrap();
    assert_eq!(listed.len(), 1);
    let pending = &listed[0];
    assert_eq!(
        pending.args,
        json!([[1], {"name":"fictional updated contact"}])
    );
    assert_eq!(pending.context, json!({"lang":"es_ES"}));
    assert_eq!(
        core.decide(&rejected.id, false).await.unwrap(),
        json!({"status":"rejected"})
    );
    assert_eq!(flow::error_code(call).await, "approval_rejected");
    assert_eq!(sim.counts.writes(), 0);
    let (call, accepted) = flow::queue(&core, &client, "write", req()).await;
    assert_eq!(sim.counts.writes(), 0);
    assert_eq!(core.decide(&accepted.id, true).await.unwrap(), json!(true));
    assert_eq!(call.await.unwrap().unwrap().data, json!(true));
    assert_eq!(sim.counts.writes(), 1);
    assert!(core.decide(&accepted.id, true).await.is_err());
    assert_eq!(sim.counts.writes(), 1);
    core.lock().await;
    assert_eq!(core.status().await, "locked");
}

#[tokio::test]
async fn destructive_operations_are_individual_and_never_auto_approved() {
    let Fixture {
        sim,
        core,
        env,
        client,
        tmp: _tmp,
    } = fixture(json!({})).await;
    core.set_mode(&env.id, WriteMode::Approval).await.unwrap();

    let unlink = |ids: Value| json!({"env":"A","model":"res.partner","ids":ids});

    let multi_delete =
        json!({"env":"A","model":"res.partner","ids":[1,2],"values":{"tag_ids":[[2,12]]}});
    assert!(core.run_tool(&client, "write", multi_delete).await.is_err());
    let multiple_relational_deletes = write(json!({"tag_ids":[[2,12],[3,13]]}));
    assert!(
        core.run_tool(&client, "write", multiple_relational_deletes)
            .await
            .is_err()
    );
    assert!(core.approvals().await.unwrap().is_empty());

    // One approval covers every listed ID.
    let (call, individual) = flow::queue(&core, &client, "unlink", unlink(json!([1, 2]))).await;
    let pending = core.approvals().await.unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].args, json!([[1, 2]]));
    flow::abandon(&core, call, &individual.id).await;
    assert!(core.approvals().await.unwrap().is_empty());
    assert_eq!(sim.counts.writes(), 0);
}

#[tokio::test]
async fn lost_write_response_is_uncertain_and_is_never_retried() {
    let Fixture {
        sim,
        core,
        env,
        client,
        tmp: _tmp,
    } = fixture(json!({})).await;
    core.set_mode(&env.id, WriteMode::Approval).await.unwrap();
    let (call, approval) = flow::queue(
        &core,
        &client,
        "write",
        write(json!({"name":"fictional update"})),
    )
    .await;
    sim.counts.lose_next_response();
    assert!(matches!(
        core.decide(&approval.id, true).await,
        Err(OdxError::Uncertain)
    ));
    assert_eq!(flow::error_code(call).await, "uncertain_result");
    assert_eq!(sim.counts.writes(), 1);
    assert!(core.decide(&approval.id, true).await.is_err());
    assert_eq!(sim.counts.writes(), 1);
}

#[tokio::test]
async fn locking_after_sent_does_not_cancel_the_in_flight_write() {
    let Fixture {
        sim,
        core,
        env,
        client,
        tmp: _tmp,
    } = fixture(json!({})).await;
    core.set_mode(&env.id, WriteMode::Approval).await.unwrap();
    let (call, approval) = flow::queue(
        &core,
        &client,
        "write",
        write(json!({"name":"fictional update"})),
    )
    .await;
    sim.counts.pause_next_write();
    let pending_core = core.clone();
    let pending_id = approval.id.clone();
    let task = tokio::spawn(async move { pending_core.decide(&pending_id, true).await });
    sim.counts.wait_for_write().await;
    core.lock().await;
    sim.counts.release_write();
    assert_eq!(task.await.unwrap().unwrap(), json!(true));
    let _ = call.await.unwrap();
    assert_eq!(core.status().await, "locked");
    assert_eq!(sim.counts.writes(), 1);
}

/// Exit locks and drains sent writes within a deadline without cancellation.
#[tokio::test]
async fn exit_drain_waits_for_the_sent_write_within_its_deadline() {
    let Fixture {
        sim,
        core,
        env,
        client,
        tmp: _tmp,
    } = fixture(json!({})).await;
    assert!(core.drain(std::time::Duration::from_millis(10)).await);
    core.set_mode(&env.id, WriteMode::Approval).await.unwrap();
    let (call, approval) = flow::queue(&core, &client, "write", write(json!({"name":"x"}))).await;
    sim.counts.pause_next_write();
    let decision = {
        let (core, id) = (core.clone(), approval.id.clone());
        tokio::spawn(async move { core.decide(&id, true).await })
    };
    sim.counts.wait_for_write().await;
    core.lock().await;
    assert!(
        !core.drain(std::time::Duration::from_millis(50)).await,
        "deadline expires while the write is held"
    );
    let drain = {
        let core = core.clone();
        tokio::spawn(async move { core.drain(std::time::Duration::from_secs(10)).await })
    };
    tokio::task::yield_now().await;
    assert!(!drain.is_finished());
    sim.counts.release_write();
    assert!(
        drain.await.unwrap(),
        "finishes when the sent write completes"
    );
    assert_eq!(decision.await.unwrap().unwrap(), json!(true));
    let _ = call.await.unwrap();
    assert_eq!(sim.counts.writes(), 1);
}

#[tokio::test]
async fn cancellation_and_environment_revision_revoke_unsent_approvals() {
    let Fixture {
        sim,
        core,
        env,
        client,
        tmp: _tmp,
    } = fixture(json!({})).await;
    core.set_mode(&env.id, WriteMode::Approval).await.unwrap();
    let request = || write(json!({"name":"fixture"}));
    let (call, cancelled) = flow::queue(&core, &client, "write", request()).await;
    flow::abandon(&core, call, &cancelled.id).await;
    assert!(core.decide(&cancelled.id, true).await.is_err());
    let (call, changed) = flow::queue(&core, &client, "write", request()).await;
    core.rename_environment(&env.id, "A renamed").await.unwrap();
    assert!(core.decide(&changed.id, true).await.is_err());
    assert_eq!(flow::error_code(call).await, "approval_revoked");
    assert_eq!(sim.counts.writes(), 0);
}

#[tokio::test]
async fn audit_persistence_failure_blocks_approved_write_before_send() {
    let Fixture {
        sim,
        core,
        env,
        client,
        tmp,
    } = fixture(json!({})).await;
    core.set_mode(&env.id, WriteMode::Approval).await.unwrap();
    let (call, approval) = flow::queue(
        &core,
        &client,
        "write",
        write(json!({"name":"fixture update"})),
    )
    .await;

    let database = rusqlite::Connection::open(tmp.path().join("vault.sqlite3")).unwrap();
    database
        .pragma_update(None, "key", "fictional master password")
        .unwrap();
    database.execute_batch("CREATE TRIGGER fail_audit BEFORE INSERT ON audit_events BEGIN SELECT RAISE(ABORT, 'fixture audit failure'); END;").unwrap();
    assert!(matches!(
        core.decide(&approval.id, true).await,
        Err(OdxError::Persistence)
    ));
    assert!(call.await.unwrap().is_err());
    assert_eq!(sim.counts.writes(), 0);
    assert!(
        core.run_tool(
            &client,
            "search_read",
            json!({"env":"A","model":"res.partner"})
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn free_modes_execute_writes_but_always_queue_deletions_and_revoke_on_change() {
    let Fixture {
        sim,
        core,
        env,
        client,
        tmp: _tmp,
    } = fixture(json!({"lang":"es_ES"})).await;
    let run = |args: Value| {
        let (core, client) = (core.clone(), client.clone());
        async move { core.run_tool(&client, "write", args).await }
    };
    assert_eq!(
        run(write(json!({"name":"fixture"})))
            .await
            .unwrap_err()
            .code,
        "forbidden"
    );
    core.set_mode(&env.id, WriteMode::Permanent).await.unwrap();
    assert_eq!(
        run(write(json!({"name":"fixture"}))).await.unwrap().data,
        json!(true)
    );
    assert_eq!(sim.counts.writes(), 1);
    assert!(core.approvals().await.unwrap().is_empty());
    let (call, deletion) = flow::queue(
        &core,
        &client,
        "unlink",
        json!({"env":"A","model":"res.partner","ids":[1]}),
    )
    .await;
    assert_eq!(sim.counts.writes(), 1);
    core.set_temporary_write(&env.id, 1).await.unwrap();
    assert!(core.decide(&deletion.id, true).await.is_err());
    assert_eq!(flow::error_code(call).await, "approval_revoked");
    let (call, relational) =
        flow::queue(&core, &client, "write", write(json!({"tag_ids":[[2,12]]}))).await;
    assert_eq!(relational.context, json!({"lang":"es_ES"}));
    core.decide(&relational.id, true).await.unwrap();
    assert!(call.await.unwrap().is_ok());
    assert_eq!(sim.counts.writes(), 2);
    assert!(
        run(write(json!({"name":"temporary fixture"})))
            .await
            .is_ok()
    );
    assert_eq!(sim.counts.writes(), 3);
    let deadline = core.environments().await.unwrap()[0].write_until.unwrap();
    core.lock().await;
    assert_eq!(
        run(write(json!({"name":"locked"}))).await.unwrap_err().code,
        "locked"
    );
    core.unlock("fictional master password").await.unwrap();
    let restored = &core.environments().await.unwrap()[0];
    assert_eq!(restored.write_mode, WriteMode::Temporary);
    assert_eq!(restored.write_until, Some(deadline));
    assert!(core.set_temporary_write(&env.id, 0).await.is_err());
    assert!(core.set_mode(&env.id, WriteMode::Temporary).await.is_err());
    core.set_mode(&env.id, WriteMode::Permanent).await.unwrap();
    core.lock().await;
    core.unlock("fictional master password").await.unwrap();
    assert_eq!(
        core.environments().await.unwrap()[0].write_mode,
        WriteMode::Permanent
    );
    sim.counts.lose_next_response();
    assert_eq!(
        run(write(json!({"name":"uncertain"})))
            .await
            .unwrap_err()
            .code,
        "uncertain_result"
    );
    assert_eq!(sim.counts.writes(), 4);
    assert!(core.approvals().await.unwrap().is_empty());
}

#[tokio::test]
async fn odoo_errors_fail_known_and_unsent_attempts_are_closed() {
    use odx_core::ActivityFilter;
    let Fixture {
        sim,
        core,
        env,
        client,
        tmp: _tmp,
    } = fixture(json!({})).await;
    let request =
        |model: &str| json!({"env":"A","model":model,"ids":[1],"values":{"name":"fixture"}});
    let last_outcome = || async {
        core.activity(None, ActivityFilter::default())
            .await
            .unwrap()
            .events[0]
            .outcome
            .clone()
    };
    core.set_mode(&env.id, WriteMode::Approval).await.unwrap();
    // Connection changes during login close the recorded attempt without dispatch.
    let (call, pending) = flow::queue(&core, &client, "write", request("res.partner")).await;
    sim.counts.pause_next_login();
    let decision = {
        let core = core.clone();
        let id = pending.id.clone();
        tokio::spawn(async move { core.decide(&id, true).await })
    };
    sim.counts.wait_for_login().await;
    core.set_mode(&env.id, WriteMode::ReadOnly).await.unwrap();
    sim.counts.release_login();
    assert!(matches!(
        decision.await.unwrap(),
        Err(OdxError::ApprovalUnavailable)
    ));
    assert!(call.await.unwrap().is_err());
    assert_eq!(last_outcome().await, "not_sent");
    assert_eq!(sim.counts.writes(), 0);
    // An Odoo error response is a known failure.
    core.set_mode(&env.id, WriteMode::Approval).await.unwrap();
    let (call, pending) = flow::queue(&core, &client, "write", request("broken.model")).await;
    assert!(matches!(
        core.decide(&pending.id, true).await,
        Err(OdxError::Remote(m)) if m.contains("broken.model")
    ));
    assert_eq!(flow::error_code(call).await, "upstream_error");
    assert_eq!(last_outcome().await, "failed");
}

async fn outcome_of(core: &Arc<Core>, id: &str) -> String {
    core.activity(None, odx_core::ActivityFilter::default())
        .await
        .unwrap()
        .events
        .into_iter()
        .find(|e| e.request_id.as_deref() == Some(id))
        .map(|e| e.outcome)
        .unwrap()
}

#[tokio::test]
async fn a_request_arriving_during_an_approved_send_does_not_revoke_it() {
    let Fixture {
        sim,
        core,
        env,
        client,
        tmp: _tmp,
    } = fixture(json!({})).await;
    core.set_mode(&env.id, WriteMode::Approval).await.unwrap();
    let (call_a, a) = flow::queue(&core, &client, "write", write(json!({"name":"a"}))).await;
    sim.counts.pause_next_write();
    let decision = {
        let (core, id) = (core.clone(), a.id.clone());
        tokio::spawn(async move { core.decide(&id, true).await })
    };
    sim.counts.wait_for_write().await;
    // A new request in B purges and wakes requests while A is in flight.
    let other = flow::client(&core).await;
    let (call_b, b) = flow::queue(&core, &other, "write", write(json!({"name":"b"}))).await;
    sim.counts.release_write();
    assert_eq!(decision.await.unwrap().unwrap(), json!(true));
    assert_eq!(call_a.await.unwrap().unwrap().data, json!(true));
    assert_eq!(sim.counts.writes(), 1);
    assert_eq!(outcome_of(&core, &a.id).await, "succeeded");
    flow::abandon(&core, call_b, &b.id).await;
}

#[tokio::test]
async fn revocations_close_pending_requests_in_the_activity() {
    let Fixture {
        sim,
        core,
        env,
        client,
        tmp: _tmp,
    } = fixture(json!({})).await;
    core.set_mode(&env.id, WriteMode::Approval).await.unwrap();
    let request = || write(json!({"name":"fixture"}));
    let (call, by_mode) = flow::queue(&core, &client, "write", request()).await;
    core.set_mode(&env.id, WriteMode::Approval).await.unwrap();
    assert_eq!(flow::error_code(call).await, "approval_revoked");
    let other = flow::client(&core).await;
    let (call, by_client) = flow::queue(&core, &other, "write", request()).await;
    core.revoke_client(&other.client_id).await.unwrap();
    assert_eq!(flow::error_code(call).await, "approval_revoked");
    let (call, by_lock) = flow::queue(&core, &client, "write", request()).await;
    core.lock().await;
    assert_eq!(flow::error_code(call).await, "approval_revoked");
    core.unlock("fictional master password").await.unwrap();
    for id in [&by_mode.id, &by_client.id, &by_lock.id] {
        assert_eq!(outcome_of(&core, id).await, "revoked");
    }
    assert_eq!(sim.counts.writes(), 0);
}

#[tokio::test]
async fn a_failed_save_keeps_the_client_and_its_pending_requests() {
    let Fixture { core, env, tmp, .. } = fixture(json!({})).await;
    core.set_mode(&env.id, WriteMode::Approval).await.unwrap();
    let other = flow::client(&core).await;
    let (call, pending) =
        flow::queue(&core, &other, "write", write(json!({"name":"fixture"}))).await;
    let database = rusqlite::Connection::open(tmp.path().join("vault.sqlite3")).unwrap();
    database
        .pragma_update(None, "key", "fictional master password")
        .unwrap();
    database.execute_batch("CREATE TRIGGER fail_config BEFORE UPDATE ON config BEGIN SELECT RAISE(ABORT, 'fixture save failure'); END;").unwrap();
    assert!(core.revoke_client(&other.client_id).await.is_err());
    let id = other.client_id.clone();
    let listed = |core: Arc<Core>| {
        let id = id.clone();
        async move {
            core.clients()
                .await
                .unwrap()
                .into_iter()
                .any(|c| c.id == id)
        }
    };
    assert!(listed(core.clone()).await);
    assert_eq!(core.approvals().await.unwrap().len(), 1);
    database.execute_batch("DROP TRIGGER fail_config;").unwrap();
    flow::abandon(&core, call, &pending.id).await;
    core.lock().await;
    core.unlock("fictional master password").await.unwrap();
    assert!(listed(core.clone()).await);
}

#[tokio::test]
async fn a_call_cancelled_while_its_request_is_being_recorded_leaves_nothing_pending() {
    let Fixture {
        core, env, client, ..
    } = fixture(json!({})).await;
    core.set_mode(&env.id, WriteMode::Approval).await.unwrap();
    for yields in 0..16 {
        let call = {
            let (core, client) = (core.clone(), client.clone());
            tokio::spawn(async move {
                core.run_tool(&client, "write", write(json!({"name":"fixture"})))
                    .await
            })
        };
        for _ in 0..yields {
            tokio::task::yield_now().await;
        }
        call.abort();
        let _ = call.await;
    }
    for _ in 0..200 {
        let pending = core.approvals().await.unwrap().len();
        let logged = core
            .activity(None, odx_core::ActivityFilter::default())
            .await
            .unwrap()
            .events
            .iter()
            .filter(|e| e.outcome == "pending")
            .count();
        if pending == 0 && logged == 0 {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("pending requests remain after call cancellation");
}

#[tokio::test]
async fn changing_only_the_protocol_revokes_unsent_approvals() {
    let Fixture {
        sim,
        core,
        env,
        client,
        tmp: _tmp,
    } = fixture(json!({})).await;
    core.set_mode(&env.id, WriteMode::Approval).await.unwrap();
    let (call, pending) =
        flow::queue(&core, &client, "write", write(json!({"name":"fixture"}))).await;
    core.update_environment(
        &env.id,
        EnvironmentDraft {
            name: "A".into(),
            url: sim.url.clone(),
            database: "base-a".into(),
            login: "fixture-user".into(),
            protocol: Protocol::Xmlrpc,
            context: json!({}),
            secret: String::new(),
        },
    )
    .await
    .unwrap();
    assert!(core.decide(&pending.id, true).await.is_err());
    assert_eq!(flow::error_code(call).await, "approval_revoked");
    assert_eq!(outcome_of(&core, &pending.id).await, "revoked");
    assert_eq!(sim.counts.writes(), 0);
}

#[tokio::test]
async fn audit_failure_after_send_keeps_the_real_result_and_blocks_new_operations() {
    let Fixture {
        sim,
        core,
        env,
        client,
        tmp,
    } = fixture(json!({})).await;
    core.set_mode(&env.id, WriteMode::Permanent).await.unwrap();
    // Fail only the closing record of a successful call; "attempted" is still written before the send.
    let database = rusqlite::Connection::open(tmp.path().join("vault.sqlite3")).unwrap();
    database
        .pragma_update(None, "key", "fictional master password")
        .unwrap();
    database.execute_batch("CREATE TRIGGER fail_close BEFORE UPDATE ON activity WHEN NEW.outcome = 'succeeded' BEGIN SELECT RAISE(ABORT, 'fixture audit failure'); END;").unwrap();
    let out = core
        .run_tool(&client, "write", write(json!({"name":"sent"})))
        .await
        .expect("Odoo applied the write; its result must not read as not sent");
    assert_eq!(out.data, json!(true));
    assert_eq!(sim.counts.writes(), 1);
    let blocked = core
        .run_tool(&client, "write", write(json!({"name":"next"})))
        .await
        .unwrap_err();
    assert_eq!(blocked.code, "audit_unavailable");
    assert_eq!(sim.counts.writes(), 1);
}
