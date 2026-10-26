//! Client tool integration: validation, grants, policy and approvals.
mod support;
use odx_core::{Capability, ClientIdentity, Core, EnvironmentDraft, Protocol, WriteMode};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};

struct Fixture {
    core: Arc<Core>,
    sim: support::Simulator,
    a: String,
    _b: String,
    _tmp: tempfile::TempDir,
}

async fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let sim = support::simulator().await;
    let core = support::unlocked_core(&tmp).await;
    let mut ids = vec![];
    for name in ["A", "B"] {
        let env = core
            .save_environment(EnvironmentDraft {
                name: name.into(),
                url: sim.url.clone(),
                database: "base-a".into(),
                login: "fixture-user".into(),
                protocol: Protocol::LegacyJsonrpc,
                context: json!({"lang": "es_ES"}),
                secret: "fixture-secret".into(),
            })
            .await
            .unwrap();
        ids.push(env.id);
    }
    Fixture {
        core,
        sim,
        _b: ids.pop().unwrap(),
        a: ids.pop().unwrap(),
        _tmp: tmp,
    }
}

async fn client(f: &Fixture, envs: &[&str], caps: &[Capability]) -> (ClientIdentity, String) {
    let issued = f
        .core
        .create_client(
            &format!("c{}", uuid_like()),
            envs.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            caps.to_vec(),
        )
        .await
        .unwrap();
    let token = issued.token.clone();
    (f.core.authenticate_client(&token).await.unwrap(), token)
}
fn uuid_like() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    N.fetch_add(1, Ordering::SeqCst)
}

#[tokio::test]
async fn reads_respect_grant_validate_before_sending_and_page() {
    let f = fixture().await;
    let (reader, _) = client(&f, &[&f.a], &[Capability::OdooRead]).await;

    f.core
        .set_environment_tags(&f.a, &["Cliente A".into(), "staging".into()])
        .await
        .unwrap();
    let envs = f.core.run_tool(&reader, "envs", json!({})).await.unwrap();
    assert_eq!(
        envs.data,
        json!([{"name":"A","protocol":"legacy_jsonrpc","write_mode":"read_only","context":{"lang":"es_ES"},"tags":["Cliente A","staging"]}])
    );
    assert!(
        !envs.data.to_string().contains("fixture-secret")
            && !envs.data.to_string().contains(&f.sim.url)
    );

    // Unavailable and ungranted environments share an error and make zero calls.
    for env in ["B", "Z"] {
        let err = f
            .core
            .run_tool(
                &reader,
                "search_count",
                json!({"env": env, "model": "res.partner"}),
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, "environment_unavailable");
    }
    for bad in [
        json!({"env":"A","model":"res.partner","extra":1}),
        json!({"env":"A","model":"res.partner","limit":null}),
        json!({"env":"A","model":"res.partner","domain":"[]"}),
        json!({"env":"A","model":"res partner"}),
        json!({"env":"A","model":"res.partner","limit":1001}),
    ] {
        let err = f.core.run_tool(&reader, "search", bad).await.unwrap_err();
        assert_eq!(err.code, "invalid_arguments");
    }
    assert_eq!(f.sim.counts.queries(), 0);
    assert_eq!(f.sim.counts.logins(), 0);

    let page = f
        .core
        .run_tool(
            &reader,
            "search",
            json!({"env":"A","model":"res.partner","limit":2,"offset":1}),
        )
        .await
        .unwrap();
    assert_eq!(page.data, json!([2, 3]));
    assert_eq!(
        page.meta["page"],
        json!({"limit":2,"offset":1,"returned":2,"has_more":true,"next_offset":3})
    );
    // The first call after unlock counts login calls and bytes.
    assert_eq!(page.meta["upstream_calls"], 2);
    let first_bytes = page.meta["upstream_bytes"].as_u64().unwrap();

    let read = f
        .core
        .run_tool(
            &reader,
            "read",
            json!({"env":"A","model":"res.partner","ids":[1,9]}),
        )
        .await
        .unwrap();
    assert_eq!(read.meta["missing_ids"], json!([9]));
    assert_eq!(read.meta["upstream_calls"], 1);
    let again = f
        .core
        .run_tool(
            &reader,
            "search",
            json!({"env":"A","model":"res.partner","limit":2,"offset":1}),
        )
        .await
        .unwrap();
    assert!(again.meta["upstream_bytes"].as_u64().unwrap() < first_bytes);

    let fields = f
        .core
        .run_tool(
            &reader,
            "fields",
            json!({"env":"A","model":"res.partner","q":"comp"}),
        )
        .await
        .unwrap();
    assert_eq!(fields.data[0]["name"], "company_id");
    assert_eq!(fields.data[0]["relation"], "res.company");

    let names = f
        .core
        .run_tool(
            &reader,
            "name_search",
            json!({"env":"A","model":"res.partner","name":"al","limit":2}),
        )
        .await
        .unwrap();
    assert_eq!(
        names.data,
        json!([{"id":1,"name":"Alpha"},{"id":2,"name":"Alphabet"}])
    );
    assert_eq!(names.meta["warnings"][0]["code"], "refine_query");
    // Normalize known legacy signatures to positional arguments for Odoo 16–19.
    for name in ["args", "domain"] {
        let out = f
            .core
            .run_tool(
                &reader,
                "call",
                json!({"env":"A","model":"res.partner","method":"name_search","args":["al"],"kwargs":{name:[]}}),
            )
            .await
            .unwrap();
        assert_eq!(
            out.data,
            json!([[1, "Alpha"], [2, "Alphabet"], [3, "Alps"]])
        );
    }

    let me = f
        .core
        .run_tool(&reader, "whoami", json!({"env":"A"}))
        .await
        .unwrap();
    assert_eq!(me.data["login"], "fixture-user");
    assert_eq!(me.data["uid"], 11);

    // Merge global < environment < call context, including call kwargs.context.
    let rows = f
        .core
        .run_tool(
            &reader,
            "search_read",
            json!({"env":"A","model":"res.partner","context":{"tz":"UTC"}}),
        )
        .await
        .unwrap();
    assert_eq!(rows.data[0]["context"], json!({"lang":"es_ES","tz":"UTC"}));
    let conflict = f.core.run_tool(&reader, "call", json!({"env":"A","model":"res.partner","method":"search_count","context":{},"kwargs":{"context":{}}})).await.unwrap_err();
    assert_eq!(conflict.code, "invalid_arguments");
    let count = f
        .core
        .run_tool(
            &reader,
            "call",
            json!({"env":"A","model":"res.partner","method":"search_count","args":[[]]}),
        )
        .await
        .unwrap();
    assert_eq!(count.data, 5);
    let private = f
        .core
        .run_tool(
            &reader,
            "call",
            json!({"env":"A","model":"res.partner","method":"_private"}),
        )
        .await
        .unwrap_err();
    assert_eq!(private.code, "invalid_arguments");

    // Identify logical Odoo errors without leaking secrets.
    let remote = f
        .core
        .run_tool(
            &reader,
            "search_count",
            json!({"env":"A","model":"broken.model"}),
        )
        .await
        .unwrap_err();
    assert_eq!(remote.code, "upstream_error");
    assert_eq!(remote.outcome, odx_core::Outcome::Failed);
    assert!(remote.message.contains("broken.model") && !remote.message.contains("fixture-secret"));
}

