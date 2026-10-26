//! Global/environment outbound proxies against test HTTP and SOCKS5 servers.
mod flow;
mod support;
use odx_core::{
    ActivityFilter, Core, EnvironmentDraft, Protocol, WriteMode,
    proxy::{EnvironmentProxyInput, ProxyInput, ProxyMode},
};
use serde_json::{Value, json};
use std::sync::Arc;
use support::proxy::{HIDDEN_HOST, TestProxy, closed_port, digest_proxy, http_proxy, socks_proxy};

/// Save the proxy called `name`, creating it if needed; returns its id.
pub async fn named(
    core: &odx_core::Core,
    name: &str,
    mut input: odx_core::proxy::ProxyInput,
) -> odx_core::error::Result<String> {
    input.name = name.into();
    let id = core
        .proxies()
        .await?
        .proxies
        .into_iter()
        .find(|p| p.name == name)
        .map(|p| p.id);
    core.save_proxy(id, input).await
}
/// The default proxy, saved as "General"; `None` makes the default direct.
pub async fn set_global(
    core: &odx_core::Core,
    input: Option<odx_core::proxy::ProxyInput>,
) -> odx_core::error::Result<()> {
    let id = match input {
        Some(input) => Some(named(core, "General", input).await?),
        None => None,
    };
    core.set_default_proxy(id).await
}
/// A proxy of the environment's own, saved as "Own <id>" and selected for it.
pub async fn set_own(
    core: &odx_core::Core,
    env: &str,
    input: odx_core::proxy::ProxyInput,
) -> odx_core::error::Result<()> {
    let id = named(core, &format!("Own {env}"), input).await?;
    core.set_environment_proxy(env, odx_core::proxy::EnvironmentProxyInput::Profile { id })
        .await
}

const PROTOCOLS: [Protocol; 3] = [Protocol::LegacyJsonrpc, Protocol::Xmlrpc, Protocol::Json2];
const USER: &str = "fixture-proxy-user";
const PASSWORD: &str = "fixture-proxy-pass";

fn proxy(url: &str) -> ProxyInput {
    ProxyInput {
        name: String::new(),
        url: url.into(),
        username: String::new(),
        password: String::new(),
    }
}
fn proxy_with_auth(url: &str, user: &str, password: &str) -> ProxyInput {
    ProxyInput {
        name: String::new(),
        url: url.into(),
        username: user.into(),
        password: password.into(),
    }
}
fn draft(name: &str, url: &str, protocol: Protocol) -> EnvironmentDraft {
    EnvironmentDraft {
        name: name.into(),
        url: url.into(),
        database: "base-a".into(),
        login: "fixture-user".into(),
        protocol,
        context: json!({}),
        secret: "fixture-secret".into(),
    }
}
/// Simulator URL with a hostname resolved only by test proxies.
fn hidden(sim_url: &str) -> String {
    sim_url.replace("127.0.0.1", HIDDEN_HOST)
}
async fn core() -> (Arc<Core>, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let core = support::unlocked_core(&tmp).await;
    (core, tmp)
}
async fn activity_dump(core: &Core) -> String {
    format!(
        "{:?}",
        core.activity(None, ActivityFilter::default())
            .await
            .unwrap()
            .events
    )
}

