//! wait, read_group, ref, history, view and access against simulated Odoo.
mod flow;
mod four_envs;
mod support;
use four_envs::fixture;
use odx_core::{ActivityFilter, Outcome};
use serde_json::{Value, json};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}
fn warnings(out: &odx_core::ToolOutput) -> Vec<String> {
    out.meta
        .get("warnings")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|w| w["code"].as_str().map(String::from))
        .collect()
}
async fn operations(f: &four_envs::Fixture) -> Vec<String> {
    f.core
        .activity(None, ActivityFilter::default())
        .await
        .unwrap()
        .events
        .into_iter()
        .map(|e| format!("{}:{}", e.operation, e.outcome))
        .collect()
}

// wait

#[tokio::test]
async fn wait_matches_presence_and_absence() {
    let f = fixture().await;
    // Present on the first poll.
    let until = now_ms() + 60_000;
    let out = f
        .run(
            "wait",
            json!({"env":"sim-a","model":"x.queue","domain":[["create_date","=",until]]}),
        )
        .await
        .unwrap();
    assert_eq!(out.data["matched"], true);
    assert_eq!(out.data["count"], 1);
    assert_eq!(out.meta["polls"], 1);
    // Record disappears after 1.5 s; the next poll arrives at least 3 s later.
    let until = now_ms() + 1_500;
    let started = Instant::now();
    let out = f
        .run(
            "wait",
            json!({"env":"sim-a","model":"x.queue","domain":[["create_date","=",until]],"absent":true,"timeout":20}),
        )
        .await
        .unwrap();
    assert_eq!(out.data["matched"], true);
    assert_eq!(out.data["count"], 0);
    assert_eq!(out.meta["polls"], 2);
    assert!(
        started.elapsed() >= Duration::from_secs(3),
        "minimum interval 3 s"
    );
    assert!(out.data["elapsed_seconds"].as_f64().unwrap() >= 3.0);
}

#[tokio::test]
async fn wait_timeout_is_a_normal_result_and_limits_are_validated() {
    let f = fixture().await;
    let until = now_ms() + 600_000;
    let started = Instant::now();
    let out = f
        .run(
            "wait",
            json!({"env":"sim-a","model":"x.queue","domain":[["create_date","=",until]],"absent":true,"timeout":1}),
        )
        .await
        .unwrap();
    // A 1 s budget allows one attempt with a 3 s polling interval.
    assert_eq!(out.data["matched"], false);
    assert_eq!(out.meta["polls"], 1);
    assert!(
        started.elapsed() >= Duration::from_secs(1),
        "do not time out early"
    );
    assert!(started.elapsed() < Duration::from_secs(2));
    let queries = f.sim.counts.queries();
    for bad in [
        json!({"env":"sim-a","model":"x.queue","domain":[],"interval":2}),
        json!({"env":"sim-a","model":"x.queue","domain":[],"timeout":601}),
        json!({"env":"sim-a","model":"x.queue","domain":[],"timeout":0}),
        json!({"env":"sim-a","model":"x.queue"}),
    ] {
        let e = f.run("wait", bad.clone()).await.unwrap_err();
        assert_eq!(
            (e.code, e.outcome),
            ("invalid_arguments", Outcome::NotSent),
            "{bad}"
        );
    }
    assert_eq!(
        f.sim.counts.queries(),
        queries,
        "validate before network access"
    );
}

#[tokio::test]
async fn cancelling_wait_stops_polling_and_holds_no_upstream_slot() {
    let f = fixture().await;
    let until = now_ms() + 600_000;
    let call = {
        let (core, client) = (f.core.clone(), f.client.clone());
        tokio::spawn(async move {
            core.run_tool(
                &client,
                "wait",
                json!({"env":"sim-a","model":"x.queue","domain":[["create_date","=",until]],"absent":true,"timeout":2}),
            )
            .await
        })
    };
    four_envs::until("first poll", || f.sim.counts.queries() >= 1).await;
    // No calls remain in flight between polls.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(f.sim.counts.in_flight_total(), 0);
    let polled = f.sim.counts.queries();
    call.abort();
    assert!(call.await.unwrap_err().is_cancelled());
    tokio::time::sleep(Duration::from_millis(3_500)).await;
    assert_eq!(
        f.sim.counts.queries(),
        polled,
        "no polls after cancellation"
    );
}