#[tokio::test]
async fn writes_follow_mode_and_wait_for_human_decisions() {
    let f = fixture().await;
    let (reader, _) = client(&f, &[&f.a], &[Capability::OdooRead]).await;
    let (writer, _) = client(
        &f,
        &[&f.a],
        &[Capability::OdooRead, Capability::OdooWriteRequest],
    )
    .await;
    let write = json!({"env":"A","model":"res.partner","ids":[1],"values":{"name":"Ada"}});

    // Missing capabilities/read-only mode prevent upstream access.
    assert_eq!(
        f.core
            .run_tool(&reader, "write", write.clone())
            .await
            .unwrap_err()
            .code,
        "forbidden"
    );
    assert_eq!(
        f.core
            .run_tool(&writer, "write", write.clone())
            .await
            .unwrap_err()
            .code,
        "forbidden"
    );
    assert_eq!(f.sim.counts.writes(), 0);

    // Free-write executes without approval; create normalizes to ID lists.
    f.core.set_mode(&f.a, WriteMode::Permanent).await.unwrap();
    assert_eq!(
        f.core
            .run_tool(&writer, "write", write.clone())
            .await
            .unwrap()
            .data,
        json!(true)
    );
    let created = f
        .core
        .run_tool(
            &writer,
            "create",
            json!({"env":"A","model":"res.partner","values":{"name":"x"}}),
        )
        .await
        .unwrap();
    assert_eq!(created.data, json!([7]));
    assert_eq!(f.sim.counts.writes(), 2);

    // unlink always waits for a human, even in free-write mode, and reports this.
    let mut notices = f.core.approval_notices();
    let core = f.core.clone();
    let w = writer.clone();
    let pending = tokio::spawn(async move {
        core.run_tool(
            &w,
            "unlink",
            json!({"env":"A","model":"res.partner","ids":[1]}),
        )
        .await
    });
    let approval = wait_pending(&f.core).await;
    assert_eq!(approval["method"], "unlink");
    let notice = notices.try_recv().unwrap();
    assert_eq!(
        (notice.method.as_str(), notice.environment_name.as_str()),
        ("unlink", "A")
    );
    assert_eq!(notice.approval_id, approval["id"].as_str().unwrap());
    assert!(approval["client_label"].is_string());
    assert_eq!(f.sim.counts.writes(), 2);
    f.core
        .decide(approval["id"].as_str().unwrap(), true)
        .await
        .unwrap();
    assert_eq!(pending.await.unwrap().unwrap().data, json!(true));
    assert_eq!(f.sim.counts.writes(), 3);

    // call also waits in approval mode; rejection sends nothing.
    f.core.set_mode(&f.a, WriteMode::Approval).await.unwrap();
    let core = f.core.clone();
    let w = writer.clone();
    let pending = tokio::spawn(async move {
        core.run_tool(
            &w,
            "call",
            json!({"env":"A","model":"sale.order","method":"action_confirm","args":[[1]]}),
        )
        .await
    });
    let approval = wait_pending(&f.core).await;
    f.core
        .decide(approval["id"].as_str().unwrap(), false)
        .await
        .unwrap();
    let err = pending.await.unwrap().unwrap_err();
    assert_eq!(err.code, "approval_rejected");
    assert_eq!(f.sim.counts.writes(), 3);

    // Abandoning approval waits removes requests without dispatch.
    f.core.set_mode(&f.a, WriteMode::Approval).await.unwrap();
    let core = f.core.clone();
    let w = writer.clone();
    let write2 = write.clone();
    let pending = tokio::spawn(async move { core.run_tool(&w, "write", write2).await });
    wait_pending(&f.core).await;
    pending.abort();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(f.core.approvals().await.unwrap().is_empty());
    assert_eq!(f.sim.counts.writes(), 3);

    // Revocation during a wait returns approval_revoked and invalidates the token.
    let core = f.core.clone();
    let w = writer.clone();
    let write3 = write.clone();
    let pending = tokio::spawn(async move { core.run_tool(&w, "write", write3).await });
    wait_pending(&f.core).await;
    f.core.revoke_client(&writer.client_id).await.unwrap();
    assert_eq!(pending.await.unwrap().unwrap_err().code, "approval_revoked");
    assert_eq!(
        f.core
            .run_tool(&writer, "envs", json!({}))
            .await
            .unwrap_err()
            .code,
        "unauthenticated"
    );

    // Locking revokes waits and disables all tools.
    let (writer2, _) = client(&f, &[&f.a], &[Capability::OdooWriteRequest]).await;
    let core = f.core.clone();
    let w = writer2.clone();
    let pending = tokio::spawn(async move { core.run_tool(&w, "write", write).await });
    wait_pending(&f.core).await;
    f.core.lock().await;
    assert_eq!(pending.await.unwrap().unwrap_err().code, "approval_revoked");
    assert_eq!(
        f.core
            .run_tool(&writer2, "envs", json!({}))
            .await
            .unwrap_err()
            .code,
        "locked"
    );
    assert_eq!(f.sim.counts.writes(), 3);
}

