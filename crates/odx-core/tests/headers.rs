mod flow;
mod support;
use odx_core::{
    Capability, ClientScope, Core, EnvironmentDraft, Protocol, ShellCall, WriteMode,
    headers::HeaderEntry,
};
use serde_json::json;

fn header(name: &str, value: &str) -> HeaderEntry {
    HeaderEntry {
        name: name.into(),
        value: value.into(),
        enabled: true,
    }
}
fn draft(url: &str, name: &str) -> EnvironmentDraft {
    EnvironmentDraft {
        name: name.into(),
        url: url.into(),
        database: "base-a".into(),
        login: "fixture-user".into(),
        secret: "fixture-secret".into(),
        protocol: Protocol::LegacyJsonrpc,
        context: json!({}),
    }
}
fn write() -> serde_json::Value {
    json!({"env":"A","model":"res.partner","ids":[1],"values":{"name":"fixture"}})
}

#[tokio::test]
async fn short_sensitive_headers_are_redacted_in_client_errors_and_audit_details() {
    let tmp = tempfile::tempdir().unwrap();
    let core = support::unlocked_core(&tmp).await;
    let sim = support::simulator().await;
    let env = core.save_environment(draft(&sim.url, "A")).await.unwrap();
    let issued = core
        .create_client("reader", vec![env.id.clone()], vec![Capability::OdooRead])
        .await
        .unwrap();
    let client = core.authenticate_client(&issued.token).await.unwrap();
    for secret in ["abc", "abc123"] {
        core.set_headers(Some(&env.id), vec![header("X-Api-Key", secret)])
            .await
            .unwrap();
        sim.counts
            .fail_next_execute(&format!("header=prefix_{secret}_suffix"));
        let error = core
            .run_tool(
                &client,
                "search_count",
                json!({"env":"A", "model":"res.partner"}),
            )
            .await
            .unwrap_err();
        assert_eq!(error.message, "header=prefix_[redacted]_suffix");
        let page = core.activity(None, Default::default()).await.unwrap();
        let details = core
            .activity_details(page.events[0].request_id.clone().unwrap())
            .await
            .unwrap()
            .unwrap();
        assert!(
            details
                .error
                .unwrap()
                .text
                .contains("prefix_[redacted]_suffix")
        );
    }
}