/// Exercise health, all protocols, download and favicon through the global proxy; only the proxy can resolve the target.
async fn assert_all_egress_through(general: &TestProxy, scheme: &str) {
    let sim = support::protocol_simulator(19).await;
    let (core, _tmp) = core().await;
    let url = hidden(&sim.url);
    let mut ids = Vec::new();
    for protocol in PROTOCOLS {
        let name = format!("{protocol:?}");
        ids.push(
            core.save_environment(draft(&name, &url, protocol))
                .await
                .unwrap()
                .id,
        );
    }
    // Without a proxy, hidden-name DNS fails, proving there is no alternate route.
    let report = core.check_environment(&ids[0]).await.unwrap();
    assert_eq!(report.network_error.as_deref(), Some("dns"), "{report:?}");
    set_global(&core, Some(proxy(&general.url(scheme))))
        .await
        .unwrap();
    let client = flow::client(&core).await;
    for (id, protocol) in ids.iter().zip(PROTOCOLS) {
        let report = core.check_environment(id).await.unwrap();
        assert_eq!(report.status, "ok", "{protocol:?} {report:?}");
        let out = core
            .run_tool(
                &client,
                "search_read",
                json!({"env": format!("{protocol:?}"), "model":"res.partner","fields":["name"],"limit":1}),
            )
            .await
            .unwrap();
        assert!(!out.data.to_string().is_empty(), "{protocol:?}");
    }
    let download = core
        .run_tool(
            &client,
            "download",
            json!({"env":"LegacyJsonrpc","model":"ir.attachment","id":9}),
        )
        .await
        .unwrap();
    assert_eq!(download.data["bytes"], 14);
    // The simulator has no favicon; any HTTP response proves network reachability.
    let page = core.favicon_page(&ids[0]).await;
    assert!(
        !matches!(page, Err(odx_core::error::OdxError::Network { .. })),
        "{:?}",
        page.err()
    );
    let image = core
        .favicon_image(&ids[0], &format!("{url}/web/image/website/1/favicon"))
        .await;
    assert!(
        !matches!(image, Err(odx_core::error::OdxError::Network { .. })),
        "{:?}",
        image.err()
    );
    let entries = general.log.entries();
    let port = url.rsplit(':').next().unwrap();
    let target = format!("{HIDDEN_HOST}:{port}");
    if scheme == "socks5h" {
        // socks5h delegates hostname resolution to the proxy.
        assert!(
            entries.iter().all(|e| e == &format!("SOCKS {target}")),
            "{entries:?}"
        );
        assert!(!entries.is_empty());
    } else {
        for path in [
            "/jsonrpc",
            "/xmlrpc/2/common",
            "/xmlrpc/2/object",
            "/json/2/",
            "/web/version",
            "/?db=base-a",
            "/web/image/website/1/favicon",
        ] {
            let wanted = format!("http://{target}{path}");
            assert!(
                entries.iter().any(|e| e.contains(&wanted)),
                "{wanted} {entries:?}"
            );
        }
    }
}

#[tokio::test]
async fn every_outgoing_request_uses_the_general_http_proxy() {
    let general = http_proxy(None).await;
    assert_all_egress_through(&general, "http").await;
}

#[tokio::test]
async fn every_outgoing_request_uses_the_general_socks5h_proxy() {
    let general = socks_proxy(None).await;
    assert_all_egress_through(&general, "socks5h").await;
}