#[tokio::test]
async fn call_in_free_modes_only_queues_known_deletions_and_unlinks() {
    let f = fixture().await;
    let (writer, _) = client(&f, &[&f.a], &[Capability::OdooWriteRequest]).await;
    for mode in [WriteMode::Permanent, WriteMode::Temporary] {
        if mode == WriteMode::Temporary {
            f.core.set_temporary_write(&f.a, 5).await.unwrap();
        } else {
            f.core.set_mode(&f.a, mode).await.unwrap();
        }
        for arguments in [
            json!({"env":"A","model":"sale.order","method":"action_confirm","args":[[3,5]]}),
            json!({"env":"A","model":"res.partner","method":"write","args":[[3,5],{"name":"x"}]}),
            json!({"env":"A","model":"res.partner","method":"create","args":[{"name":"x"}]}),
        ] {
            tokio::time::timeout(
                Duration::from_secs(2),
                f.core.run_tool(&writer, "call", arguments),
            )
            .await
            .unwrap()
            .unwrap();
            assert!(f.core.approvals().await.unwrap().is_empty());
        }
        let sent = f.sim.counts.writes();
        for arguments in [
            json!({"env":"A","model":"res.partner","method":"unlink","args":[[1]]}),
            json!({"env":"A","model":"res.partner","method":"unlink","kwargs":{"ids":[1]}}),
            json!({"env":"A","model":"res.partner","method":"write","args":[[1],{"tags":[[2,8,0]]}]}),
            json!({"env":"A","model":"res.partner","method":"write","args":[[1]],"kwargs":{"vals":{"tags":[[3,8,0]]}}}),
            json!({"env":"A","model":"res.partner","method":"write","kwargs":{"ids":[1],"vals":{"tags":[[5,0,0]]}}}),
            json!({"env":"A","model":"res.partner","method":"write","args":[[1],{"tags":[[6,0,[3,5]]]}]}),
        ] {
            let core = f.core.clone();
            let writer = writer.clone();
            let pending =
                tokio::spawn(async move { core.run_tool(&writer, "call", arguments).await });
            let approval = wait_pending(&f.core).await;
            assert_eq!(f.sim.counts.writes(), sent);
            f.core
                .decide(approval["id"].as_str().unwrap(), false)
                .await
                .unwrap();
            assert_eq!(
                pending.await.unwrap().unwrap_err().code,
                "approval_rejected"
            );
            assert_eq!(f.sim.counts.writes(), sent);
        }
    }
}