#[tokio::test]
async fn headers_reach_every_upstream_path_and_remain_private_and_encrypted() {
    let tmp = tempfile::tempdir().unwrap();
    let sim = support::simulator().await;
    let path = tmp.path().join("vault.sqlite3");
    let core = Core::new(&path).unwrap();
    assert!(core.header_settings(None).await.is_err());
    core.create("fictional master password").await.unwrap();
    let a = core.save_environment(draft(&sim.url, "A")).await.unwrap();
    core.save_environment(draft(&sim.url, "B")).await.unwrap();
    assert!(core.header_settings(None).await.unwrap().headers.is_empty());
    assert!(
        core.header_settings(Some(&a.id))
            .await
            .unwrap()
            .headers
            .is_empty()
    );
    core.check_environment(&a.id).await.unwrap();
    for headers in sim.counts.headers() {
        assert_eq!(headers["accept"], "*/*");
        assert_eq!(headers["content-type"], "application/json");
        assert_eq!(
            headers["user-agent"],
            concat!("Odx/", env!("CARGO_PKG_VERSION"))
        );
    }
    core.set_headers(
        None,
        vec![
            header("User-Agent", "fixture-global-agent"),
            header("X-Inherited", "fixture-global-secret"),
        ],
    )
    .await
    .unwrap();
    core.set_headers(
        Some(&a.id),
        vec![
            header("user-AGENT", "fixture-local-agent"),
            HeaderEntry {
                name: "X-Inherited".into(),
                value: "disabled".into(),
                enabled: false,
            },
            header("Accept", "application/json"),
        ],
    )
    .await
    .unwrap();
    assert!(
        core.set_headers(None, vec![header("X-Test", "invalid\r\nInjected: yes")])
            .await
            .is_err()
    );
    assert_eq!(
        core.header_settings(None).await.unwrap().headers[0].value,
        "fixture-global-agent"
    );
    let start = sim.counts.headers().len();
    core.check_environment(&a.id).await.unwrap();
    for headers in &sim.counts.headers()[start..] {
        assert_eq!(headers["user-agent"], "fixture-local-agent");
        assert_eq!(headers["x-inherited"], "fixture-global-secret");
        assert_eq!(headers["accept"], "application/json");
    }
    let writer = flow::client(&core).await;
    let start = sim.counts.headers().len();
    core.run_tool(
        &writer,
        "search_read",
        json!({"env":"B","model":"res.partner","limit":1}),
    )
    .await
    .unwrap();
    assert_eq!(sim.counts.headers().len() - start, 2); // Fresh login and read.
    for headers in &sim.counts.headers()[start..] {
        assert_eq!(headers["user-agent"], "fixture-global-agent");
    }
    let issued = core
        .create_client(
            "fixture-client",
            ClientScope::All,
            vec![Capability::OdooRead],
        )
        .await
        .unwrap();
    let client = core.authenticate_client(&issued.token).await.unwrap();
    core.run_tool(
        &client,
        "search_read",
        json!({"env":"A", "model":"res.partner", "fields":["display_name"], "limit":1}),
    )
    .await
    .unwrap();
    assert_eq!(
        sim.counts.headers().last().unwrap()["user-agent"],
        "fixture-local-agent"
    );
    let listed = core.run_tool(&client, "envs", json!({})).await.unwrap();
    assert!(!listed.data.to_string().contains("fixture-global-secret"));
    core.set_mode(&a.id, WriteMode::Approval).await.unwrap();
    let (call, pending) = flow::queue(&core, &writer, "write", write()).await;
    core.set_headers(None, vec![header("User-Agent", "fixture-new-agent")])
        .await
        .unwrap();
    assert!(core.decide(&pending.id, true).await.is_err());
    assert_eq!(flow::error_code(call).await, "approval_revoked");
    let (call, pending) = flow::queue(&core, &writer, "write", write()).await;
    core.set_headers(Some(&a.id), vec![header("X-Local", "fixture-local-secret")])
        .await
        .unwrap();
    assert!(core.decide(&pending.id, true).await.is_err());
    assert_eq!(flow::error_code(call).await, "approval_revoked");
    let (call, pending) = flow::queue(&core, &writer, "write", write()).await;
    core.decide(&pending.id, true).await.unwrap();
    call.await.unwrap().unwrap();
    assert_eq!(
        sim.counts.headers().last().unwrap()["x-local"],
        "fixture-local-secret"
    );
    assert_eq!(sim.counts.writes(), 1);
    core.shell_call(ShellCall {
        environment_id: a.id.clone(),
        model: "res.partner".into(),
        method: "search_read".into(),
        args: vec![json!([])],
        kwargs: Default::default(),
        context: Default::default(),
        confirmed: false,
        client_id: None,
        client_revision: None,
    })
    .await
    .unwrap();
    assert_eq!(
        sim.counts.headers().last().unwrap()["user-agent"],
        "fixture-new-agent"
    );
    assert_eq!(
        sim.counts.headers().last().unwrap()["x-local"],
        "fixture-local-secret"
    );
    // Connection/context edits preserve separately managed headers.
    let mut changed = draft(&sim.url, "A");
    changed.secret.clear();
    core.update_environment(&a.id, changed).await.unwrap();
    let copy = core.clone_environment(&a.id, "Copy").await.unwrap();
    let copied = &core.header_settings(Some(&copy.id)).await.unwrap().headers[0];
    assert_eq!(
        (copied.name.as_str(), copied.value.as_str()),
        ("X-Local", odx_core::headers::MASK)
    );
    let views = serde_json::to_string(&core.environments().await.unwrap()).unwrap();
    assert!(!views.contains("fixture-local-secret"));
    assert!(!views.contains("headers"));
    let bytes = std::fs::read(&path).unwrap();
    assert!(!String::from_utf8_lossy(&bytes).contains("fixture-local-secret"));
    core.lock().await;
    assert!(core.set_headers(None, vec![]).await.is_err());
    let reopened = Core::new(path).unwrap();
    reopened.unlock("fictional master password").await.unwrap();
    assert_eq!(
        reopened.header_settings(None).await.unwrap().headers[0].value,
        "fixture-new-agent"
    );
    assert_eq!(
        reopened.header_settings(Some(&a.id)).await.unwrap().headers[0].value,
        odx_core::headers::MASK
    );
    reopened.check_environment(&a.id).await.unwrap();
    assert_eq!(
        sim.counts.headers().last().unwrap()["x-local"],
        "fixture-local-secret"
    );
    reopened.set_headers(Some(&a.id), vec![]).await.unwrap();
    reopened.check_environment(&a.id).await.unwrap();
    assert!(!sim.counts.headers().last().unwrap().contains_key("x-local"));
}

