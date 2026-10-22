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

#[tokio::test]
async fn draft_check_and_database_list_persist_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let sim = support::simulator().await;
    let core = Core::new(tmp.path().join("vault.sqlite3")).unwrap();
    core.create("fictional master password").await.unwrap();

    assert_eq!(
        core.list_databases(&format!("{}/", sim.url)).await.unwrap(),
        ["base-a", "base-b"]
    );
    assert!(core.list_databases("ftp://example.test").await.is_err());
    // Servers without /jsonrpc answer over XML-RPC.
    let xml_only = support::protocol_simulator_with_status(17, Some(404), None).await;
    assert_eq!(
        core.list_databases(&xml_only.url).await.unwrap(),
        ["base-a", "base-b"]
    );
    assert!(xml_only.requests.lock().unwrap()[0].contains("<methodName>list</methodName>"));
    let report = core
        .check_draft(draft("Draft", &sim.url, "fixture-secret"))
        .await
        .unwrap();
    assert_eq!(report.status, "ok");
    let report = core
        .check_draft(draft("Draft", &sim.url, "wrong-fixture-secret"))
        .await
        .unwrap();
    assert_eq!(report.status, "auth_failed");
    assert!(core.environments().await.unwrap().is_empty());
    assert_eq!(sim.counts.queries(), 0);
}

#[tokio::test]
async fn tags_are_normalized_and_persisted() {
    let tmp = tempfile::tempdir().unwrap();
    let sim = support::simulator().await;
    let core = Core::new(tmp.path().join("vault.sqlite3")).unwrap();
    core.create("fictional master password").await.unwrap();
    let env = core
        .save_environment(draft("Tagged", &sim.url, "fixture-secret"))
        .await
        .unwrap();
    let tags = [" Cliente A ", "prod", "cliente a", ""].map(String::from);
    let view = core.set_environment_tags(&env.id, &tags).await.unwrap();
    assert_eq!(view.tags, ["Cliente A", "prod"]);
    assert!(
        core.set_environment_tags(&env.id, &["x".repeat(33)])
            .await
            .is_err()
    );
    assert!(
        core.set_environment_tags(&env.id, &(0..21).map(|i| i.to_string()).collect::<Vec<_>>())
            .await
            .is_err()
    );
    assert_eq!(
        core.environments().await.unwrap()[0].tags,
        ["Cliente A", "prod"]
    );
    // Connection edits keep tags.
    let updated = core
        .update_environment(&env.id, draft("Renamed", &sim.url, ""))
        .await
        .unwrap();
    assert_eq!(updated.tags, ["Cliente A", "prod"]);
    // Colors are per tag name, ignoring case.
    core.set_tag_color("cliente a", Some("danger"))
        .await
        .unwrap();
    core.set_tag_color("prod", Some("info")).await.unwrap();
    assert!(core.set_tag_color("prod", Some("pink")).await.is_err());
    core.set_tag_color("prod", None).await.unwrap();
    // Tags survive locking and reopening the vault, and so do their colors.
    core.lock().await;
    core.unlock("fictional master password").await.unwrap();
    assert_eq!(
        core.environments().await.unwrap()[0].tags,
        ["Cliente A", "prod"]
    );
    let colors = core.tag_colors().await.unwrap();
    assert_eq!(
        colors.into_iter().collect::<Vec<_>>(),
        [("cliente a".to_owned(), "danger".to_owned())]
    );
    // Colors of tags no environment uses are dropped on the next change.
    core.set_environment_tags(&env.id, &["prod".to_owned()])
        .await
        .unwrap();
    core.set_tag_color("prod", Some("ok")).await.unwrap();
    assert_eq!(
        core.tag_colors().await.unwrap().keys().collect::<Vec<_>>(),
        ["prod"]
    );
}