#[tokio::test]
async fn concurrent_reads_share_one_login_and_a_failed_login_is_retried() {
    let f = fixture().await;
    let (reader, _) = client(&f, &[&f.a], &[Capability::OdooRead]).await;
    // 30 cold-cache reads: 4 active and 16 queued per environment; excess calls return capacity_exceeded before Odoo.
    f.sim.counts.pause_next_login();
    let calls: Vec<_> = (0..30)
        .map(|_| {
            let (core, reader) = (f.core.clone(), reader.clone());
            tokio::spawn(async move {
                core.run_tool(
                    &reader,
                    "search_count",
                    json!({"env":"A","model":"res.partner"}),
                )
                .await
            })
        })
        .collect();
    f.sim.counts.wait_for_login().await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    f.sim.counts.release_login();
    let (mut with_login, mut served) = (0, 0);
    for call in calls {
        match call.await.unwrap() {
            Ok(out) => {
                served += 1;
                with_login += usize::from(out.meta["upstream_calls"] == 2);
            }
            Err(e) => assert_eq!(e.code, "capacity_exceeded"),
        }
    }
    assert!(served >= 20, "{served}");
    assert_eq!(f.sim.counts.logins(), 1);
    assert_eq!(with_login, 1, "only the call that logged in counts it");

    // Failed logins remain uncached so later calls can retry.
    f.core.lock().await;
    f.core.unlock("fictional master password").await.unwrap();
    f.sim.counts.fail_next_login();
    let err = f
        .core
        .run_tool(&reader, "whoami", json!({"env":"A"}))
        .await
        .unwrap_err();
    assert_eq!(err.code, "upstream_auth");
    let me = f
        .core
        .run_tool(&reader, "whoami", json!({"env":"A"}))
        .await
        .unwrap();
    assert_eq!(me.data["uid"], 11);
    assert_eq!(me.meta["upstream_calls"], 2);
    assert_eq!(f.sim.counts.logins(), 3);
    // whoami uses normal read auditing, including login failures.
    let activity = f
        .core
        .activity(None, odx_core::ActivityFilter::default())
        .await
        .unwrap();
    let whoami: Vec<_> = activity
        .events
        .iter()
        .filter(|e| e.operation == "read:res.users")
        .map(|e| e.outcome.as_str())
        .collect();
    assert_eq!(whoami, ["succeeded", "failed"]);
}

#[tokio::test]
async fn relational_deletions_get_the_same_code_through_tools_and_call() {
    let f = fixture().await;
    let (writer, _) = client(&f, &[&f.a], &[Capability::OdooWriteRequest]).await;
    f.core.set_mode(&f.a, WriteMode::Permanent).await.unwrap();
    let code = |name: &'static str, arguments: Value| {
        let (core, writer) = (f.core.clone(), writer.clone());
        async move {
            core.run_tool(&writer, name, arguments)
                .await
                .unwrap_err()
                .code
        }
    };
    let pairs = [
        (
            code("create", json!({"env":"A","model":"res.partner","values":{"tags":[[2,8]]}})).await,
            code("call", json!({"env":"A","model":"res.partner","method":"create","args":[{"tags":[[2,8]]}]})).await,
        ),
        (
            code("write", json!({"env":"A","model":"res.partner","ids":[1,2],"values":{"tags":[[3,8]]}})).await,
            code("call", json!({"env":"A","model":"res.partner","method":"write","args":[[1,2],{"tags":[[3,8]]}]})).await,
        ),
    ];
    for (tool, call) in pairs {
        assert_eq!(tool, call);
        assert_eq!(tool, "invalid_arguments");
    }
    assert_eq!(f.sim.counts.writes(), 0);
    assert!(f.core.approvals().await.unwrap().is_empty());
}

