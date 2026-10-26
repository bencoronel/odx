//! Limits: 16/4 upstream slots, 64/16 queues with 10 s deadlines, I/O sizes and connection/operation timeouts. Core cases use run_tool against simulated Odoo.
mod flow;
mod support;
use odx_core::{
    Capability, ClientIdentity, ClientScope, Core, EnvironmentDraft, Format, Outcome, Protocol,
    ToolError, ToolOutput, WriteMode,
    error::OdxError,
    limits::Limits,
    output::{self, MAX_OUTPUT},
    transports::http,
};
use serde_json::{Map, Value, json};
use std::sync::Arc;
use tokio::{
    task::JoinHandle,
    time::{Duration, Instant, sleep, timeout},
};

async fn wait_for(depth: impl Fn() -> usize, expected: usize) {
    timeout(Duration::from_secs(1), async {
        while depth() != expected {
            sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("request reached expected queue depth");
}

#[tokio::test]
async fn global_queue_is_fifo_bounded_and_releases_capacity() {
    let limits = Limits::with_caps(1, 2, 2, 2, 5);
    let held = limits.acquire("holder").await.unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

    for label in ['a', 'b'] {
        let worker_limits = limits.clone();
        let tx = tx.clone();
        tokio::spawn(async move {
            let permit = worker_limits.acquire("other").await.unwrap();
            tx.send((label, permit)).unwrap();
        });
        wait_for(
            || limits.queue_depth("other").unwrap().global,
            (label as usize - 'a' as usize) + 1,
        )
        .await;
    }

    assert!(matches!(
        limits.acquire("third").await,
        Err(OdxError::Capacity)
    ));
    drop(held);
    let (first_label, first_permit) = timeout(Duration::from_secs(1), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first_label, 'a', "Tokio semaphore waiters are served FIFO");
    assert!(timeout(Duration::from_millis(20), rx.recv()).await.is_err());
    drop(first_permit);
    let (second_label, second_permit) = timeout(Duration::from_secs(1), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(second_label, 'b');
    drop(second_permit);
}

#[tokio::test]
async fn environment_queue_is_bounded_and_does_not_block_other_environments() {
    let limits = Limits::with_caps(4, 1, 4, 1, 4);
    let held = limits.acquire("busy").await.unwrap();
    let waiting = {
        let limits = limits.clone();
        tokio::spawn(async move { limits.acquire("busy").await })
    };
    wait_for(|| limits.queue_depth("busy").unwrap().environment, 1).await;
    assert!(matches!(
        limits.acquire("busy").await,
        Err(OdxError::Capacity)
    ));

    let independent = limits.acquire("free").await.unwrap();
    drop(independent);
    drop(held);
    drop(waiting.await.unwrap().unwrap());
    assert_eq!(limits.queue_depth("busy").unwrap().environment, 0);
}

#[tokio::test]
async fn global_admission_cap_rejects_before_adding_more_waiters() {
    let limits = Limits::with_caps(1, 1, 3, 3, 2);
    let held = limits.acquire("a").await.unwrap();
    let waiting = {
        let limits = limits.clone();
        tokio::spawn(async move { limits.acquire("b").await })
    };
    wait_for(|| limits.queue_depth("b").unwrap().global, 1).await;
    assert!(matches!(limits.acquire("c").await, Err(OdxError::Capacity)));
    drop(held);
    drop(waiting.await.unwrap().unwrap());
}

// Core run_tool integration

struct Fixture {
    core: Arc<Core>,
    sim: support::Simulator,
    /// Environment E{i} uses database db-{i}.
    envs: Vec<String>,
    client: ClientIdentity,
    token: String,
    _tmp: tempfile::TempDir,
}

async fn fixture(count: usize) -> Fixture {
    fixture_at(count, None).await
}

/// Override the simulator URL with an unresponsive target.
async fn fixture_at(count: usize, url: Option<String>) -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let sim = support::simulator().await;
    let core = support::unlocked_core(&tmp).await;
    let mut envs = Vec::new();
    for i in 0..count {
        let env = core
            .save_environment(EnvironmentDraft {
                name: format!("E{i}"),
                url: url.clone().unwrap_or_else(|| sim.url.clone()),
                database: format!("db-{i}"),
                login: "fixture-user".into(),
                protocol: Protocol::LegacyJsonrpc,
                context: json!({}),
                secret: "fixture-secret".into(),
            })
            .await
            .unwrap();
        envs.push(env.id);
    }
    let issued = core
        .create_client(
            "limits",
            ClientScope::All,
            vec![Capability::OdooRead, Capability::OdooWriteRequest],
        )
        .await
        .unwrap();
    let client = core.authenticate_client(&issued.token).await.unwrap();
    Fixture {
        core,
        sim,
        envs,
        client,
        token: issued.token.clone(),
        _tmp: tmp,
    }
}

type Call = JoinHandle<Result<ToolOutput, ToolError>>;

fn spawn(f: &Fixture, tool: &'static str, args: Value) -> Call {
    let (core, client) = (f.core.clone(), f.client.clone());
    tokio::spawn(async move { core.run_tool(&client, tool, args).await })
}
fn read(env: usize) -> Value {
    json!({"env": format!("E{env}"), "model": "res.partner"})
}
async fn until(what: &str, condition: impl Fn() -> bool) {
    timeout(Duration::from_secs(5), async {
        while !condition() {
            sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("condition not reached: {what}"));
}
/// Allow spawned tasks to reach admission waits without pending I/O.
async fn settle() {
    sleep(Duration::from_millis(200)).await;
}
fn capacity(result: Result<ToolOutput, ToolError>) {
    let err = result.expect_err("expected capacity_exceeded");
    assert_eq!(
        (err.code, err.outcome, err.http_status()),
        ("capacity_exceeded", Outcome::NotSent, 429)
    );
}

#[tokio::test]
async fn saturated_environment_queues_16_rejects_the_next_and_leaves_others_free() {
    let f = fixture(2).await;
    f.sim.counts.hold_database("db-0");
    let mut calls: Vec<Call> = (0..4).map(|_| spawn(&f, "search_read", read(0))).collect();
    until("4 in-flight reads in E0", || {
        f.sim.counts.in_flight("db-0") == 4
    })
    .await;
    calls.extend((0..16).map(|_| spawn(&f, "search_read", read(0))));
    settle().await;
    assert_eq!(
        f.sim.counts.in_flight("db-0"),
        4,
        "queued calls do not reach Odoo"
    );

    // A full environment queue rejects immediately, without the 10 s wait.
    let started = Instant::now();
    capacity(f.core.run_tool(&f.client, "search_read", read(0)).await);
    assert!(started.elapsed() < Duration::from_secs(1));

    // Fairness: E1 does not wait for E0.
    let started = Instant::now();
    let other = f.core.run_tool(&f.client, "search_read", read(1)).await;
    assert_eq!(other.unwrap().data[0]["display_name"], "contact-db-1");
    assert!(started.elapsed() < Duration::from_secs(1));

    f.sim.counts.release_databases();
    for call in calls {
        assert!(call.await.unwrap().is_ok(), "all 20 admitted calls finish");
    }
    assert_eq!(f.sim.counts.max_in_flight("db-0"), 4);
}

#[tokio::test]
async fn global_cap_16_and_queue_64_reject_the_next_without_upstream_calls() {
    let f = fixture(20).await;
    for db in 0..4 {
        f.sim.counts.hold_database(&format!("db-{db}"));
    }
    let mut calls: Vec<Call> = (0..16)
        .map(|i| spawn(&f, "search_read", read(i % 4)))
        .collect();
    until("16 in-flight reads", || {
        f.sim.counts.in_flight_total() == 16
    })
    .await;
    // 64 global waiters, 4 per free environment; release must not starve local admission.
    calls.extend((0..64).map(|i| spawn(&f, "search_read", read(4 + i % 16))));
    settle().await;
    assert_eq!(f.sim.counts.in_flight_total(), 16);
    let queries = f.sim.counts.queries();

    capacity(f.core.run_tool(&f.client, "search_read", read(19)).await);
    assert_eq!(
        f.sim.counts.queries(),
        queries,
        "rejection does not contact Odoo"
    );

    f.sim.counts.release_databases();
    for call in calls {
        assert!(call.await.unwrap().is_ok(), "all 80 admitted calls finish");
    }
    assert_eq!(f.sim.counts.max_in_flight_total(), 16);
    for db in 0..20 {
        assert!(f.sim.counts.max_in_flight(&format!("db-{db}")) <= 4);
    }
}

#[tokio::test]
async fn reads_proceed_while_approvals_are_pending_without_upstream_slots() {
    let f = fixture(1).await;
    f.core
        .set_mode(&f.envs[0], WriteMode::Approval)
        .await
        .unwrap();
    let write = |n: usize| json!({"env":"E0","model":"res.partner","ids":[1],"values":{"name":format!("w{n}")}});
    let writes: Vec<Call> = (0..20).map(|n| spawn(&f, "write", write(n))).collect();
    let core = f.core.clone();
    timeout(Duration::from_secs(5), async {
        while core.approvals().await.unwrap().len() < 20 {
            sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("20 pending approvals");
    // 20 pending requests per environment limits requests, not upstream slots.
    capacity(f.core.run_tool(&f.client, "write", write(20)).await);

    f.sim.counts.hold_database("db-0");
    let reads: Vec<Call> = (0..4).map(|_| spawn(&f, "search_read", read(0))).collect();
    until("4 in-flight reads with 20 pending approvals", || {
        f.sim.counts.in_flight("db-0") == 4
    })
    .await;
    f.sim.counts.release_databases();
    for call in reads {
        assert!(call.await.unwrap().is_ok());
    }
    assert_eq!(f.core.approvals().await.unwrap().len(), 20);
    assert_eq!(f.sim.counts.writes(), 0);
    for call in writes {
        call.abort();
    }
}

// Use real time during audit/login before queuing; paused time would jump during real I/O (see the 60 s test).
#[tokio::test]
async fn queue_wait_gives_up_at_the_deadline_with_capacity_exceeded() {
    let f = fixture(1).await;
    // The deadline is 10 s; a short one checks the same path without waiting it out.
    f.core.set_queue_wait(Duration::from_millis(500));
    f.sim.counts.hold_database("db-0");
    let held: Vec<Call> = (0..4).map(|_| spawn(&f, "search_read", read(0))).collect();
    until("4 in-flight reads", || f.sim.counts.in_flight("db-0") == 4).await;

    let queued = Instant::now();
    capacity(f.core.run_tool(&f.client, "search_read", read(0)).await);
    let waited = queued.elapsed();
    assert!(
        waited >= Duration::from_millis(500) && waited < Duration::from_secs(2),
        "queue deadline: {waited:?}"
    );
    f.sim.counts.release_databases();
    for call in held {
        assert!(call.await.unwrap().is_ok());
    }
}

// Pause only after the simulator holds the request. start_paused jumps during loopback I/O, making timeout measurements meaningless (a read once measured 120 virtual seconds).
#[tokio::test]
async fn silent_operation_ends_at_60s_with_upstream_timeout() {
    let f = fixture(1).await;
    // Log in under real time; only the held call remains afterward.
    f.core
        .run_tool(&f.client, "search_read", read(0))
        .await
        .unwrap();
    f.sim.counts.hold_database("db-0");
    let started = Instant::now();
    let call = spawn(&f, "search_read", read(0));
    until("read held in Odoo", || f.sim.counts.in_flight("db-0") == 1).await;
    tokio::time::pause();
    let err = call.await.unwrap().unwrap_err();
    let elapsed = started.elapsed();
    tokio::time::resume();
    assert_eq!(
        (err.code, err.outcome),
        ("upstream_timeout", Outcome::Failed)
    );
    assert!(
        elapsed >= Duration::from_secs(60) && elapsed < Duration::from_secs(62),
        "operation deadline: {elapsed:?}"
    );
    f.sim.counts.release_databases();
}

#[tokio::test]
async fn connection_attempt_ends_at_5s() {
    // Fill the accept queue so the kernel drops SYN and connection stalls.
    let socket = tokio::net::TcpSocket::new_v4().unwrap();
    socket.bind("127.0.0.1:0".parse().unwrap()).unwrap();
    let listener = socket.listen(0).unwrap();
    let addr = listener.local_addr().unwrap();
    let _fill = std::net::TcpStream::connect(addr).unwrap();
    let f = fixture_at(1, Some(format!("http://{addr}"))).await;

    let started = Instant::now();
    let err = f
        .core
        .run_tool(&f.client, "search_read", read(0))
        .await
        .unwrap_err();
    let elapsed = started.elapsed();
    assert_eq!(
        (err.code, err.outcome),
        ("upstream_timeout", Outcome::Failed)
    );
    assert!(
        elapsed >= Duration::from_secs(5) && elapsed < Duration::from_secs(7),
        "connection deadline: {elapsed:?}"
    );
}

#[tokio::test]
async fn argument_bounds_are_checked_before_any_upstream_call() {
    let f = fixture(1).await;
    let ids = |n: i64| (1..=n).collect::<Vec<_>>();
    let ok = f
        .core
        .run_tool(
            &f.client,
            "read",
            json!({"env":"E0","model":"res.partner","ids":ids(1000)}),
        )
        .await
        .unwrap();
    assert_eq!(ok.data.as_array().unwrap().len(), 3);
    assert_eq!(ok.meta["missing_ids"].as_array().unwrap().len(), 997);
    let queries = f.sim.counts.queries();
    let logins = f.sim.counts.logins();
    for (tool, args) in [
        (
            "read",
            json!({"env":"E0","model":"res.partner","ids":ids(1001)}),
        ),
        (
            "search_read",
            json!({"env":"E0","model":"res.partner","limit":1001}),
        ),
        (
            "search",
            json!({"env":"E0","model":"res.partner","limit":0}),
        ),
    ] {
        let err = f.core.run_tool(&f.client, tool, args).await.unwrap_err();
        assert_eq!(
            (err.code, err.outcome),
            ("invalid_arguments", Outcome::NotSent)
        );
    }
    assert_eq!(
        (f.sim.counts.queries(), f.sim.counts.logins()),
        (queries, logins)
    );
}

#[tokio::test]
async fn http_body_of_exactly_1_mib_is_accepted_and_one_more_byte_is_413() {
    let f = fixture(1).await;
    let listener = http::bind(0).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(http::serve(
        f.core.clone(),
        listener,
        std::future::pending(),
    ));
    let body = |size: usize| {
        let head = br#"{"env":"E0","model":"res.partner""#;
        let mut body = head.to_vec();
        body.resize(size - 1, b' ');
        body.push(b'}');
        body
    };
    let post = |body: Vec<u8>| {
        reqwest::Client::new()
            .post(format!("http://127.0.0.1:{port}/api/v1/search_read"))
            .header("Content-Type", "application/json")
            .bearer_auth(&f.token)
            .body(body)
            .send()
    };
    let at = post(body(1024 * 1024)).await.unwrap();
    assert_eq!(at.status(), 200);
    assert_eq!(
        at.json::<Value>().await.unwrap()["data"][0]["display_name"],
        "contact-db-0"
    );
    let queries = f.sim.counts.queries();
    let over = post(body(1024 * 1024 + 1)).await.unwrap();
    assert_eq!(over.status(), 413);
    assert_eq!(
        over.json::<Value>().await.unwrap()["error"]["code"],
        "request_too_large"
    );
    assert_eq!(f.sim.counts.queries(), queries);
}

#[tokio::test]
async fn output_is_cut_by_whole_rows_at_256_kib_and_marked() {
    let f = fixture(1).await;
    let big = json!({"env":"E0","model":"limits.big","limit":1000,"trunc":0});
    let out = f
        .core
        .run_tool(&f.client, "search_read", big)
        .await
        .unwrap();
    assert_eq!(out.data.as_array().unwrap().len(), 1000);
    let (fitted, rendered) = output::fit(out).unwrap();
    let returned = fitted.data.as_array().unwrap().len();
    assert!(rendered.text.len() <= MAX_OUTPUT && returned < 1000 && returned > 0);
    eprintln!(
        "1000 rows of 1000 characters: {returned} remain, {} bytes",
        rendered.text.len()
    );
    assert_eq!(fitted.meta["truncated"], true);
    assert_eq!(
        fitted.meta["page"],
        json!({"limit":1000,"offset":0,"returned":returned,"has_more":true,"next_offset":returned})
    );

    // Exact size passes unchanged; one extra byte returns output_too_large because there are no rows to trim. JSON counts both representations,
    // the text as sent inside JSON-RPC: two outer quotes and four escaped inner ones.
    let sized = |n: usize| ToolOutput {
        data: json!("y".repeat(n)),
        meta: Map::new(),
        format: Format::Json,
        trunc: 0,
    };
    let exact = (MAX_OUTPUT - 6) / 2 - r#"{"data":""}"#.len();
    let (kept, rendered) = output::fit(sized(exact)).unwrap();
    assert_eq!(rendered.text.len() * 2 + 6, MAX_OUTPUT);
    assert!(kept.meta.get("truncated").is_none());
    let err = output::fit(sized(exact + 1)).unwrap_err();
    assert_eq!((err.code, err.http_status()), ("output_too_large", 413));
}

#[tokio::test]
async fn upstream_response_over_16_mib_is_refused() {
    let f = fixture(1).await;
    let err = f
        .core
        .run_tool(
            &f.client,
            "search_read",
            json!({"env":"E0","model":"limits.huge"}),
        )
        .await
        .unwrap_err();
    assert_eq!(err.outcome, Outcome::Failed);
    assert_eq!((err.code, err.http_status()), ("output_too_large", 413));
    assert!(err.message.contains("16 MiB"), "{}", err.message);
}

#[tokio::test]
async fn read_meta_reports_upstream_calls_bytes_and_latency() {
    let f = fixture(1).await;
    let big = json!({"env":"E0","model":"limits.big","limit":20});
    let mut report = Vec::new();
    for (round, args) in [
        ("first call with login", big.clone()),
        ("second call reuses session", big),
        ("default res.partner", read(0)),
    ] {
        let before = f.sim.counts.response_bytes();
        let started = Instant::now();
        let out = f
            .core
            .run_tool(&f.client, "search_read", args)
            .await
            .unwrap();
        let wall = started.elapsed();
        let received = f.sim.counts.response_bytes() - before;
        assert_eq!(out.meta["upstream_bytes"], json!(received));
        let compact = output::fit(out.clone()).unwrap().1.text.len();
        let as_json = output::fit(ToolOutput {
            format: Format::Json,
            ..out.clone()
        })
        .unwrap()
        .1;
        report.push(format!(
            "{round}: upstream_calls={} upstream_bytes={} upstream_ms={} total_ms={} \
             compact_bytes={compact} json_text_bytes={} json_mcp_bytes={}",
            out.meta["upstream_calls"],
            out.meta["upstream_bytes"],
            out.meta["upstream_ms"],
            wall.as_millis(),
            as_json.text.len(),
            as_json.text.len() + as_json.structured.unwrap().to_string().len(),
        ));
        out.meta["upstream_ms"].as_u64().expect("measured latency");
    }
    assert!(report[0].starts_with("first call with login: upstream_calls=2"));
    assert!(report[1].starts_with("second call reuses session: upstream_calls=1"));
    eprintln!("{}", report.join("\n"));
}

/// Await the sole approval, advance wait and approve it.
async fn approve_after(f: &Fixture, wait: Duration) {
    let core = f.core.clone();
    let id = timeout(Duration::from_secs(5), async {
        loop {
            if let Some(p) = core.approvals().await.unwrap().pop() {
                return p.id;
            }
            sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("pending approval");
    sleep(wait).await;
    f.core.decide(&id, true).await.unwrap();
}

#[tokio::test]
async fn mutation_meta_reports_upstream_metrics_and_separates_approval_wait() {
    let f = fixture(1).await;
    f.core
        .set_mode(&f.envs[0], WriteMode::Approval)
        .await
        .unwrap();
    let human = Duration::from_millis(300);
    let mut report = Vec::new();
    let mut check = |round: &str, out: &ToolOutput, logins: usize, bytes: usize, approved: bool| {
        let meta = &out.meta;
        assert_eq!(meta["upstream_calls"], json!(1 + logins), "{round}");
        assert_eq!(meta["upstream_bytes"], json!(bytes), "{round}");
        let upstream_ms = meta["upstream_ms"].as_u64().expect("upstream_ms");
        let wait_ms = meta["approval_wait_ms"].as_u64().expect("approval_wait_ms");
        if approved {
            assert!(
                wait_ms >= human.as_millis() as u64,
                "{round}: approval wait {wait_ms}"
            );
            assert!(
                upstream_ms < human.as_millis() as u64,
                "{round}: execution {upstream_ms}"
            );
        } else {
            assert_eq!(wait_ms, 0, "{round}");
        }
        report.push(format!(
            "{round}: upstream_calls={} upstream_bytes={} upstream_ms={upstream_ms} \
             approval_wait_ms={wait_ms}",
            meta["upstream_calls"], meta["upstream_bytes"],
        ));
    };
    let cases = [
        (
            "first approved write with login",
            "write",
            json!({"env":"E0","model":"res.partner","ids":[1],"values":{"name":"x"}}),
        ),
        (
            "approved unlink",
            "unlink",
            json!({"env":"E0","model":"res.partner","ids":[1]}),
        ),
        (
            "approved call action_confirm",
            "call",
            json!({"env":"E0","model":"sale.order","method":"action_confirm","args":[[1]]}),
        ),
    ];
    for (round, tool, args) in cases {
        let (bytes, logins) = (f.sim.counts.response_bytes(), f.sim.counts.logins());
        let call = spawn(&f, tool, args);
        approve_after(&f, human).await;
        let out = call.await.unwrap().unwrap();
        check(
            round,
            &out,
            f.sim.counts.logins() - logins,
            f.sim.counts.response_bytes() - bytes,
            true,
        );
    }
    f.core
        .set_mode(&f.envs[0], WriteMode::Permanent)
        .await
        .unwrap();
    for (round, tool, args) in [
        (
            "free-write create",
            "create",
            json!({"env":"E0","model":"res.partner","values":{"name":"x"}}),
        ),
        (
            "free-write call action_confirm",
            "call",
            json!({"env":"E0","model":"sale.order","method":"action_confirm","args":[[1]]}),
        ),
    ] {
        let (bytes, logins) = (f.sim.counts.response_bytes(), f.sim.counts.logins());
        let out = f.core.run_tool(&f.client, tool, args).await.unwrap();
        check(
            round,
            &out,
            f.sim.counts.logins() - logins,
            f.sim.counts.response_bytes() - bytes,
            false,
        );
    }
    assert!(report[0].starts_with("first approved write with login: upstream_calls=2"));
    // Known-read call uses the read path with identical metrics.
    let out = f
        .core
        .run_tool(
            &f.client,
            "call",
            json!({"env":"E0","model":"res.partner","method":"search_count","args":[[]]}),
        )
        .await
        .unwrap();
    out.meta["upstream_ms"]
        .as_u64()
        .expect("upstream_ms for a read call");
    assert!(!out.meta.contains_key("approval_wait_ms"));
    eprintln!("{}", report.join("\n"));
}

// Settings

fn settings(f: impl FnOnce(&mut odx_core::Settings)) -> odx_core::Settings {
    let mut s = odx_core::Settings::default();
    f(&mut s);
    s
}

#[tokio::test]
async fn timeout_setting_shortens_the_operation_deadline_without_restart() {
    let f = fixture(1).await;
    f.core
        .run_tool(&f.client, "search_read", read(0))
        .await
        .unwrap();
    f.core
        .save_settings(settings(|s| s.upstream_timeout_secs = 10))
        .await
        .unwrap();
    f.sim.counts.hold_database("db-0");
    let started = Instant::now();
    let call = spawn(&f, "search_read", read(0));
    until("read held in Odoo", || f.sim.counts.in_flight("db-0") == 1).await;
    tokio::time::pause();
    let err = call.await.unwrap().unwrap_err();
    let elapsed = started.elapsed();
    tokio::time::resume();
    assert_eq!(
        (err.code, err.outcome),
        ("upstream_timeout", Outcome::Failed)
    );
    assert!(
        elapsed >= Duration::from_secs(10) && elapsed < Duration::from_secs(12),
        "configured deadline: {elapsed:?}"
    );
    f.sim.counts.release_databases();
}

#[tokio::test]
async fn concurrency_setting_caps_calls_in_flight_without_restart() {
    let f = fixture(1).await;
    f.core
        .save_settings(settings(|s| s.environment_concurrency = 1))
        .await
        .unwrap();
    f.sim.counts.hold_database("db-0");
    let calls: Vec<Call> = (0..3).map(|_| spawn(&f, "search_read", read(0))).collect();
    until("one in-flight read", || f.sim.counts.in_flight("db-0") == 1).await;
    settle().await;
    assert_eq!(
        f.sim.counts.in_flight("db-0"),
        1,
        "other calls await admission"
    );
    f.sim.counts.release_databases();
    for call in calls {
        assert!(call.await.unwrap().is_ok());
    }
    assert_eq!(f.sim.counts.max_in_flight("db-0"), 1);
}

#[tokio::test]
async fn cancelled_waiters_leave_the_queues() {
    // One global slot: the waiter queues globally. Four global slots, one per environment: it queues per environment.
    for (global, queue) in [(1, "global"), (4, "environment")] {
        let limits = Limits::with_caps(global, 1, 2, 2, 5);
        let held = limits.acquire("busy").await.unwrap();
        let waiting = {
            let limits = limits.clone();
            tokio::spawn(async move { limits.acquire("busy").await })
        };
        wait_for(
            || {
                let depth = limits.queue_depth("busy").unwrap();
                depth.global + depth.environment
            },
            1,
        )
        .await;
        waiting.abort();
        let _ = waiting.await;
        let depth = limits.queue_depth("busy").unwrap();
        assert_eq!((depth.global, depth.environment), (0, 0), "{queue} queue");
        drop(held);
    }
}

#[tokio::test]
async fn a_write_waiting_for_a_slot_does_not_log_in_after_lock() {
    let f = fixture(1).await;
    f.core
        .save_settings(settings(|s| s.environment_concurrency = 1))
        .await
        .unwrap();
    f.core
        .set_mode(&f.envs[0], WriteMode::Permanent)
        .await
        .unwrap();
    f.sim.counts.hold_database("db-0");
    let held = spawn(&f, "search_read", read(0));
    until("read holds the only slot", || {
        f.sim.counts.in_flight("db-0") == 1
    })
    .await;
    let write = spawn(
        &f,
        "write",
        json!({"env":"E0","model":"res.partner","ids":[1],"values":{"name":"x"}}),
    );
    settle().await;
    let logins = f.sim.counts.logins();
    f.core.lock().await;
    f.sim.counts.release_databases();
    let _ = held.await;
    let err = write.await.unwrap().unwrap_err();
    assert_eq!(err.outcome, Outcome::NotSent, "{err:?}");
    assert_eq!(
        f.sim.counts.logins(),
        logins,
        "no login with the vault locked"
    );
}

#[tokio::test]
async fn a_read_whose_environment_changes_while_queued_closes_its_attempt() {
    let f = fixture(1).await;
    f.core
        .save_settings(settings(|s| s.environment_concurrency = 1))
        .await
        .unwrap();
    f.sim.counts.hold_database("db-0");
    let held = spawn(&f, "search_read", read(0));
    until("read holds the only slot", || {
        f.sim.counts.in_flight("db-0") == 1
    })
    .await;
    let queued = spawn(&f, "search_read", read(0));
    settle().await;
    flow::rename(&f.core, &f.envs[0], "E0b").await;
    f.sim.counts.release_databases();
    let _ = held.await;
    let err = queued.await.unwrap().unwrap_err();
    assert_eq!(err.outcome, Outcome::NotSent, "{err:?}");
    let events = f
        .core
        .activity(None, odx_core::ActivityFilter::default())
        .await
        .unwrap()
        .events;
    assert!(events.iter().any(|e| e.outcome == "not_sent"));
    assert!(
        events.iter().all(|e| e.outcome != "attempted"),
        "{events:?}"
    );
}