#[tokio::test]
async fn wait_revalidates_the_grant_before_each_poll() {
    let f = fixture().await;
    let until = now_ms() + 600_000;
    let call = {
        let (core, client) = (f.core.clone(), f.client.clone());
        tokio::spawn(async move {
            core.run_tool(
                &client,
                "wait",
                json!({"env":"sim-a","model":"x.queue","domain":[["create_date","=",until]],"absent":true,"timeout":20}),
            )
            .await
        })
    };
    four_envs::until("first poll", || f.sim.counts.queries() >= 1).await;
    let polled = f.sim.counts.queries();
    f.core.revoke_client(&f.client.client_id).await.unwrap();
    let e = call.await.unwrap().unwrap_err();
    assert_eq!(e.code, "unauthenticated");
    assert_eq!(
        f.sim.counts.queries(),
        polled,
        "no polling with revoked grants"
    );
}

// read_group

#[tokio::test]
async fn read_group_pages_groups_with_lazy_false() {
    let f = fixture().await;
    let out = f
        .run(
            "read_group",
            json!({"env":"sim-a","model":"res.partner","groupby":["parent_id"],"fields":["amount_total:sum"],"limit":2}),
        )
        .await
        .unwrap();
    let rows = out.data.as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["count"], 1);
    assert_eq!(rows[0]["parent_id"], json!([1, "P1"]));
    assert!(rows[0].get("__domain").is_none() && rows[0].get("__range").is_none());
    assert_eq!(out.meta["page"]["has_more"], true);
    assert_eq!(out.meta["page"]["next_offset"], 2);
    let last = f
        .run(
            "read_group",
            json!({"env":"sim-a","model":"res.partner","groupby":["parent_id"],"limit":2,"offset":4}),
        )
        .await
        .unwrap();
    assert_eq!(last.data.as_array().unwrap().len(), 1);
    assert_eq!(last.meta["page"]["has_more"], false);
    let queries = f.sim.counts.queries();
    for bad in [
        json!({"env":"sim-a","model":"res.partner","groupby":[]}),
        json!({"env":"sim-a","model":"res.partner","groupby":["x;drop"]}),
        json!({"env":"sim-a","model":"res.partner","groupby":["a"],"fields":["sum(x)"]}),
        json!({"env":"sim-a","model":"res.partner","groupby":["a"],"orderby":"a; drop"}),
    ] {
        let e = f.run("read_group", bad.clone()).await.unwrap_err();
        assert_eq!(e.code, "invalid_arguments", "{bad}");
    }
    assert_eq!(f.sim.counts.queries(), queries);
}

// ref

#[tokio::test]
async fn ref_resolves_without_fallback() {
    let f = fixture().await;
    let out = f
        .run("ref", json!({"env":"sim-a","xmlid":"base.main_company"}))
        .await
        .unwrap();
    assert_eq!(out.data, json!({"model":"res.company","id":1}));
    assert!(warnings(&out).is_empty());
    for (xmlid, code) in [
        ("base.missing", "not_found"),
        ("base.dangling", "not_found"),
    ] {
        let e = f
            .run("ref", json!({"env":"sim-a","xmlid":xmlid}))
            .await
            .unwrap_err();
        assert_eq!((e.code, e.outcome), (code, Outcome::Failed), "{xmlid}");
        assert_eq!(e.http_status(), 404);
    }
    // Unverifiable targets return with a warning.
    let out = f
        .run("ref", json!({"env":"sim-a","xmlid":"base.secret"}))
        .await
        .unwrap();
    assert_eq!(out.data["model"], "x.forbidden");
    assert_eq!(warnings(&out), ["existence_unverified"]);
    for bad in ["nodot", "a.b.c", "a .b"] {
        let e = f
            .run("ref", json!({"env":"sim-a","xmlid":bad}))
            .await
            .unwrap_err();
        assert_eq!(e.code, "invalid_arguments", "{bad}");
    }
}

// history