async fn wait_pending(core: &Arc<Core>) -> Value {
    for _ in 0..100 {
        if let Some(p) = core.approvals().await.unwrap().into_iter().next() {
            return serde_json::to_value(p).unwrap();
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("pending approval did not appear");
}

/// Error code from an expected tool failure.
async fn code_of(
    f: &Fixture,
    client: &ClientIdentity,
    tool: &str,
    arguments: Value,
) -> &'static str {
    f.core
        .run_tool(client, tool, arguments)
        .await
        .unwrap_err()
        .code
}

#[tokio::test]
async fn models_filters_by_q_and_pages_stably() {
    let f = fixture().await;
    let (reader, _) = client(&f, &[&f.a], &[Capability::OdooRead]).await;
    let models = |q: &str, extra: Value| {
        let mut arguments = json!({"env":"A","q":q});
        arguments
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let (core, reader) = (f.core.clone(), reader.clone());
        async move { core.run_tool(&reader, "models", arguments).await.unwrap() }
    };

    // Match model or label; return model/name rows without internal IDs.
    let res = models("res", json!({})).await;
    assert_eq!(
        res.data,
        json!([{"model":"res.company","name":"Companies"},{"model":"res.partner","name":"Contact"},{"model":"res.users","name":"Users"}])
    );
    assert_eq!(
        res.meta["page"],
        json!({"limit":50,"offset":0,"returned":3,"has_more":false})
    );
    assert_eq!(
        models("ORDER", json!({})).await.data,
        json!([{"model":"sale.order","name":"Sales Order"}])
    );

    // Stable limit+1 pagination with explicit continuation.
    let first = models("", json!({"limit":2})).await;
    assert_eq!(first.data[1]["model"], "res.partner");
    assert_eq!(
        first.meta["page"],
        json!({"limit":2,"offset":0,"returned":2,"has_more":true,"next_offset":2})
    );
    let second = models("", json!({"limit":2,"offset":2})).await;
    assert_eq!(
        second.data,
        json!([{"model":"res.users","name":"Users"},{"model":"sale.order","name":"Sales Order"}])
    );
    assert_eq!(second.meta["page"]["has_more"], false);
    let none = models("zzz", json!({})).await;
    assert_eq!(none.data, json!([]));
    assert_eq!(none.meta["page"]["returned"], 0);

    // Require textual q; models rejects model arguments.
    let queries = f.sim.counts.queries();
    for bad in [
        json!({"env":"A"}),
        json!({"env":"A","q":1}),
        json!({"env":"A","q":"res","model":"res.partner"}),
        json!({"env":"A","q":"res","limit":0}),
        json!({"env":"A","q":"res","offset":-1}),
    ] {
        assert_eq!(
            code_of(&f, &reader, "models", bad).await,
            "invalid_arguments"
        );
    }
    assert_eq!(
        code_of(&f, &reader, "models", json!({"env":"B","q":"res"})).await,
        "environment_unavailable"
    );
    assert_eq!(f.sim.counts.queries(), queries, "nothing reaches Odoo");

    // ir.model read denial returns an explicit error instead of an empty list.
    f.sim
        .counts
        .fail_next_execute("You are not allowed to access 'Models' (ir.model) records.");
    let err = f
        .core
        .run_tool(&reader, "models", json!({"env":"A","q":"res"}))
        .await
        .unwrap_err();
    assert_eq!(
        (err.code, err.outcome),
        ("upstream_error", odx_core::Outcome::Failed)
    );
    assert!(err.message.contains("ir.model"));
}

#[tokio::test]
async fn read_metadata_and_whoami_report_their_own_errors() {
    let f = fixture().await;
    let (reader, _) = client(&f, &[&f.a], &[Capability::OdooRead]).await;
    let queries = f.sim.counts.queries();

    // read requires 1–1000 unique positive IDs and explicit nonempty unique fields.
    let too_many: Vec<u32> = (1..=1001).collect();
    for bad in [
        json!({"env":"A","model":"res.partner"}),
        json!({"env":"A","model":"res.partner","ids":[]}),
        json!({"env":"A","model":"res.partner","ids":[0]}),
        json!({"env":"A","model":"res.partner","ids":[-1]}),
        json!({"env":"A","model":"res.partner","ids":[1,1]}),
        json!({"env":"A","model":"res.partner","ids":[1.5]}),
        json!({"env":"A","model":"res.partner","ids":["1"]}),
        json!({"env":"A","model":"res.partner","ids":[9007199254740992u64]}),
        json!({"env":"A","model":"res.partner","ids":too_many}),
        json!({"env":"A","model":"res.partner","ids":[1],"fields":[]}),
        json!({"env":"A","model":"res.partner","ids":[1],"fields":["name","name"]}),
        json!({"env":"A","model":"res.partner","ids":[1],"limit":1}),
    ] {
        assert_eq!(code_of(&f, &reader, "read", bad).await, "invalid_arguments");
    }
    // fields requires textual q and page sizes 1–1000.
    for bad in [
        json!({"env":"A"}),
        json!({"env":"A","model":"res.partner","q":1}),
        json!({"env":"A","model":"res.partner","limit":0}),
        json!({"env":"A","model":"res.partner","fields":["name"]}),
    ] {
        assert_eq!(
            code_of(&f, &reader, "fields", bad).await,
            "invalid_arguments"
        );
    }
    // name_search requires textual name and offers no offset.
    for bad in [
        json!({"env":"A","model":"res.partner"}),
        json!({"env":"A","model":"res.partner","name":1}),
        json!({"env":"A","model":"res.partner","name":"al","offset":1}),
        json!({"env":"A","model":"res.partner","name":"al","domain":[["name"]]}),
    ] {
        assert_eq!(
            code_of(&f, &reader, "name_search", bad).await,
            "invalid_arguments"
        );
    }
    // whoami rejects model arguments and non-object context.
    for bad in [
        json!({}),
        json!({"env":"A","model":"res.users"}),
        json!({"env":"A","context":"lang=es"}),
    ] {
        assert_eq!(
            code_of(&f, &reader, "whoami", bad).await,
            "invalid_arguments"
        );
    }
    for tool in ["read", "fields", "name_search", "whoami"] {
        let arguments = json!({"env":"B","model":"res.partner","ids":[1],"name":"al"});
        let mut arguments = arguments.as_object().unwrap().clone();
        match tool {
            "whoami" => arguments.retain(|k, _| k == "env"),
            "fields" => arguments.retain(|k, _| k == "env" || k == "model"),
            "read" => {
                arguments.remove("name");
            }
            _ => {
                arguments.remove("ids");
            }
        }
        assert_eq!(
            code_of(&f, &reader, tool, Value::Object(arguments)).await,
            "environment_unavailable",
            "{tool}"
        );
    }
    assert_eq!(
        f.sim.counts.queries(),
        queries,
        "validation errors do not reach Odoo"
    );

    // Flag every missing read ID; empty results cannot imply complete success.
    let missing = f
        .core
        .run_tool(
            &reader,
            "read",
            json!({"env":"A","model":"res.partner","ids":[8,9]}),
        )
        .await
        .unwrap();
    assert_eq!(missing.data, json!([]));
    assert_eq!(missing.meta["missing_ids"], json!([8, 9]));

    // Empty fields q is equivalent to omission; page stably by name.
    let all = f
        .core
        .run_tool(
            &reader,
            "fields",
            json!({"env":"A","model":"res.partner","q":"","limit":2}),
        )
        .await
        .unwrap();
    let names: Vec<_> = all
        .data
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["name"].clone())
        .collect();
    assert_eq!(names, [json!("company_id"), json!("email")]);
    assert_eq!(
        all.meta["page"],
        json!({"limit":2,"offset":0,"returned":2,"has_more":true,"next_offset":2})
    );
    let rest = f
        .core
        .run_tool(
            &reader,
            "fields",
            json!({"env":"A","model":"res.partner","offset":2}),
        )
        .await
        .unwrap();
    assert_eq!(rest.data[0]["name"], "name");
    assert_eq!(rest.data[0]["stored"], true);
    assert_eq!(rest.meta["page"]["has_more"], false);

    // Identify missing models/access denial without leaking secrets.
    let broken = f
        .core
        .run_tool(&reader, "fields", json!({"env":"A","model":"broken.model"}))
        .await
        .unwrap_err();
    assert_eq!(
        (broken.code, broken.outcome),
        ("upstream_error", odx_core::Outcome::Failed)
    );
    assert!(broken.message.contains("broken.model"));
    for (tool, arguments) in [
        ("read", json!({"env":"A","model":"res.partner","ids":[1]})),
        (
            "name_search",
            json!({"env":"A","model":"res.partner","name":"al"}),
        ),
        ("whoami", json!({"env":"A"})),
    ] {
        f.sim
            .counts
            .fail_next_execute("Access Denied by ACLs for operation: read");
        let err = f.core.run_tool(&reader, tool, arguments).await.unwrap_err();
        assert_eq!(
            (err.code, err.outcome),
            ("upstream_error", odx_core::Outcome::Failed),
            "{tool}"
        );
        assert!(err.message.contains("Access Denied") && !err.message.contains("fixture-secret"));
    }
}

