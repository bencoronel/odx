use axum::{
    Router,
    extract::Query,
    http::HeaderMap,
    response::{Html, Redirect},
    routing::get,
};
use odx_core::{Core, EnvironmentDraft, Protocol, headers::HeaderEntry};
use serde_json::json;
use std::collections::HashMap;

fn assert_public_headers(headers: &HeaderMap) {
    assert_eq!(headers["user-agent"], "fixture-local-agent");
    assert_eq!(headers["accept-language"], "es-ES");
    assert_eq!(headers["x-request-id"], "fixture-inherited");
    for name in [
        "authorization",
        "cookie",
        "x-secret",
        "x-local-secret",
        "cache-control",
    ] {
        assert!(!headers.contains_key(name), "{name}");
    }
}

#[tokio::test]
async fn discovers_public_metadata_with_bounded_redirects_without_vault_credentials() {
    let app = Router::new()
        .route(
            "/",
            get(
                |Query(query): Query<HashMap<String, String>>, headers: HeaderMap| async move {
                    assert_eq!(query.get("db").map(String::as_str), Some("fixture-db"));
                    assert_public_headers(&headers);
                    Redirect::to("/web/login")
                },
            ),
        )
        .route(
            "/web/login",
            get(|headers: HeaderMap| async move {
                assert_public_headers(&headers);
                Html("<link rel=\"shortcut icon\" href=\"/web/image/website/2/favicon\">")
            }),
        )
        .route("/loop/", get(|| async { Redirect::to("/loop/") }))
        .route(
            "/icon",
            get(|headers: HeaderMap| async move {
                assert_public_headers(&headers);
                ([("content-type", "image/png")], vec![1_u8, 2, 3])
            }),
        )
        .route("/icon-redirect", get(|| async { Redirect::to("/icon") }))
        .route(
            "/icon-elsewhere",
            get(|Query(query): Query<HashMap<String, String>>| async move {
                Redirect::to(&query["to"])
            }),
        )
        .route(
            "/icon-large",
            get(|| async { ([("content-type", "image/png")], vec![0_u8; 256 * 1024 + 1]) }),
        )
        .route(
            "/large/",
            get(|| async { Html("a".repeat(256 * 1024 + 1)) }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let tmp = tempfile::tempdir().unwrap();
    let core = Core::new(tmp.path().join("vault.sqlite3")).unwrap();
    assert!(core.favicon_page("unknown").await.is_err());
    core.create("fictional master password").await.unwrap();
    core.set_headers(
        None,
        vec![
            HeaderEntry {
                name: "User-Agent".into(),
                value: "fixture-global-agent".into(),
                enabled: true,
            },
            HeaderEntry {
                name: "X-Request-Id".into(),
                value: "fixture-inherited".into(),
                enabled: true,
            },
            HeaderEntry {
                name: "Cache-Control".into(),
                value: "no-cache".into(),
                enabled: false,
            },
            HeaderEntry {
                name: "Authorization".into(),
                value: "Bearer fixture".into(),
                enabled: true,
            },
            HeaderEntry {
                name: "Cookie".into(),
                value: "fixture=value".into(),
                enabled: true,
            },
            HeaderEntry {
                name: "X-Secret".into(),
                value: "fixture".into(),
                enabled: true,
            },
        ],
    )
    .await
    .unwrap();
    let draft = |url: String, name: &str| EnvironmentDraft {
        name: name.into(),
        url,
        database: "fixture-db".into(),
        login: "fixture-user".into(),
        secret: "fixture-secret".into(),
        protocol: Protocol::LegacyJsonrpc,
        context: json!({}),
    };
    let env = core
        .save_environment(draft(url.clone(), "Fixture"))
        .await
        .unwrap();
    core.set_headers(
        Some(&env.id),
        vec![
            HeaderEntry {
                name: "user-AGENT".into(),
                value: "fixture-local-agent".into(),
                enabled: true,
            },
            HeaderEntry {
                name: "Accept-Language".into(),
                value: "es-ES".into(),
                enabled: true,
            },
            HeaderEntry {
                name: "X-Local-Secret".into(),
                value: "fixture-secret".into(),
                enabled: true,
            },
        ],
    )
    .await
    .unwrap();
    let page = core.favicon_page(&env.id).await.unwrap();
    assert_eq!(page.url, format!("{url}/web/login"));
    assert!(page.html.contains("/web/image/website/2/favicon"));
    let image = core
        .favicon_image(&env.id, &format!("{url}/icon-redirect"))
        .await
        .unwrap();
    assert_eq!(image.content_type, "image/png");
    assert_eq!(image.data, vec![1, 2, 3]);
    // Reject other origins, including another port on the same host, even after redirects.
    let other = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let other_url = format!("http://{}", other.local_addr().unwrap());
    let other_hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let hits = other_hits.clone();
    let other_task = tokio::spawn(async move {
        let app = Router::new().route(
            "/icon",
            get(move || async move {
                hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                ([("content-type", "image/png")], vec![1_u8])
            }),
        );
        axum::serve(other, app).await.unwrap();
    });
    assert!(
        core.favicon_image(&env.id, &format!("{other_url}/icon"))
            .await
            .is_err()
    );
    assert!(
        core.favicon_image(
            &env.id,
            &format!("{url}/icon-elsewhere?to={other_url}/icon")
        )
        .await
        .is_err()
    );
    assert_eq!(other_hits.load(std::sync::atomic::Ordering::SeqCst), 0);
    other_task.abort();
    for invalid in [
        format!("{url}/web/login"),
        format!("{url}/icon-large"),
        format!("{url}/loop/"),
        "file:///etc/passwd".into(),
        "https://user:password@example.test/icon".into(),
    ] {
        assert!(core.favicon_image(&env.id, &invalid).await.is_err());
    }
    for path in ["loop", "large"] {
        let env = core
            .save_environment(draft(format!("{url}/{path}"), path))
            .await
            .unwrap();
        assert!(core.favicon_page(&env.id).await.is_err());
    }
    core.lock().await;
    assert!(core.favicon_page(&env.id).await.is_err());
    assert!(
        core.favicon_image(&env.id, &format!("{url}/icon"))
            .await
            .is_err()
    );
    task.abort();
}