#[tokio::test]
async fn history_reads_tracking_in_odoo_16_and_17_plus_columns() {
    let f = fixture().await;
    for env in ["sim-a", "sim-16"] {
        let out = f
            .run(
                "history",
                json!({"env":env,"model":"res.partner","id":1,"format":"json"}),
            )
            .await
            .unwrap();
        let rows = out.data.as_array().unwrap();
        let ids: Vec<_> = rows.iter().map(|r| r["id"].as_i64().unwrap()).collect();
        assert_eq!(ids, [30, 20, 10], "descending ID order in {env}");
        assert_eq!(
            rows[0]["changes"],
            json!([
                {"field":"state","label":"Status","old":"Draft","new":"Done"},
                {"field":"amount","label":"Amount","old":10.0,"new":12.5},
            ]),
            "{env}"
        );
        assert_eq!(rows[1]["changes"], json!([]));
        assert_eq!(
            rows[2]["changes"],
            json!([{"field":"partner_id","label":"Partner","old":false,"new":[7,"ACME"]}])
        );
        assert_eq!(rows[0]["author"], json!([3, "Admin"]));
        assert!(warnings(&out).is_empty(), "{env}");
    }
    let page = f
        .run(
            "history",
            json!({"env":"sim-a","model":"res.partner","id":1,"limit":1,"offset":1}),
        )
        .await
        .unwrap();
    assert_eq!(page.data[0]["id"], 20);
    assert_eq!(page.meta["page"]["next_offset"], 2);
    let empty = f
        .run(
            "history",
            json!({"env":"sim-a","model":"res.partner","id":2}),
        )
        .await
        .unwrap();
    assert_eq!(empty.data, json!([]));
    assert_eq!(warnings(&empty), ["no_visible_messages"]);
}

#[tokio::test]
async fn history_without_tracking_access_warns_instead_of_failing() {
    // tracking_value_ids and mail.tracking.value require base.group_system in 16–19.
    let f = fixture().await;
    let out = f
        .run(
            "history",
            json!({"env":"sim-b","model":"res.partner","id":1}),
        )
        .await
        .unwrap();
    let rows = out.data.as_array().unwrap();
    assert_eq!(rows.len(), 3);
    assert!(
        rows.iter().all(|r| r["changes"].is_null()),
        "unknown changes are not empty changes"
    );
    assert_eq!(warnings(&out), ["partial_permissions"]);
}

// view

#[tokio::test]
async fn view_returns_final_arch_and_bounded_inheritance_chain() {
    let f = fixture().await;
    let out = f
        .run("view", json!({"env":"sim-a","model":"sale.order"}))
        .await
        .unwrap();
    assert!(out.data["arch"].as_str().unwrap().contains("x_foo"));
    let chain: Vec<_> = out.data["views"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["id"].as_i64().unwrap())
        .collect();
    assert_eq!(chain, [100, 101, 102, 103]);
    assert_eq!(out.data["view_id"], 100);
    // q marks matches and source views; limit pages the chain.
    let out = f
        .run(
            "view",
            json!({"env":"sim-a","model":"sale.order","q":"x_foo","limit":2}),
        )
        .await
        .unwrap();
    assert!(out.data["arch"].is_null());
    assert_eq!(out.data["matches"][0]["text"], "<field name=\"x_foo\"/>");
    assert_eq!(out.data["views"][1]["has_q"], true);
    assert_eq!(out.data["views"][0]["has_q"], false);
    assert!(out.data["views"][0].get("arch_db").is_none());
    assert_eq!(out.meta["page"]["has_more"], true);
    // offset continues the chain instead of repeating its start.
    let out = f
        .run(
            "view",
            json!({"env":"sim-a","model":"sale.order","limit":2,"offset":2}),
        )
        .await
        .unwrap();
    let ids: Vec<_> = out.data["views"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["id"].as_i64().unwrap())
        .collect();
    assert_eq!(ids, [102, 103]);
    assert_eq!(out.meta["page"]["has_more"], false);
    // Detect inheritance cycles without looping.
    let out = f
        .run("view", json!({"env":"sim-a","model":"x.cycle"}))
        .await
        .unwrap();
    assert!(warnings(&out).contains(&"inheritance_cycle".to_string()));
    // Generated views have no records to list.
    let out = f
        .run(
            "view",
            json!({"env":"sim-a","model":"sale.order","view_type":"kanban"}),
        )
        .await
        .unwrap();
    assert_eq!(out.data["views"], json!([]));
    assert_eq!(warnings(&out), ["no_view_record"]);
}