#[tokio::test]
async fn create_validates_values_before_policy_and_sending() {
    let f = fixture().await;
    let (reader, _) = client(&f, &[&f.a], &[Capability::OdooRead]).await;
    let (writer, _) = client(&f, &[&f.a], &[Capability::OdooWriteRequest]).await;
    let one = json!({"env":"A","model":"res.partner","values":{"name":"x"}});

    // Read-only/missing capabilities return forbidden without dispatch.
    assert_eq!(
        code_of(&f, &writer, "create", one.clone()).await,
        "forbidden"
    );
    assert_eq!(
        code_of(&f, &reader, "create", one.clone()).await,
        "forbidden"
    );

    f.core.set_mode(&f.a, WriteMode::Permanent).await.unwrap();
    let batch: Vec<Value> = (0..1001).map(|i| json!({"name": i})).collect();
    for bad in [
        json!({"env":"A","model":"res.partner"}),
        json!({"env":"A","model":"res.partner","values":{}}),
        json!({"env":"A","model":"res.partner","values":[]}),
        json!({"env":"A","model":"res.partner","values":[{}]}),
        json!({"env":"A","model":"res.partner","values":[{"name":"x"},"y"]}),
        json!({"env":"A","model":"res.partner","values":"name=x"}),
        json!({"env":"A","model":"res.partner","values":batch}),
        json!({"env":"A","model":"res.partner","values":{"name":"x"},"ids":[1]}),
        json!({"env":"A","values":{"name":"x"}}),
    ] {
        assert_eq!(
            code_of(&f, &writer, "create", bad).await,
            "invalid_arguments"
        );
    }
    assert_eq!(
        code_of(
            &f,
            &writer,
            "create",
            json!({"env":"B","model":"res.partner","values":{"name":"x"}})
        )
        .await,
        "environment_unavailable"
    );
    assert_eq!(f.sim.counts.writes(), 0);
    assert!(f.core.approvals().await.unwrap().is_empty());

    // Homogeneous batch returns IDs in upstream order.
    let created = f
        .core
        .run_tool(
            &writer,
            "create",
            json!({"env":"A","model":"res.partner","values":[{"name":"a"},{"name":"b"}]}),
        )
        .await
        .unwrap();
    assert_eq!(created.data, json!([7, 8]));
    assert_eq!(f.sim.counts.writes(), 1);
}

