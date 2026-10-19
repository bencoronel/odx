//! XML-RPC/JSON-2 against simulated Odoo 16–19: authentication, errors, types, limits, redaction, uncertain writes and shared policy across protocols.
mod support;
use odx_core::{
    Capability, ClientIdentity, Core, EnvironmentDraft, Protocol, ToolError, ToolOutput, WriteMode,
    headers::HeaderEntry,
};
use serde_json::{Value, json};
use std::{sync::Arc, sync::atomic::Ordering};

const PROTOCOLS: [Protocol; 3] = [Protocol::LegacyJsonrpc, Protocol::Xmlrpc, Protocol::Json2];

struct Fixture {
    core: Arc<Core>,
    sim: support::ProtocolSimulator,
    env: String,
    client: ClientIdentity,
    _tmp: tempfile::TempDir,
}

async fn fixture(major: u8, protocol: Protocol, secret: &str) -> Fixture {
    fixture_with_sim(support::protocol_simulator(major).await, protocol, secret).await
}

async fn fixture_with_sim(
    sim: support::ProtocolSimulator,
    protocol: Protocol,
    secret: &str,
) -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let core = Core::new(tmp.path().join("vault.sqlite3")).unwrap();
    core.create("fictional master password").await.unwrap();
    let env = core
        .save_environment(EnvironmentDraft {
            name: "A".into(),
            url: sim.url.clone(),
            database: "base-a".into(),
            login: "fixture-user".into(),
            protocol,
            context: json!({"lang": "es_ES"}),
            secret: secret.into(),
        })
        .await
        .unwrap()
        .id;
    let issued = core
        .create_client(
            "agent",
            vec![env.clone()],
            vec![Capability::OdooRead, Capability::OdooWriteRequest],
        )
        .await
        .unwrap();
    let client = core.authenticate_client(&issued.token).await.unwrap();
    Fixture {
        core,
        sim,
        env,
        client,
        _tmp: tmp,
    }
}

impl Fixture {
    async fn run(&self, tool: &str, args: Value) -> Result<ToolOutput, ToolError> {
        let mut args = args;
        args["env"] = json!("A");
        self.core.run_tool(&self.client, tool, args).await
    }
    fn upstream_requests(&self) -> usize {
        self.sim.requests.lock().unwrap().len()
    }
}

fn calls(out: &ToolOutput) -> u64 {
    out.meta["upstream_calls"].as_u64().unwrap()
}