#[tokio::test]
async fn view_with_unreadable_chain_or_large_arch_is_flagged() {
    let f = fixture().await;
    let out = f
        .run("view", json!({"env":"sim-a","model":"x.noviews"}))
        .await
        .unwrap();
    assert_eq!(out.data["arch"], "<form/>");
    assert!(
        out.data["views"].is_null(),
        "unknown chain is not an empty chain"
    );
    assert_eq!(warnings(&out), ["partial_permissions"]);
    let out = f
        .run("view", json!({"env":"sim-a","model":"x.big"}))
        .await
        .unwrap();
    assert_eq!(out.meta["truncated"], true);
    let arch = out.data["arch"].as_str().unwrap();
    assert!(arch.ends_with("/>") && arch.len() < out.data["arch_bytes"].as_u64().unwrap() as usize);
    assert!(warnings(&out).contains(&"arch_truncated".to_string()));
    let rendered = odx_core::output::render(&out);
    assert!(rendered.text.len() <= odx_core::output::MAX_OUTPUT);
    // JSON carries the arch twice; a 150 KiB arch is cut instead of failing as output_too_large.
    let out = f
        .run(
            "view",
            json!({"env":"sim-a","model":"x.medium","format":"json"}),
        )
        .await
        .unwrap();
    assert_eq!(out.meta["truncated"], true);
    assert!(odx_core::output::fit(out).is_ok());
    let e = f
        .run("view", json!({"env":"sim-a","model":"x.unknown"}))
        .await
        .unwrap_err();
    assert_eq!(e.code, "upstream_error");
}

// access

#[tokio::test]
async fn access_reports_rights_acls_and_rules_with_named_continuations() {
    let f = fixture().await;
    let out = f
        .run("access", json!({"env":"sim-a","model":"res.partner"}))
        .await
        .unwrap();
    assert_eq!(
        out.data["rights"],
        json!({"read":true,"write":true,"create":false,"unlink":false})
    );
    let applies: Vec<_> = out.data["acls"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| {
            (
                a["perms"].as_str().unwrap().to_string(),
                a["applies"].clone(),
            )
        })
        .collect();
    assert_eq!(
        applies,
        [
            ("rwc-".to_string(), json!(true)),
            ("rwcd".to_string(), json!(false)),
            ("r---".to_string(), json!(true)),
        ]
    );
    assert_eq!(out.data["rules"][0]["global"], true);
    assert_eq!(out.data["rules"][1]["applies"], false);
    assert!(out.meta.get("page").is_none(), "no ambiguous next_offset");
    assert_eq!(out.meta["continuations"]["acls"]["has_more"], false);
    let paged = f
        .run(
            "access",
            json!({"env":"sim-a","model":"res.partner","limit":1}),
        )
        .await
        .unwrap();
    assert_eq!(paged.meta["continuations"]["acls"]["next_offset"], 1);
    assert_eq!(paged.meta["continuations"]["rules"]["next_offset"], 1);
    // Odoo 19 uses all_group_ids; unreadable ACLs yield null and a warning.
    let out = f
        .run("access", json!({"env":"sim-19","model":"x.secret"}))
        .await
        .unwrap();
    assert!(out.data["acls"].is_null());
    assert_eq!(out.data["rules"], json!([]));
    assert_eq!(warnings(&out), ["partial_permissions"]);
}

#[tokio::test]
async fn every_subcall_is_audited_with_the_uniform_context() {
    let f = fixture().await;
    f.run(
        "access",
        json!({"env":"sim-a","model":"res.partner","context":{"tz":"Europe/Madrid"}}),
    )
    .await
    .unwrap();
    let ops = operations(&f).await;
    for op in [
        "check_access_rights:res.partner",
        "fields_get:res.users",
        "read:res.users",
        "search_read:ir.model.access",
        "search_read:ir.rule",
    ] {
        assert!(
            ops.iter().any(|o| o == &format!("{op}:succeeded")),
            "{op}: {ops:?}"
        );
    }
    assert_eq!(
        ops.iter()
            .filter(|o| *o == "check_access_rights:res.partner:succeeded")
            .count(),
        4
    );
    // Merge global < environment < call context for each subcall.
    let events = f
        .core
        .activity(None, ActivityFilter::default())
        .await
        .unwrap()
        .events;
    let contexts: Vec<String> = events
        .iter()
        .filter_map(|e| e.details.as_ref()?.request.as_ref())
        .map(|r| r.text.clone())
        .collect();
    assert!(!contexts.is_empty());
    assert!(
        contexts
            .iter()
            .all(|d| d.contains("Europe/Madrid") && d.contains("es_ES")),
        "{contexts:?}"
    );
    four_envs::assert_no_secret(&f, &[]).await;
}