#[tokio::test]
async fn compact_output_truncates_long_text_by_default_and_json_keeps_types() {
    use odx_core::output::{MAX_OUTPUT, fit};
    let f = fixture().await;
    let (reader, _) = client(&f, &[&f.a], &[Capability::OdooRead]).await;
    let read = |extra: Value| {
        let mut arguments = json!({"env":"A","model":"x.types"});
        arguments
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let (core, reader) = (f.core.clone(), reader.clone());
        async move { core.run_tool(&reader, "search_read", arguments).await }
    };
    let long = "ñ€😀".repeat(100);

    // Default trunc=200 preserves 200 whole characters and a marker; numbers stay intact.
    let out = read(json!({})).await.unwrap();
    assert_eq!(out.trunc, 200);
    let (_, rendered) = fit(out).unwrap();
    let row: Vec<&str> = rendered.text.lines().nth(1).unwrap().split('\t').collect();
    let header: Vec<&str> = rendered.text.lines().next().unwrap().split('\t').collect();
    let cell = |name: &str| row[header.iter().position(|h| *h == name).unwrap()];
    let expected: String = long.chars().take(200).chain(['…']).collect();
    assert_eq!(cell("name"), expected);
    assert_eq!(cell("flag"), "false");
    assert_eq!(cell("zero"), "0");
    assert_eq!(cell("empty"), "");
    assert_eq!(cell("none"), "null");
    assert_eq!(cell("big"), u64::MAX.to_string());
    assert_eq!(cell("float"), "1.5e300");
    assert_eq!(cell("nested"), r#"{"a":[1,{"b":"ç"}]}"#);
    assert!(rendered.text.len() <= MAX_OUTPUT);

    // trunc=0 preserves full text.
    let (_, full) = fit(read(json!({"trunc":0})).await.unwrap()).unwrap();
    assert!(full.text.contains(&long));

    // JSON preserves values/types even with default truncation settings.
    let (json_out, rendered) = fit(read(json!({"format":"json"})).await.unwrap()).unwrap();
    assert!(json_out.meta.get("truncated").is_none());
    let parsed: Value = serde_json::from_str(&rendered.text).unwrap();
    assert_eq!(parsed, rendered.structured.unwrap());
    let row = &parsed["data"][0];
    assert_eq!(row["name"], json!(long));
    assert_eq!(
        (&row["flag"], &row["zero"], &row["empty"], &row["none"]),
        (&json!(false), &json!(0), &json!(""), &Value::Null)
    );
    assert_eq!(row["big"], json!(u64::MAX));
    assert_eq!(row["float"], json!(1.5e300));
    assert_eq!(row["m2o"], json!([1, "Main"]));

    // Explicitly fail unserializable Odoo fields and non-JSON values.
    let err = f
        .core
        .run_tool(
            &reader,
            "search_read",
            json!({"env":"A","model":"x.unserializable"}),
        )
        .await
        .unwrap_err();
    assert_eq!(
        (err.code, err.outcome),
        ("upstream_error", odx_core::Outcome::Failed)
    );
    assert!(err.message.contains("not JSON serializable"));
    let err = f
        .core
        .run_tool(&reader, "search_read", json!({"env":"A","model":"x.nan"}))
        .await
        .unwrap_err();
    assert_eq!(err.outcome, odx_core::Outcome::Failed);
    assert_eq!(err.code, "upstream_network");
    assert!(err.message.contains("invalid"));
}

#[tokio::test]
async fn credential_methods_are_forbidden_in_every_mode_without_approval() {
    let f = fixture().await;
    let (both, _) = client(
        &f,
        &[&f.a],
        &[Capability::OdooRead, Capability::OdooWriteRequest],
    )
    .await;
    let attempts = [
        (
            "call",
            json!({"env":"A","model":"res.users.apikeys","method":"generate",
                   "args":[],"kwargs":{"key":"k","scope":null,"name":"n","expiration_date":"2030-01-01"}}),
        ),
        (
            "call",
            json!({"env":"A","model":"res.users.apikeys.description","method":"make_key","args":[[1]]}),
        ),
        (
            "call",
            json!({"env":"A","model":"res.users","method":"api_key_wizard","args":[[2]]}),
        ),
        (
            "call",
            json!({"env":"A","model":"res.users.apikeys.show","method":"read","args":[[1],["key"]]}),
        ),
        (
            "create",
            json!({"env":"A","model":"res.users.apikeys.description","values":{"name":"n"}}),
        ),
        (
            "search_read",
            json!({"env":"A","model":"res.users.apikeys.show","fields":["key"]}),
        ),
    ];
    let modes = [
        WriteMode::ReadOnly,
        WriteMode::Approval,
        WriteMode::Temporary,
        WriteMode::Permanent,
    ];
    for mode in modes {
        if mode == WriteMode::Temporary {
            f.core.set_temporary_write(&f.a, 5).await.unwrap();
        } else {
            f.core.set_mode(&f.a, mode).await.unwrap();
        }
        for (tool, arguments) in &attempts {
            let err = tokio::time::timeout(
                Duration::from_secs(5),
                f.core.run_tool(&both, tool, arguments.clone()),
            )
            .await
            .expect("reject without awaiting approval")
            .unwrap_err();
            assert_eq!(
                (err.code, err.outcome),
                ("forbidden", odx_core::Outcome::NotSent),
                "{tool} {arguments}"
            );
            assert!(err.message.contains("API keys"), "{}", err.message);
        }
        assert!(f.core.approvals().await.unwrap().is_empty());
    }
    assert_eq!(f.sim.counts.queries(), 0);
    assert_eq!(f.sim.counts.writes(), 0);
    assert_eq!(f.sim.counts.logins(), 0);
    let activity = f
        .core
        .activity(None, odx_core::ActivityFilter::default())
        .await
        .unwrap();
    let denied = activity
        .events
        .iter()
        .filter(|e| e.outcome == "denied")
        .count();
    assert_eq!(denied, attempts.len() * modes.len());
}

#[tokio::test]
async fn x2many_shorthand_clear_or_set_needs_approval_even_in_free_write_mode() {
    let f = fixture().await;
    let (writer, _) = client(&f, &[&f.a], &[Capability::OdooWriteRequest]).await;
    f.core.set_mode(&f.a, WriteMode::Permanent).await.unwrap();
    // A plain false on a scalar field is an ordinary write.
    f.core
        .run_tool(
            &writer,
            "write",
            json!({"env":"A","model":"res.partner","ids":[1],"values":{"name":false}}),
        )
        .await
        .unwrap();
    assert_eq!(f.sim.counts.writes(), 1);
    assert!(f.core.approvals().await.unwrap().is_empty());
    // Odoo reads false as Command.clear() and [3] as Command.set([3]) on a many2many.
    // Odoo only checks that the first item is not a list: [3.0] and ["3"] are Command.set too.
    // Shorthand inside an update command clears the comodel's x2many as well.
    for values in [
        json!({"tag_ids": false}),
        json!({"tag_ids": [3]}),
        json!({"tag_ids": [3.0]}),
        json!({"tag_ids": ["3"]}),
        json!({"tag_ids": [[1, 5, {"tag_ids": false}]]}),
    ] {
        let core = f.core.clone();
        let w = writer.clone();
        let pending = tokio::spawn(async move {
            core.run_tool(
                &w,
                "write",
                json!({"env":"A","model":"res.partner","ids":[1],"values":values}),
            )
            .await
        });
        let approval = wait_pending(&f.core).await;
        assert_eq!(approval["method"], "write");
        f.core
            .decide(approval["id"].as_str().unwrap(), false)
            .await
            .unwrap();
        assert_eq!(
            pending.await.unwrap().unwrap_err().code,
            "approval_rejected"
        );
    }
    assert_eq!(f.sim.counts.writes(), 1);
    // A new child's x2many shorthand removes nothing: an ordinary write, on any number of records.
    f.core
        .run_tool(
            &writer,
            "write",
            json!({"env":"A","model":"res.partner","ids":[1,2],"values":{"tag_ids":[[0, 0, {"tag_ids": [3]}]]}}),
        )
        .await
        .unwrap();
    assert_eq!(f.sim.counts.writes(), 2);
    // Shorthand on several records is refused like an explicit relational delete.
    let error = f
        .core
        .run_tool(
            &writer,
            "write",
            json!({"env":"A","model":"res.partner","ids":[1,2],"values":{"tag_ids":false}}),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "invalid_arguments");
}

#[tokio::test]
async fn relational_deletes_in_the_call_context_are_refused() {
    let f = fixture().await;
    let (writer, _) = client(&f, &[&f.a], &[Capability::OdooWriteRequest]).await;
    f.core.set_mode(&f.a, WriteMode::Permanent).await.unwrap();
    // Odoo's default_get would apply this DELETE to the partner being created.
    for (tool, arguments) in [
        (
            "create",
            json!({"env":"A","model":"res.partner","values":{"name":"x"},"context":{"default_child_ids":[[2, 42]]}}),
        ),
        (
            "call",
            json!({"env":"A","model":"res.partner","method":"name_create","args":["x"],"kwargs":{"context":{"default_child_ids":[[5, 0, 0]]}}}),
        ),
    ] {
        let error = f.core.run_tool(&writer, tool, arguments).await.unwrap_err();
        assert_eq!(error.code, "invalid_arguments", "{tool}");
    }
    assert_eq!(f.sim.counts.writes(), 0);
    assert!(f.core.approvals().await.unwrap().is_empty());
}