#[tokio::test]
async fn sensitive_header_values_are_masked_and_kept_when_saved_unchanged() {
    let tmp = tempfile::tempdir().unwrap();
    let sim = support::simulator().await;
    let core = support::unlocked_core(&tmp).await;
    let a = core.save_environment(draft(&sim.url, "A")).await.unwrap();
    core.set_headers(None, vec![header("X-Api-Key", "fixture-global-key")])
        .await
        .unwrap();
    core.set_headers(
        Some(&a.id),
        vec![
            header("Authorization", "Bearer fixture-local-token"),
            header("Accept-Language", "es-ES"),
            header("X-Tenant", "fixture-unknown"),
        ],
    )
    .await
    .unwrap();
    let local = core.header_settings(Some(&a.id)).await.unwrap();
    let shown = serde_json::to_string(&local.headers).unwrap()
        + &serde_json::to_string(&local.inherited).unwrap()
        + &serde_json::to_string(&core.header_settings(None).await.unwrap().headers).unwrap();
    assert!(!shown.contains("fixture-local-token") && !shown.contains("fixture-global-key"));
    assert_eq!(local.headers[0].value, odx_core::headers::MASK);
    assert_eq!(local.inherited[0].value, odx_core::headers::MASK);
    assert_eq!(local.headers[1].value, "es-ES");
    // Unknown headers without credential keywords are also masked.
    assert_eq!(local.headers[2].value, odx_core::headers::MASK);
    assert!(!shown.contains("fixture-unknown"));
    // Saving placeholders preserves stored values.
    let mut edited = local.headers.clone();
    edited[1].value = "en-US".into();
    core.set_headers(Some(&a.id), edited).await.unwrap();
    core.check_environment(&a.id).await.unwrap();
    let sent = sim.counts.headers().last().unwrap().clone();
    assert_eq!(sent["authorization"], "Bearer fixture-local-token");
    assert_eq!(sent["x-api-key"], "fixture-global-key");
    assert_eq!(sent["accept-language"], "en-US");
    assert_eq!(sent["x-tenant"], "fixture-unknown");
    // Placeholders cannot move values to other headers or be stored literally.
    for headers in [
        vec![header("X-Other-Token", odx_core::headers::MASK)],
        vec![header("Accept-Language", odx_core::headers::MASK)],
    ] {
        assert!(core.set_headers(Some(&a.id), headers).await.is_err());
    }
    assert!(
        core.set_headers(Some(&a.id), vec![header(" X-Padded", "v")])
            .await
            .is_err()
    );
    assert!(
        core.set_headers(Some(&a.id), vec![header("X-Padded ", "v")])
            .await
            .is_err()
    );
}
