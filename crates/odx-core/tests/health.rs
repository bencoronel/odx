mod support;
use odx_core::{Core, EnvironmentDraft, Protocol};
use serde_json::json;

fn draft(name: &str, url: &str, secret: &str) -> EnvironmentDraft {
    EnvironmentDraft {
        name: name.into(),
        url: url.into(),
        database: "base-a".into(),
        login: "fixture-user".into(),
        protocol: Protocol::LegacyJsonrpc,
        context: json!({}),
        secret: secret.into(),
    }
}

#[tokio::test]
async fn health_check_reports_reachability_and_credentials_without_leaking_secrets() {
    let tmp = tempfile::tempdir().unwrap();
    let sim = support::simulator().await;
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let closed_url = format!("http://{}", closed.local_addr().unwrap());
    drop(closed);
    let core = Core::new(tmp.path().join("vault.sqlite3")).unwrap();
    core.create("fictional master password").await.unwrap();
    let ok = core
        .save_environment(draft("Ok", &sim.url, "fixture-secret"))
        .await
        .unwrap();
    let bad = core
        .save_environment(draft("Bad", &sim.url, "wrong-fixture-secret"))
        .await
        .unwrap();
    let down = core
        .save_environment(draft("Down", &closed_url, "fixture-secret"))
        .await
        .unwrap();

    let report = core.check_environment(&ok.id).await.unwrap();
    assert_eq!(report.status, "ok");
    assert!(report.error.is_none());
    let logins = sim.counts.logins();
    assert_eq!(core.check_environment(&ok.id).await.unwrap().status, "ok");
    assert_eq!(sim.counts.logins(), logins, "reuse authentication");
    assert_eq!(sim.counts.queries(), 0, "do not read business data");
    assert_eq!(sim.counts.writes(), 0);

    let report = core.check_environment(&bad.id).await.unwrap();
    assert_eq!(report.status, "auth_failed");
    assert_eq!(report.error_code.as_deref(), Some("upstream.auth_rejected"));
    assert!(report.error.is_none());

    let down_report = core.check_environment(&down.id).await.unwrap();
    assert_eq!(down_report.status, "unreachable");
    assert_eq!(down_report.network_error.as_deref(), Some("refused"));
    assert_eq!(down_report.http_status, None);

    // Report HTTP 503 from an upstream proxy.
    let busy = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let busy_url = format!("http://{}", busy.local_addr().unwrap());
    tokio::spawn(async move {
        let app = axum::Router::new().route(
            "/jsonrpc",
            axum::routing::post(|| async { axum::http::StatusCode::SERVICE_UNAVAILABLE }),
        );
        axum::serve(busy, app).await.unwrap();
    });
    let unavailable = core
        .save_environment(draft("Busy", &busy_url, "fixture-secret"))
        .await
        .unwrap();
    let report = core.check_environment(&unavailable.id).await.unwrap();
    assert_eq!(report.status, "unreachable");
    assert_eq!(report.http_status, Some(503));
    assert_eq!(report.network_error, None);
    assert_eq!(
        core.check_environment(&ok.id).await.unwrap().http_status,
        None
    );

    core.lock().await;
    assert!(core.check_environment(&ok.id).await.is_err());
}