#[tokio::test]
async fn every_protocol_and_detection_send_default_or_configured_user_agent() {
    use axum::{Router, body::Body, http::Request, response::IntoResponse};
    let app = Router::new().fallback(|request: Request<Body>| async move {
        let path = request.uri().path().to_owned();
        let expected = if path.starts_with("/custom/") {
            "fixture-local-agent"
        } else {
            concat!("Odx/", env!("CARGO_PKG_VERSION"))
        };
        assert_eq!(request.headers()["user-agent"], expected);
        let body = axum::body::to_bytes(request.into_body(), 4096).await.unwrap();
        let login = String::from_utf8_lossy(&body).contains("login");
        if path.ends_with("/web/version") {
            axum::Json(json!({"version": "19.0"})).into_response()
        } else if path.ends_with("/context_get") {
            axum::Json(json!({"uid": 11})).into_response()
        } else if path.contains("/xmlrpc/") && login {
            "<methodResponse><params><param><value><int>11</int></value></param></params></methodResponse>".into_response()
        } else if path.contains("/xmlrpc/") {
            "<methodResponse><params><param><value><struct><member><name>server_version</name><value><string>19.0</string></value></member></struct></value></param></params></methodResponse>".into_response()
        } else if login {
            axum::Json(json!({"result": 11})).into_response()
        } else {
            axum::Json(json!({"result": {"server_version": "19.0"}})).into_response()
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    for protocol in PROTOCOLS.into_iter().chain([Protocol::Auto]) {
        let tmp = tempfile::tempdir().unwrap();
        let core = Core::new(tmp.path().join("vault.sqlite3")).unwrap();
        core.create("fictional master password").await.unwrap();
        let mut draft = EnvironmentDraft {
            name: "A".into(),
            url: format!("{url}/default"),
            database: "base-a".into(),
            login: "fixture-user".into(),
            secret: "fixture-secret".into(),
            protocol,
            context: json!({}),
        };
        let env = core.save_environment(draft.clone()).await.unwrap();
        assert_eq!(
            core.check_environment(&env.id).await.unwrap().status,
            "ok",
            "{protocol:?}"
        );
        core.set_headers(
            None,
            vec![HeaderEntry {
                name: "User-Agent".into(),
                value: "fixture-global-agent".into(),
                enabled: true,
            }],
        )
        .await
        .unwrap();
        core.set_headers(
            Some(&env.id),
            vec![HeaderEntry {
                name: "user-AGENT".into(),
                value: "fixture-local-agent".into(),
                enabled: true,
            }],
        )
        .await
        .unwrap();
        draft.url = format!("{url}/custom");
        core.update_environment(&env.id, draft).await.unwrap();
        assert_eq!(
            core.check_environment(&env.id).await.unwrap().status,
            "ok",
            "{protocol:?}"
        );
    }
    task.abort();
}

#[tokio::test]
async fn automatic_protocol_is_detected_and_persisted_without_business_queries() {
    for major in 16..=19 {
        let f = fixture(major, Protocol::Auto, "fixture-secret").await;
        let expected = if major >= 19 {
            Protocol::Json2
        } else {
            Protocol::LegacyJsonrpc
        };
        let report = f.core.check_environment(&f.env).await.unwrap();
        assert_eq!(report.status, "ok", "{report:?}");
        assert_eq!(report.protocol, Some(expected));
        assert_eq!(f.sim.counts.queries() + f.sim.counts.writes(), 0);
        let env = f.core.environments().await.unwrap().remove(0);
        assert_eq!(env.protocol, Protocol::Auto);
        assert_eq!(env.detected_protocol, Some(expected));
        let listed = f.core.run_tool(&f.client, "envs", json!({})).await.unwrap();
        assert_eq!(
            listed.data[0]["protocol"],
            serde_json::to_value(expected).unwrap()
        );
        let requests = f.upstream_requests();
        let logins = f.sim.counts.logins();
        f.core.check_environment(&f.env).await.unwrap();
        assert_eq!(f.upstream_requests(), requests, "do not repeat discovery");
        assert_eq!(f.sim.counts.logins(), logins);

        f.core.lock().await;
        f.core.unlock("fictional master password").await.unwrap();
        assert_eq!(
            f.core.environments().await.unwrap()[0].detected_protocol,
            Some(expected)
        );
        assert_eq!(
            f.core.check_environment(&f.env).await.unwrap().protocol,
            Some(expected)
        );
    }
}

#[tokio::test]
async fn first_automatic_call_resolves_before_validating_named_arguments() {
    let f = fixture(19, Protocol::Auto, "fixture-secret").await;
    let error = f
        .run(
            "call",
            json!({"model":"res.partner","method":"read","args":[[1]]}),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "invalid_arguments");
    assert!(error.message.contains("named arguments"));
    assert_eq!(f.sim.counts.queries(), 0);
    let read = f.run("call", json!({"model":"res.partner","method":"read","kwargs":{"ids":[1],"fields":["display_name"]}})).await.unwrap();
    assert_eq!(read.data, json!([{"id":1,"display_name":"record-1"}]));
}

#[tokio::test]
async fn forbidden_credentials_do_not_even_trigger_automatic_detection() {
    let f = fixture(19, Protocol::Auto, "fixture-secret").await;
    let error = f
        .run(
            "call",
            json!({"model":"res.users.apikeys.show","method":"read","kwargs":{"ids":[1]}}),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "forbidden");
    assert_eq!(f.sim.bytes.load(Ordering::SeqCst), 0);
    assert_eq!(
        f.upstream_requests() + f.sim.counts.logins() + f.sim.counts.queries(),
        0
    );
}

#[tokio::test]
async fn automatic_falls_back_only_for_missing_endpoints_or_verified_legacy_credentials() {
    for (major, rpc_status, json2_status, expected) in [
        (18, Some(404), None, Protocol::Xmlrpc),
        (19, None, Some(404), Protocol::LegacyJsonrpc),
        (19, Some(405), Some(404), Protocol::Xmlrpc),
        // A valid RPC password is not necessarily a JSON-2 API key.
        (19, None, Some(401), Protocol::LegacyJsonrpc),
    ] {
        let sim = support::protocol_simulator_with_status(major, rpc_status, json2_status).await;
        let f = fixture_with_sim(sim, Protocol::Auto, "fixture-secret").await;
        let report = f.core.check_environment(&f.env).await.unwrap();
        assert_eq!(
            (report.status.as_str(), report.protocol),
            ("ok", Some(expected)),
            "{report:?}"
        );
        assert_eq!(f.sim.counts.queries() + f.sim.counts.writes(), 0);
        assert!(
            f.run(
                "search_read",
                json!({"model":"res.partner","fields":["display_name"]})
            )
            .await
            .is_ok()
        );
    }
}

#[tokio::test]
async fn automatic_treats_a_redirected_version_probe_as_missing_json2_only() {
    for status in [302, 303] {
        let sim =
            support::protocol_simulator_with_version_status(16, None, None, Some(status)).await;
        let f = fixture_with_sim(sim, Protocol::Auto, "fixture-secret").await;
        let report = f.core.check_environment(&f.env).await.unwrap();
        assert_eq!(
            (report.status.as_str(), report.protocol),
            ("ok", Some(Protocol::LegacyJsonrpc)),
            "{report:?}"
        );
    }
    // RPC endpoints keep rejecting redirects: they may hide a wrong URL.
    let sim = support::protocol_simulator_with_version_status(16, Some(303), None, Some(303)).await;
    let f = fixture_with_sim(sim, Protocol::Auto, "fixture-secret").await;
    let report = f.core.check_environment(&f.env).await.unwrap();
    assert_eq!(report.http_status, Some(303), "{report:?}");
    assert_eq!(f.sim.counts.logins(), 0);
    assert_eq!(
        f.core.environments().await.unwrap()[0].detected_protocol,
        None
    );
}

#[tokio::test]
async fn automatic_does_not_mask_bad_credentials_permissions_or_server_errors() {
    let f = fixture(19, Protocol::Auto, "invalid-fixture-secret").await;
    let report = f.core.check_environment(&f.env).await.unwrap();
    assert_eq!(report.status, "auth_failed", "{report:?}");
    assert_eq!(report.error_code.as_deref(), Some("upstream.auth_rejected"));
    assert!(!format!("{report:?}").contains("invalid-fixture-secret"));
    assert_eq!(
        f.core.environments().await.unwrap()[0].detected_protocol,
        None
    );
    assert_eq!(f.sim.counts.queries() + f.sim.counts.writes(), 0);
    for status in [403, 429, 500, 503] {
        let sim = support::protocol_simulator_with_status(19, None, Some(status)).await;
        let f = fixture_with_sim(sim, Protocol::Auto, "fixture-secret").await;
        let report = f.core.check_environment(&f.env).await.unwrap();
        assert_eq!(report.status, "unreachable", "{report:?}");
        assert_eq!(report.http_status, Some(status));
        assert_eq!(
            f.sim.counts.logins(),
            0,
            "do not fall back to RPC after HTTP {status}"
        );
        assert_eq!(
            f.core.environments().await.unwrap()[0].detected_protocol,
            None
        );
    }
}

#[tokio::test]
async fn concurrent_automatic_checks_share_detection_and_stop_after_lock() {
    let f = fixture(18, Protocol::Auto, "fixture-secret").await;
    f.sim.counts.pause_next_login();
    let mut tasks = Vec::new();
    for _ in 0..3 {
        let (core, id) = (f.core.clone(), f.env.clone());
        tasks.push(tokio::spawn(
            async move { core.check_environment(&id).await },
        ));
    }
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        f.sim.counts.wait_for_login(),
    )
    .await
    .unwrap();
    f.sim.counts.release_login();
    for task in tasks {
        let report = tokio::time::timeout(std::time::Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(report.status, "ok");
    }
    assert_eq!(f.sim.counts.logins(), 1);
    // Connection changes clear cached protocol selection and authentication.
    f.core
        .update_environment(
            &f.env,
            EnvironmentDraft {
                name: "A".into(),
                url: f.sim.url.clone(),
                database: "base-a".into(),
                login: "fixture-user".into(),
                protocol: Protocol::Auto,
                context: json!({}),
                secret: "".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        f.core.environments().await.unwrap()[0].detected_protocol,
        None
    );
    f.sim.counts.pause_next_login();
    let (core, id) = (f.core.clone(), f.env.clone());
    let task = tokio::spawn(async move { core.check_environment(&id).await });
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        f.sim.counts.wait_for_login(),
    )
    .await
    .unwrap();
    f.core.lock().await;
    f.sim.counts.release_login();
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    f.core.unlock("fictional master password").await.unwrap();
    assert_eq!(
        f.core.environments().await.unwrap()[0].detected_protocol,
        None
    );
}

#[tokio::test]
async fn automatic_keeps_write_policy_and_does_not_retry_uncertain_writes() {
    let f = fixture(19, Protocol::Auto, "fixture-secret").await;
    assert_eq!(
        f.run(
            "write",
            json!({"model":"res.partner","ids":[1],"values":{"name":"blocked"}})
        )
        .await
        .unwrap_err()
        .code,
        "forbidden"
    );
    assert_eq!(f.sim.counts.writes(), 0);
    f.core.set_mode(&f.env, WriteMode::Approval).await.unwrap();
    let (core, client) = (f.core.clone(), f.client.clone());
    let task = tokio::spawn(async move {
        core.run_tool(
            &client,
            "write",
            json!({"env":"A","model":"res.partner","ids":[1],"values":{"name":"approved"}}),
        )
        .await
    });
    let pending = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if let Some(p) = f.core.approvals().await.unwrap().into_iter().next() {
                break p;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(f.sim.counts.writes(), 0);
    f.core.decide(&pending.id, true).await.unwrap();
    assert!(task.await.unwrap().is_ok());
    assert_eq!(f.sim.counts.writes(), 1);
    f.core.set_mode(&f.env, WriteMode::Permanent).await.unwrap();
    f.sim.counts.lose_next_response();
    let error = f
        .run(
            "write",
            json!({"model":"res.partner","ids":[1],"values":{"name":"uncertain"}}),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "uncertain_result");
    assert_eq!(f.sim.counts.writes(), 2, "send the write only once");
    assert_eq!(
        f.core.environments().await.unwrap()[0].detected_protocol,
        Some(Protocol::Json2)
    );
}

#[tokio::test]
async fn version_matrix_16_to_19() {
    for major in 16..=19u8 {
        for protocol in PROTOCOLS {
            let f = fixture(major, protocol, "fixture-secret").await;
            let report = f.core.check_environment(&f.env).await.unwrap();
            let read = f
                .run(
                    "search_read",
                    json!({"model":"res.partner","fields":["display_name"]}),
                )
                .await;
            if protocol == Protocol::Json2 && major < 19 {
                // Reject JSON-2 before Odoo 19 explicitly.
                assert_eq!(report.status, "unreachable", "{major} {protocol:?}");
                let error = read.unwrap_err();
                assert!(error.message.contains("JSON-2"), "{}", error.message);
                assert!(error.message.contains("Odoo 19"), "{}", error.message);
                assert_eq!(f.sim.counts.queries() + f.sim.counts.logins(), 0);
            } else {
                assert_eq!(report.status, "ok", "{major} {protocol:?} {report:?}");
                assert_eq!(report.server_version, Some(format!("{major}.0")));
                let rows = read.unwrap().data;
                assert_eq!(
                    rows[0]["display_name"], "contact-base-a",
                    "{major} {protocol:?}"
                );
                // Preserve environment context across all protocols.
                assert_eq!(rows[0]["context"]["lang"], "es_ES");
            }
        }
    }
}

#[tokio::test]
async fn authentication_per_protocol_and_cached_identity() {
    for protocol in PROTOCOLS {
        let bad = fixture(19, protocol, "wrong-secret").await;
        let error = bad.run("whoami", json!({})).await.unwrap_err();
        assert_eq!(error.code, "upstream_auth", "{protocol:?} {error:?}");
        assert!(!format!("{error:?}").contains("wrong-secret"));
        // Rejected credentials prevent the query from being sent.
        assert_eq!(bad.sim.counts.queries(), 0, "{protocol:?}");
        let report = bad.core.check_environment(&bad.env).await.unwrap();
        assert_eq!(report.status, "auth_failed", "{protocol:?}");

        let f = fixture(19, protocol, "fixture-secret").await;
        let first = f.run("whoami", json!({})).await.unwrap();
        assert_eq!(first.data["uid"], 11, "{protocol:?}");
        assert_eq!(first.data["login"], "fixture-user");
        // First call: login or JSON-2 context_get, then the read.
        assert_eq!(calls(&first), 2, "{protocol:?}");
        let second = f.run("whoami", json!({})).await.unwrap();
        assert_eq!(calls(&second), 1, "{protocol:?}");
        // JSON-2 has no common.login; send the Bearer key on every call.
        let logins = if protocol == Protocol::Json2 { 0 } else { 1 };
        assert_eq!(f.sim.counts.logins(), logins, "{protocol:?}");
        if protocol != Protocol::LegacyJsonrpc {
            // Measured bytes equal simulator response body bytes.
            let measured = first.meta["upstream_bytes"].as_u64().unwrap()
                + second.meta["upstream_bytes"].as_u64().unwrap();
            assert_eq!(measured as usize, f.sim.bytes.load(Ordering::SeqCst));
        }
    }
}

#[tokio::test]
async fn same_tool_results_on_every_protocol() {
    let mut results: Vec<Vec<Value>> = vec![];
    for protocol in PROTOCOLS {
        let f = fixture(19, protocol, "fixture-secret").await;
        f.core.set_mode(&f.env, WriteMode::Permanent).await.unwrap();
        let mut row = vec![];
        for (tool, args) in [
            (
                "read",
                json!({"model":"res.partner","ids":[1,9],"fields":["display_name"]}),
            ),
            ("search", json!({"model":"res.partner","limit":2})),
            ("search_count", json!({"model":"res.partner"})),
            ("fields", json!({"model":"res.partner","q":"mail"})),
            ("models", json!({"q":"res."})),
            (
                "name_search",
                json!({"model":"res.partner","name":"Al","limit":2}),
            ),
            (
                "create",
                json!({"model":"res.partner","values":{"name":"x"}}),
            ),
            (
                "create",
                json!({"model":"res.partner","values":[{"name":"x"},{"name":"y"}]}),
            ),
            (
                "write",
                json!({"model":"res.partner","ids":[1],"values":{"name":"y"}}),
            ),
        ] {
            let out = f.run(tool, args).await.unwrap();
            row.push(json!({"data": out.data, "page": out.meta.get("page"),
                            "missing": out.meta.get("missing_ids")}));
        }
        assert_eq!(f.sim.counts.writes(), 3, "{protocol:?}");
        results.push(row);
    }
    assert_eq!(results[0], results[1], "XML-RPC differs from JSON-RPC");
    assert_eq!(results[0], results[2], "JSON-2 differs from JSON-RPC");
}

#[tokio::test]
async fn xmlrpc_cannot_return_none_but_json_protocols_can() {
    // Odoo XML-RPC allow_none=False faults on None or integers outside 32 bits.
    for protocol in PROTOCOLS {
        let f = fixture(19, protocol, "fixture-secret").await;
        let out = f
            .run("search_read", json!({"model":"x.types","fields":["name"]}))
            .await;
        if protocol == Protocol::Xmlrpc {
            let error = out.unwrap_err();
            assert_eq!(error.code, "upstream_error");
            assert!(
                error.message.contains("cannot marshal None"),
                "{}",
                error.message
            );
        } else {
            let data = out.unwrap().data;
            assert_eq!(data[0]["none"], Value::Null, "{protocol:?}");
            assert_eq!(data[0]["flag"], false);
        }
    }
}

#[tokio::test]
async fn logical_errors_are_errors_and_never_leak_the_secret() {
    for protocol in [Protocol::Xmlrpc, Protocol::Json2] {
        let f = fixture(19, protocol, "fixture-secret").await;
        // AccessError is XML-RPC fault 4 with HTTP 200, or JSON-2 HTTP 403.
        f.sim
            .counts
            .fail_next_execute("Access denied to fixture-secret");
        let error = f
            .run("search_read", json!({"model":"res.partner"}))
            .await
            .unwrap_err();
        assert_eq!(error.code, "upstream_error", "{protocol:?}");
        assert!(
            error.message.contains("Access denied to [redacted]"),
            "{}",
            error.message
        );
        assert!(!format!("{error:?}").contains("fixture-secret"));
        if protocol == Protocol::Json2 {
            assert!(error.message.contains("HTTP 403"), "{}", error.message);
        }
        // Unexpected XML-RPC exceptions return faultCode 1 tracebacks; display the last line.
        let error = f
            .run("search_read", json!({"model":"x.unserializable"}))
            .await
            .unwrap_err();
        assert!(
            error
                .message
                .contains("Object of type bytes is not JSON serializable"),
            "{}",
            error.message
        );
        assert!(!error.message.contains("Traceback"), "{}", error.message);
    }
    let f = fixture(19, Protocol::Json2, "fixture-secret").await;
    // Odoo 404 for missing models; reject unknown kwargs before dispatch.
    let error = f
        .run("search_read", json!({"model":"missing.model"}))
        .await
        .unwrap_err();
    assert!(
        error.message.contains("does not exist (HTTP 404)"),
        "{}",
        error.message
    );
    let requests_before_invalid_kwargs = f.upstream_requests();
    let error = f
        .run(
            "call",
            json!({"model":"res.partner","method":"search_read","kwargs":{"bogus":1}}),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "invalid_arguments");
    assert_eq!(error.message, "unknown field: bogus");
    assert_eq!(f.upstream_requests(), requests_before_invalid_kwargs);
    // Send the key in headers only, never the body.
    assert!(
        f.sim
            .requests
            .lock()
            .unwrap()
            .iter()
            .all(|b| !b.contains("fixture-secret"))
    );
}

#[tokio::test]
async fn responses_over_16_mib_are_rejected() {
    for protocol in PROTOCOLS {
        let f = fixture(19, protocol, "fixture-secret").await;
        let error = f
            .run("search_read", json!({"model":"limits.huge"}))
            .await
            .unwrap_err();
        assert_eq!(error.code, "output_too_large", "{protocol:?}");
        assert!(
            error.message.contains("16 MiB"),
            "{protocol:?} {}",
            error.message
        );
    }
}

#[tokio::test]
async fn uncertain_writes_are_not_repeated_nor_retried_on_another_protocol() {
    for protocol in [Protocol::Xmlrpc, Protocol::Json2] {
        let f = fixture(19, protocol, "fixture-secret").await;
        f.core.set_mode(&f.env, WriteMode::Permanent).await.unwrap();
        f.run("whoami", json!({})).await.unwrap();
        let before = f.upstream_requests();
        f.sim.counts.lose_next_response();
        let error = f
            .run(
                "write",
                json!({"model":"res.partner","ids":[1],"values":{"name":"x"}}),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, "uncertain_result", "{protocol:?} {error:?}");
        assert_eq!(f.sim.counts.writes(), 1);
        // Dispatch once through the configured protocol; no retry or JSON-RPC fallback.
        assert_eq!(f.upstream_requests(), before + 1);
        assert_eq!(
            f.sim.counts.logins(),
            if protocol == Protocol::Xmlrpc { 1 } else { 0 }
        );
    }
}

#[tokio::test]
async fn json2_call_needs_named_arguments() {
    let f = fixture(19, Protocol::Json2, "fixture-secret").await;
    f.core.set_mode(&f.env, WriteMode::Permanent).await.unwrap();
    for (method, args) in [("read", json!([[1]])), ("action_confirm", json!([[1]]))] {
        let error = f
            .run(
                "call",
                json!({"model":"res.partner","method":method,"args":args}),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, "invalid_arguments", "{method}");
        assert!(error.message.contains("JSON-2"), "{}", error.message);
        assert!(error.message.contains("kwargs"), "{}", error.message);
    }
    // Reject before authentication or dispatch.
    assert_eq!(f.upstream_requests(), 0);
    assert_eq!(f.sim.counts.writes(), 0);
    let rows = f
        .run(
            "call",
            json!({"model":"res.partner","method":"read","kwargs":{"ids":[1],"fields":["display_name"]}}),
        )
        .await
        .unwrap();
    assert_eq!(rows.data, json!([{"id":1,"display_name":"record-1"}]));
    let done = f
        .run(
            "call",
            json!({"model":"sale.order","method":"action_confirm","kwargs":{"ids":[1]}}),
        )
        .await
        .unwrap();
    assert_eq!(done.data, json!(true));
    assert_eq!(f.sim.counts.writes(), 1);

    // XML-RPC preserves execute_kw positional arguments.
    let x = fixture(19, Protocol::Xmlrpc, "fixture-secret").await;
    let rows = x
        .run(
            "call",
            json!({"model":"res.partner","method":"read","args":[[1]],"kwargs":{"fields":["display_name"]}}),
        )
        .await
        .unwrap();
    assert_eq!(rows.data, json!([{"id":1,"display_name":"record-1"}]));
}

#[tokio::test]
async fn policy_is_the_same_on_every_protocol() {
    for protocol in PROTOCOLS {
        let f = fixture(19, protocol, "fixture-secret").await;
        // Read-only calls are denied without dispatch.
        let error = f
            .run(
                "write",
                json!({"model":"res.partner","ids":[1],"values":{"name":"x"}}),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, "forbidden", "{protocol:?}");
        let error = f
            .run(
                "call",
                json!({"model":"sale.order","method":"action_confirm","kwargs":{"ids":[1]}}),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, "forbidden", "{protocol:?}");
        assert_eq!(f.sim.counts.writes(), 0);
        assert_eq!(f.upstream_requests(), 0);
    }
}

#[tokio::test]
async fn json2_owns_its_authorization_and_database_headers() {
    let f = fixture(19, Protocol::Json2, "fixture-secret").await;
    f.core
        .set_headers(
            Some(&f.env),
            vec![HeaderEntry {
                name: "Authorization".into(),
                value: "Basic fixture-proxy".into(),
                enabled: true,
            }],
        )
        .await
        .unwrap();
    let error = f.run("whoami", json!({})).await.unwrap_err();
    assert_eq!(error.code, "invalid_arguments");
    assert!(error.message.contains("Authorization"), "{}", error.message);
    assert_eq!(f.upstream_requests(), 0);
}

#[tokio::test]
async fn changing_secret_or_protocol_drops_the_cached_identity() {
    let f = fixture(19, Protocol::Xmlrpc, "fixture-secret").await;
    let draft = |protocol, secret: &str| EnvironmentDraft {
        name: "A".into(),
        url: f.sim.url.clone(),
        database: "base-a".into(),
        login: "fixture-user".into(),
        protocol,
        context: json!({"lang": "es_ES"}),
        secret: secret.into(),
    };
    assert_eq!(calls(&f.run("whoami", json!({})).await.unwrap()), 2);
    // Switching XML-RPC→JSON-2 resolves identity through context_get without login.
    f.core
        .update_environment(&f.env, draft(Protocol::Json2, ""))
        .await
        .unwrap();
    let out = f.run("whoami", json!({})).await.unwrap();
    assert_eq!(calls(&out), 2);
    assert_eq!(f.sim.counts.logins(), 1);
    // After key replacement, never send the old key.
    f.core
        .update_environment(&f.env, draft(Protocol::Json2, "rotated-secret"))
        .await
        .unwrap();
    let error = f.run("whoami", json!({})).await.unwrap_err();
    assert_eq!(error.code, "upstream_auth", "{error:?}");
    assert!(!format!("{error:?}").contains("rotated-secret"));
    // Switching back to XML-RPC with a valid key requires a new login.
    f.core
        .update_environment(&f.env, draft(Protocol::Xmlrpc, "fixture-secret"))
        .await
        .unwrap();
    assert_eq!(calls(&f.run("whoami", json!({})).await.unwrap()), 2);
    assert_eq!(f.sim.counts.logins(), 2);
}

#[tokio::test]
async fn untrusted_tls_certificates_are_rejected_before_sending() {
    let (port, decrypted) = support::untrusted_tls_server().await;
    for protocol in PROTOCOLS {
        let tmp = tempfile::tempdir().unwrap();
        let core = Core::new(tmp.path().join("vault.sqlite3")).unwrap();
        core.create("fictional master password").await.unwrap();
        let env = core
            .save_environment(EnvironmentDraft {
                name: "A".into(),
                url: format!("https://localhost:{port}"),
                database: "base-a".into(),
                login: "fixture-user".into(),
                protocol,
                context: json!({}),
                secret: "fixture-secret".into(),
            })
            .await
            .unwrap();
        let report = core.check_environment(&env.id).await.unwrap();
        assert_eq!(report.status, "unreachable", "{protocol:?}");
        assert_eq!(
            report.network_error.as_deref(),
            Some("tls"),
            "{protocol:?} {report:?}"
        );
    }
    // No request bytes or key may cross a connection with an invalid certificate.
    assert_eq!(decrypted.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn unlink_needs_individual_approval_on_every_protocol() {
    for protocol in PROTOCOLS {
        let f = Arc::new(fixture(19, protocol, "fixture-secret").await);
        f.core.set_mode(&f.env, WriteMode::Permanent).await.unwrap();
        let runner = f.clone();
        let pending = tokio::spawn(async move {
            runner
                .run("unlink", json!({"model":"res.partner","ids":[1]}))
                .await
        });
        let mut approval = None;
        for _ in 0..200 {
            approval = f.core.approvals().await.unwrap().into_iter().next();
            if approval.is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let approval = approval.expect("pending approval did not appear");
        assert_eq!(approval.method, "unlink", "{protocol:?}");
        assert_eq!(f.sim.counts.writes(), 0, "nothing sent before approval");
        f.core.decide(&approval.id, true).await.unwrap();
        assert_eq!(pending.await.unwrap().unwrap().data, json!(true));
        assert_eq!(f.sim.counts.writes(), 1, "{protocol:?}");
    }
}