#[tokio::test]
async fn https_destinations_tunnel_through_connect_and_socks_and_keep_tls_verification() {
    let (port, decrypted) = support::untrusted_tls_server().await;
    let http = http_proxy(None).await;
    let socks = socks_proxy(None).await;
    for (proxy_url, log) in [
        (http.url("http"), &http.log),
        (socks.url("socks5h"), &socks.log),
    ] {
        let (core, _tmp) = core().await;
        let env = core
            .save_environment(draft(
                "A",
                &format!("https://{HIDDEN_HOST}:{port}"),
                Protocol::LegacyJsonrpc,
            ))
            .await
            .unwrap();
        set_global(&core, Some(proxy(&proxy_url))).await.unwrap();
        let report = core.check_environment(&env.id).await.unwrap();
        // Tunnel succeeds but the certificate is untrusted: classify as tls.
        assert_eq!(report.network_error.as_deref(), Some("tls"), "{report:?}");
        let entries = log.entries();
        assert!(
            entries.contains(&format!("CONNECT {HIDDEN_HOST}:{port}"))
                || entries.contains(&format!("SOCKS {HIDDEN_HOST}:{port}")),
            "{entries:?}"
        );
    }
    assert_eq!(decrypted.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[tokio::test]
async fn custom_environment_proxy_overrides_the_general_one_and_direct_bypasses_it() {
    let sim = support::simulator().await;
    let general = http_proxy(None).await;
    let own = socks_proxy(None).await;
    let (core, _tmp) = core().await;
    let inherit = core
        .save_environment(draft("inherit", &hidden(&sim.url), Protocol::LegacyJsonrpc))
        .await
        .unwrap();
    let custom = core
        .save_environment(draft("custom", &hidden(&sim.url), Protocol::LegacyJsonrpc))
        .await
        .unwrap();
    let direct = core
        .save_environment(draft("direct", &sim.url, Protocol::LegacyJsonrpc))
        .await
        .unwrap();
    // Without a global proxy, inherited access is direct.
    assert_eq!(
        core.environment_proxy(&inherit.id).await.unwrap().mode,
        ProxyMode::Inherit
    );
    assert_eq!(
        core.check_environment(&direct.id).await.unwrap().status,
        "ok"
    );
    set_global(&core, Some(proxy(&general.url("http"))))
        .await
        .unwrap();
    set_own(&core, &custom.id, proxy(&own.url("socks5h")))
        .await
        .unwrap();
    core.set_environment_proxy(&direct.id, EnvironmentProxyInput::Direct)
        .await
        .unwrap();
    let view = core.proxies().await.unwrap();
    let default = view
        .proxies
        .iter()
        .find(|p| Some(&p.id) == view.default.as_ref())
        .unwrap();
    assert_eq!(default.url, general.url("http"));

    let before = (general.log.connections(), own.log.connections());
    assert_eq!(
        core.check_environment(&custom.id).await.unwrap().status,
        "ok"
    );
    assert_eq!(
        general.log.connections(),
        before.0,
        "custom proxy bypasses the global proxy"
    );
    assert!(own.log.connections() > before.1);

    let before = (general.log.connections(), own.log.connections());
    let queries = sim.counts.headers().len();
    assert_eq!(
        core.check_environment(&direct.id).await.unwrap().status,
        "ok"
    );
    assert_eq!(
        (general.log.connections(), own.log.connections()),
        before,
        "direct bypasses every proxy"
    );
    assert!(sim.counts.headers().len() > queries);

    let before = own.log.connections();
    assert_eq!(
        core.check_environment(&inherit.id).await.unwrap().status,
        "ok"
    );
    assert_eq!(own.log.connections(), before);
    assert!(
        general
            .log
            .entries()
            .iter()
            .any(|e| e.starts_with("POST http://odoo.invalid"))
    );
    // Removing the global proxy restores direct access; the hidden name stops resolving.
    set_global(&core, None).await.unwrap();
    let report = core.check_environment(&inherit.id).await.unwrap();
    assert_eq!(report.network_error.as_deref(), Some("dns"), "{report:?}");
    assert_eq!(
        core.check_environment(&custom.id).await.unwrap().status,
        "ok"
    );
}

#[tokio::test]
async fn proxy_authentication_is_required_and_rejections_are_proxy_errors() {
    let sim = support::simulator().await;
    let (tls_port, _) = support::untrusted_tls_server().await;
    let http = http_proxy(Some((USER, PASSWORD))).await;
    let socks = socks_proxy(Some((USER, PASSWORD))).await;
    let (core, _tmp) = core().await;
    let plain = core
        .save_environment(draft("plain", &hidden(&sim.url), Protocol::LegacyJsonrpc))
        .await
        .unwrap();
    let tls = core
        .save_environment(draft(
            "tls",
            &format!("https://{HIDDEN_HOST}:{tls_port}"),
            Protocol::LegacyJsonrpc,
        ))
        .await
        .unwrap();
    let client = flow::client(&core).await;
    for (url, scheme) in [
        (http.url("http"), "http"),
        (socks.url("socks5h"), "socks5h"),
    ] {
        for (user, password) in [("", ""), (USER, "fixture-wrong-pass")] {
            set_global(&core, Some(proxy_with_auth(&url, user, password)))
                .await
                .unwrap();
            // HTTP forwarding/HTTPS CONNECT return 407; SOCKS rejects the method or credentials.
            for id in [&plain.id, &tls.id] {
                let report = core.check_environment(id).await.unwrap();
                assert_eq!(report.status, "unreachable", "{scheme} {report:?}");
                assert_eq!(
                    report.network_error.as_deref(),
                    Some("proxy"),
                    "{scheme} {user} {report:?}"
                );
            }
            let error = core
                .run_tool(
                    &client,
                    "search_read",
                    json!({"env":"plain","model":"res.partner","limit":1}),
                )
                .await
                .unwrap_err();
            assert_eq!(error.code, "upstream_network", "{error:?}");
            assert!(error.message.contains("proxy"), "{error:?}");
            assert!(!error.message.contains("fixture-wrong-pass"), "{error:?}");
        }
        set_global(&core, Some(proxy_with_auth(&url, USER, PASSWORD)))
            .await
            .unwrap();
        assert_eq!(
            core.check_environment(&plain.id).await.unwrap().status,
            "ok"
        );
        assert_eq!(
            core.check_environment(&tls.id)
                .await
                .unwrap()
                .network_error
                .as_deref(),
            Some("tls")
        );
    }
    assert!(http.log.entries().iter().any(|e| e.starts_with("407 POST")));
    assert!(
        http.log
            .entries()
            .iter()
            .any(|e| e.starts_with("407 CONNECT"))
    );
    assert!(socks.log.entries().iter().any(|e| e == "SOCKS auth-failed"));
    let dump = activity_dump(&core).await;
    assert!(!dump.contains(PASSWORD) && !dump.contains("fixture-wrong-pass"));
}

#[tokio::test]
async fn an_unreachable_proxy_fails_without_falling_back_to_a_direct_connection() {
    let sim = support::simulator().await;
    let (core, _tmp) = core().await;
    let env = core
        .save_environment(draft("A", &sim.url, Protocol::LegacyJsonrpc))
        .await
        .unwrap();
    core.set_mode(&env.id, WriteMode::Permanent).await.unwrap();
    let client = flow::client(&core).await;
    let port = closed_port().await;
    for scheme in ["http", "socks5h"] {
        set_global(
            &core,
            Some(proxy_with_auth(
                &format!("{scheme}://127.0.0.1:{port}"),
                USER,
                PASSWORD,
            )),
        )
        .await
        .unwrap();
        let report = core.check_environment(&env.id).await.unwrap();
        assert_eq!(report.status, "unreachable");
        assert_eq!(report.network_error.as_deref(), Some("proxy"), "{report:?}");
        let error = report.error.unwrap();
        assert!(
            !error.contains(PASSWORD) && !error.contains(USER),
            "{error}"
        );
        for (tool, args) in [
            (
                "search_read",
                json!({"env":"A","model":"res.partner","limit":1}),
            ),
            (
                "write",
                json!({"env":"A","model":"res.partner","ids":[1],"values":{"name":"x"}}),
            ),
        ] {
            let error = core.run_tool(&client, tool, args).await.unwrap_err();
            // Pre-send proxy connection failure is a known failure.
            assert_eq!(error.code, "upstream_network", "{tool} {error:?}");
            assert!(error.message.contains("proxy"), "{error:?}");
        }
    }
    assert!(sim.counts.headers().is_empty(), "no direct connection");
    assert_eq!(sim.counts.writes(), 0);
    let dump = activity_dump(&core).await;
    assert!(!dump.contains(PASSWORD), "{dump}");
}

#[tokio::test]
async fn changing_a_proxy_revokes_pending_requests_and_invalidates_authentication() {
    let sim = support::simulator().await;
    let general = http_proxy(None).await;
    let own = http_proxy(None).await;
    let (core, _tmp) = core().await;
    let a = core
        .save_environment(draft("A", &sim.url, Protocol::LegacyJsonrpc))
        .await
        .unwrap();
    let b = core
        .save_environment(draft("B", &sim.url, Protocol::LegacyJsonrpc))
        .await
        .unwrap();
    for id in [&a.id, &b.id] {
        core.set_mode(id, WriteMode::Approval).await.unwrap();
    }
    core.set_environment_proxy(&b.id, EnvironmentProxyInput::Direct)
        .await
        .unwrap();
    let writer = flow::client(&core).await;
    let write =
        |env: &str| json!({"env":env,"model":"res.partner","ids":[1],"values":{"name":"x"}});
    core.check_environment(&a.id).await.unwrap();
    let logins = sim.counts.logins();
    let (call_a, pending_a) = flow::queue(&core, &writer, "write", write("A")).await;
    let (call_b, pending_b) = flow::queue(&core, &writer, "write", write("B")).await;
    // Global proxy changes affect inheriting environments only.
    set_global(&core, Some(proxy(&general.url("http"))))
        .await
        .unwrap();
    assert!(core.decide(&pending_a.id, true).await.is_err());
    assert_eq!(flow::error_code(call_a).await, "approval_revoked");
    core.check_environment(&a.id).await.unwrap();
    assert_eq!(
        sim.counts.logins(),
        logins + 1,
        "authentication invalidated"
    );
    assert!(general.log.connections() > 0);
    // Saving identical proxy settings changes nothing.
    let (call_a, pending_a) = flow::queue(&core, &writer, "write", write("A")).await;
    set_global(&core, Some(proxy(&general.url("http"))))
        .await
        .unwrap();
    let pending: Vec<String> = core
        .approvals()
        .await
        .unwrap()
        .into_iter()
        .map(|p| p.id)
        .collect();
    assert!(pending.contains(&pending_a.id) && pending.contains(&pending_b.id));
    // B's custom proxy change revokes only B's requests.
    set_own(&core, &b.id, proxy(&own.url("http")))
        .await
        .unwrap();
    assert_eq!(flow::error_code(call_b).await, "approval_revoked");
    core.decide(&pending_a.id, true).await.unwrap();
    call_a.await.unwrap().unwrap();
    assert_eq!(sim.counts.writes(), 1);
    let before = own.log.connections();
    core.check_environment(&b.id).await.unwrap();
    assert!(own.log.connections() > before);
}

#[tokio::test]
async fn proxy_credentials_never_leave_the_backend() {
    let sim = support::simulator().await;
    let proxy_server = http_proxy(Some((USER, PASSWORD))).await;
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("vault.sqlite3");
    let core = Core::new(&path).unwrap();
    assert!(core.proxies().await.is_err(), "locked");
    core.create("fictional master password").await.unwrap();
    assert!(core.proxies().await.unwrap().proxies.is_empty());
    let env = core
        .save_environment(draft("A", &hidden(&sim.url), Protocol::LegacyJsonrpc))
        .await
        .unwrap();
    let url = proxy_server.url("http");
    set_global(&core, Some(proxy_with_auth(&url, USER, PASSWORD)))
        .await
        .unwrap();
    set_own(&core, &env.id, proxy_with_auth(&url, USER, PASSWORD))
        .await
        .unwrap();
    let proxies = core.proxies().await.unwrap();
    let general = proxies
        .proxies
        .iter()
        .find(|p| p.name == "General")
        .unwrap()
        .clone();
    let local = core.environment_proxy(&env.id).await.unwrap();
    let shown = serde_json::to_string(&proxies).unwrap() + &serde_json::to_string(&local).unwrap();
    assert!(
        !shown.contains(USER) && !shown.contains(PASSWORD),
        "{shown}"
    );
    assert_eq!(general.url, url);
    assert_eq!(general.username, odx_core::headers::MASK);
    assert_eq!(general.password, odx_core::headers::MASK);
    assert_eq!(local.mode, ProxyMode::Profile);
    // Saving placeholders preserves credentials and authentication.
    let back = proxies
        .proxies
        .iter()
        .find(|p| Some(&p.id) == local.id.as_ref())
        .unwrap()
        .clone();
    set_own(
        &core,
        &env.id,
        proxy_with_auth(&back.url, &back.username, &back.password),
    )
    .await
    .unwrap();
    set_global(
        &core,
        Some(proxy_with_auth(
            &general.url,
            &general.username,
            &general.password,
        )),
    )
    .await
    .unwrap();
    assert_eq!(core.check_environment(&env.id).await.unwrap().status, "ok");
    // Redact proxy credentials from Odoo errors and activity.
    let client = flow::client(&core).await;
    sim.counts
        .fail_next_execute(&format!("echo {USER} {PASSWORD}"));
    let error = core
        .run_tool(
            &client,
            "search_read",
            json!({"env":"A","model":"res.partner","limit":1}),
        )
        .await
        .unwrap_err();
    assert!(
        !error.message.contains(PASSWORD) && !error.message.contains(USER),
        "{error:?}"
    );
    assert!(error.message.contains("[redacted]"), "{error:?}");
    // Clients cannot see proxy configuration.
    let listed = core.run_tool(&client, "envs", json!({})).await.unwrap();
    let text = odx_core::output::render(&listed).text + &listed.envelope().to_string();
    assert!(
        !text.contains(USER) && !text.contains(PASSWORD) && !text.contains(&url),
        "{text}"
    );
    let views = serde_json::to_string(&core.environments().await.unwrap()).unwrap();
    assert!(
        !views.contains(PASSWORD) && !views.contains("proxy"),
        "{views}"
    );
    let dump = activity_dump(&core).await;
    assert!(!dump.contains(PASSWORD) && !dump.contains(USER), "{dump}");
    // Encrypt in the vault and preserve across reopen.
    core.lock().await;
    assert!(!String::from_utf8_lossy(&std::fs::read(&path).unwrap()).contains(PASSWORD));
    let reopened = Core::new(&path).unwrap();
    reopened.unlock("fictional master password").await.unwrap();
    assert_eq!(
        reopened.check_environment(&env.id).await.unwrap().status,
        "ok"
    );
    // Reject placeholders without saved credentials.
    assert!(
        named(
            &reopened,
            "Fresh",
            proxy_with_auth(&url, odx_core::headers::MASK, odx_core::headers::MASK),
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn proxy_urls_are_validated_without_echoing_credentials() {
    let (core, _tmp) = core().await;
    for url in [
        "socks5://127.0.0.1:1080",
        "socks4://127.0.0.1:1080",
        "ftp://127.0.0.1:21",
        "http://user:fixture-url-pass@127.0.0.1:3128",
        "http://127.0.0.1:3128/?q=1",
        "http://127.0.0.1:3128/#f",
        "http://127.0.0.1:3128/path",
        "http://127.0.0.1",
        "socks5h://127.0.0.1",
        "http://:3128",
        "127.0.0.1:3128",
        "",
    ] {
        let error = set_global(&core, Some(proxy(url))).await.unwrap_err();
        assert!(!error.to_string().contains("fixture-url-pass"), "{url}");
    }
    // Reject passwords without usernames and colons in Basic usernames.
    for (user, password) in [("", PASSWORD), ("a:b", PASSWORD)] {
        let error = set_global(
            &core,
            Some(proxy_with_auth("http://127.0.0.1:3128", user, password)),
        )
        .await
        .unwrap_err();
        assert!(!error.to_string().contains(PASSWORD));
    }
    for url in [
        "http://127.0.0.1:3128",
        "https://proxy.example.test:8443/",
        "socks5h://[::1]:1080",
        "HTTP://Proxy.Example.Test:80",
    ] {
        set_global(&core, Some(proxy(url))).await.unwrap();
    }
    let proxies = core.proxies().await.unwrap();
    assert_eq!(proxies.proxies.len(), 1, "saved over the same proxy");
    assert_eq!(proxies.proxies[0].url, "http://proxy.example.test:80");
    let value: Value = serde_json::to_value(&proxies.proxies[0]).unwrap();
    assert_eq!(value["username"], "");
    // Names are required and unique, ignoring case.
    let unnamed = core
        .save_proxy(None, proxy("http://127.0.0.1:3128"))
        .await
        .unwrap_err();
    assert_eq!(unnamed.code(), "validation.proxy.name_invalid");
    let mut taken = proxy("http://127.0.0.1:3128");
    taken.name = "general".into();
    assert_eq!(
        core.save_proxy(None, taken).await.unwrap_err().code(),
        "validation.proxy.name_taken"
    );
}

#[tokio::test]
async fn proxies_in_use_cannot_be_deleted_and_the_default_can_be_cleared() {
    let (core, _tmp) = core().await;
    let env = core
        .save_environment(draft("A", "https://a.test", Protocol::LegacyJsonrpc))
        .await
        .unwrap();
    set_own(&core, &env.id, proxy("http://own.test:3128"))
        .await
        .unwrap();
    set_global(&core, Some(proxy("http://general.test:3128")))
        .await
        .unwrap();
    let view = core.proxies().await.unwrap();
    let own = view
        .proxies
        .iter()
        .find(|p| p.name.starts_with("Own"))
        .unwrap()
        .id
        .clone();
    let general = view.default.clone().unwrap();
    let counts: Vec<_> = view
        .proxies
        .iter()
        .map(|p| (p.name.starts_with("Own"), p.environments))
        .collect();
    assert!(
        counts.contains(&(true, 1)) && counts.contains(&(false, 0)),
        "{counts:?}"
    );
    let error = core.delete_proxy(&own).await.unwrap_err();
    assert_eq!(error.code(), "validation.proxy.in_use");
    // Unknown proxies are rejected instead of silently connecting directly.
    let unknown = core
        .set_environment_proxy(
            &env.id,
            EnvironmentProxyInput::Profile {
                id: "missing".into(),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(unknown.code(), "validation.proxy.not_found");
    core.delete_proxy(&general).await.unwrap();
    assert!(core.proxies().await.unwrap().default.is_none());
    core.set_environment_proxy(&env.id, EnvironmentProxyInput::Inherit)
        .await
        .unwrap();
    core.delete_proxy(&own).await.unwrap();
    assert!(core.proxies().await.unwrap().proxies.is_empty());
}

#[tokio::test]
async fn editing_the_connection_keeps_the_environment_proxy() {
    let (core, _tmp) = core().await;
    let custom = core
        .save_environment(draft(
            "custom",
            "https://custom.test",
            Protocol::LegacyJsonrpc,
        ))
        .await
        .unwrap();
    let direct = core
        .save_environment(draft(
            "direct",
            "https://direct.test",
            Protocol::LegacyJsonrpc,
        ))
        .await
        .unwrap();
    set_own(
        &core,
        &custom.id,
        proxy_with_auth("http://proxy.example.test:3128", USER, PASSWORD),
    )
    .await
    .unwrap();
    core.set_environment_proxy(&direct.id, EnvironmentProxyInput::Direct)
        .await
        .unwrap();
    for (env, name, url) in [
        (&custom, "custom", "https://custom.test"),
        (&direct, "direct", "https://direct.test"),
    ] {
        let mut changed = draft(name, url, Protocol::LegacyJsonrpc);
        changed.login = "another-user".into();
        core.update_environment(&env.id, changed).await.unwrap();
    }
    let view = core.environment_proxy(&custom.id).await.unwrap();
    assert_eq!(view.mode, ProxyMode::Profile);
    let proxies = core.proxies().await.unwrap().proxies;
    let kept = proxies
        .iter()
        .find(|p| Some(&p.id) == view.id.as_ref())
        .unwrap();
    assert_eq!(kept.url, "http://proxy.example.test:3128");
    assert!(!kept.username.is_empty(), "credentials kept (shown masked)");
    assert_eq!(
        core.environment_proxy(&direct.id).await.unwrap().mode,
        ProxyMode::Direct
    );
}

#[tokio::test]
async fn a_digest_only_proxy_is_reached_through_a_relay_started_on_demand() {
    let sim = support::simulator().await;
    let basic = http_proxy(Some((USER, PASSWORD))).await;
    let digest = digest_proxy(USER, PASSWORD).await;
    let (core, _tmp) = core().await;
    let env = core
        .save_environment(draft("A", &hidden(&sim.url), Protocol::LegacyJsonrpc))
        .await
        .unwrap();
    // Accepted Basic credentials never probe for Digest.
    set_global(
        &core,
        Some(proxy_with_auth(&basic.url("http"), USER, PASSWORD)),
    )
    .await
    .unwrap();
    assert_eq!(core.check_environment(&env.id).await.unwrap().status, "ok");
    assert!(
        basic.log.entries().iter().all(|e| !e.starts_with("407")),
        "{:?}",
        basic.log.entries()
    );
    set_global(
        &core,
        Some(proxy_with_auth(&digest.url("http"), USER, PASSWORD)),
    )
    .await
    .unwrap();
    let report = core.check_environment(&env.id).await.unwrap();
    assert_eq!(report.status, "ok", "{report:?} {:?}", digest.log.entries());
    let entries = digest.log.entries();
    // Forwarded Basic is rejected, then every request goes through authenticated CONNECT tunnels.
    assert!(
        entries.iter().any(|e| e.starts_with("407 POST")),
        "{entries:?}"
    );
    let tunnel = format!("CONNECT {}", hidden(&sim.url).trim_start_matches("http://"));
    assert!(entries.contains(&tunnel), "{entries:?}");
    let client = flow::client(&core).await;
    core.run_tool(
        &client,
        "search_read",
        json!({"env":"A","model":"res.partner","fields":["name"],"limit":1}),
    )
    .await
    .unwrap();
    assert_eq!(
        digest
            .log
            .entries()
            .iter()
            .filter(|e| e.starts_with("407 POST"))
            .count(),
        1,
        "the relay is reused"
    );

    // HTTPS destinations detect the rejection from the CONNECT tunnel instead of a 407 response.
    let (tls_port, _) = support::untrusted_tls_server().await;
    let tls = core
        .save_environment(draft(
            "tls",
            &format!("https://{HIDDEN_HOST}:{tls_port}"),
            Protocol::LegacyJsonrpc,
        ))
        .await
        .unwrap();
    let tunnel_only = digest_proxy(USER, PASSWORD).await;
    set_global(
        &core,
        Some(proxy_with_auth(&tunnel_only.url("http"), USER, PASSWORD)),
    )
    .await
    .unwrap();
    let report = core.check_environment(&tls.id).await.unwrap();
    // The untrusted certificate is only reached through an open tunnel.
    assert_eq!(report.network_error.as_deref(), Some("tls"), "{report:?}");
    let entries = tunnel_only.log.entries();
    assert!(
        entries.contains(&format!("407 CONNECT {HIDDEN_HOST}:{tls_port}")),
        "{entries:?}"
    );
    assert!(
        entries.contains(&format!("CONNECT {HIDDEN_HOST}:{tls_port}")),
        "{entries:?}"
    );

    let wrong = digest_proxy(USER, "fixture-wrong-pass").await;
    set_global(
        &core,
        Some(proxy_with_auth(&wrong.url("http"), USER, PASSWORD)),
    )
    .await
    .unwrap();
    let report = core.check_environment(&env.id).await.unwrap();
    assert_eq!(report.network_error.as_deref(), Some("proxy"), "{report:?}");
    assert!(!wrong.log.entries().iter().any(|e| e.starts_with("CONNECT")));
    let error = core
        .run_tool(
            &client,
            "search_read",
            json!({"env":"A","model":"res.partner","limit":1}),
        )
        .await
        .unwrap_err();
    assert!(error.message.contains("Digest"), "{error:?}");
    assert!(!activity_dump(&core).await.contains(PASSWORD));
}

#[tokio::test]
async fn testing_a_proxy_reaches_the_server_only_through_it() {
    let sim = support::simulator().await;
    let proxy_server = http_proxy(Some((USER, PASSWORD))).await;
    let (core, _tmp) = core().await;
    let url = hidden(&sim.url);
    let good = proxy_with_auth(&proxy_server.url("http"), USER, PASSWORD);
    // Only the proxy resolves the hidden host, so any answer proves the route.
    let before = proxy_server.log.connections();
    core.test_proxy(None, good, &url).await.unwrap();
    assert!(proxy_server.log.connections() > before);
    // Wrong credentials and dead proxies are proxy errors, never a direct fallback.
    let wrong = proxy_with_auth(&proxy_server.url("http"), USER, "fixture-wrong-pass");
    let error = core.test_proxy(None, wrong, &url).await.unwrap_err();
    assert_eq!(error.code(), "upstream.network");
    assert!(
        error.to_string().contains("proxy") && !error.to_string().contains("fixture-wrong-pass"),
        "{error}"
    );
    let dead = proxy(&format!("http://127.0.0.1:{}", closed_port().await));
    assert!(core.test_proxy(None, dead, &url).await.is_err());
    // A saved proxy is tested with its stored credentials behind the mask.
    let id = named(
        &core,
        "Office",
        proxy_with_auth(&proxy_server.url("http"), USER, PASSWORD),
    )
    .await
    .unwrap();
    let masked = proxy_with_auth(
        &proxy_server.url("http"),
        odx_core::headers::MASK,
        odx_core::headers::MASK,
    );
    assert!(core.test_proxy(Some(id), masked, &url).await.is_ok());
    // Testing saves nothing.
    assert_eq!(core.proxies().await.unwrap().proxies.len(), 1);
}

#[tokio::test]
async fn a_draft_is_checked_through_the_proxy_chosen_for_it() {
    let sim = support::simulator().await;
    let (core, _tmp) = core().await;
    // The default proxy is dead, so only a direct check reaches the simulator.
    set_global(
        &core,
        Some(proxy(&format!("http://127.0.0.1:{}", closed_port().await))),
    )
    .await
    .unwrap();
    let inherited = core
        .check_draft(draft("New", &sim.url, Protocol::LegacyJsonrpc))
        .await
        .unwrap();
    assert_eq!(
        inherited.network_error.as_deref(),
        Some("proxy"),
        "{inherited:?}"
    );
    let direct = core
        .check_draft_with_proxy(
            draft("New", &sim.url, Protocol::LegacyJsonrpc),
            EnvironmentProxyInput::Direct,
        )
        .await
        .unwrap();
    assert_eq!(direct.status, "ok", "{direct:?}");
    let unknown = core
        .check_draft_with_proxy(
            draft("New", &sim.url, Protocol::LegacyJsonrpc),
            EnvironmentProxyInput::Profile {
                id: "missing".into(),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(unknown.code(), "validation.proxy.not_found");
}
