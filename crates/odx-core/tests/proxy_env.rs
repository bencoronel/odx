//! Ignore system proxy variables. Isolate this test in its own binary because it mutates process environment.
mod support;
use odx_core::{Core, EnvironmentDraft, Protocol, proxy::ProxyInput};
use serde_json::json;
use support::proxy::{HIDDEN_HOST, http_proxy};

#[test]
fn system_proxy_variables_are_ignored() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let sim = support::simulator().await;
        let trap = http_proxy(None).await;
        let configured = http_proxy(None).await;
        for name in [
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
        ] {
            // SAFETY: sole test, single-threaded runtime, before blocking threads start; no concurrent environment readers.
            unsafe { std::env::set_var(name, trap.url("http")) };
        }
        // NO_PROXY excludes only the hidden name: loopback would hit the trap if environment proxies were used, while the hidden name proves NO_PROXY cannot bypass configuration.
        for name in ["NO_PROXY", "no_proxy"] {
            // SAFETY: same exclusive environment access as above.
            unsafe { std::env::set_var(name, HIDDEN_HOST) };
        }

        let tmp = tempfile::tempdir().unwrap();
        let core = Core::new(tmp.path().join("vault.sqlite3")).unwrap();
        core.create("fictional master password").await.unwrap();
        let draft = |name: &str, url: String| EnvironmentDraft {
            name: name.into(),
            url,
            database: "base-a".into(),
            login: "fixture-user".into(),
            protocol: Protocol::LegacyJsonrpc,
            context: json!({}),
            secret: "fixture-secret".into(),
        };
        let loopback = core
            .save_environment(draft("loopback", sim.url.clone()))
            .await
            .unwrap();
        let hidden = core
            .save_environment(draft("hidden", sim.url.replace("127.0.0.1", HIDDEN_HOST)))
            .await
            .unwrap();
        // No configured proxy means direct access despite HTTP_PROXY/ALL_PROXY traps.
        assert_eq!(
            core.check_environment(&loopback.id).await.unwrap().status,
            "ok"
        );
        let report = core.check_environment(&hidden.id).await.unwrap();
        assert_eq!(report.network_error.as_deref(), Some("dns"), "{report:?}");
        // Configured global proxy applies even to NO_PROXY targets.
        core.set_proxy(Some(ProxyInput {
            url: configured.url("http"),
            username: String::new(),
            password: String::new(),
        }))
        .await
        .unwrap();
        assert_eq!(
            core.check_environment(&hidden.id).await.unwrap().status,
            "ok"
        );
        assert_eq!(
            core.check_environment(&loopback.id).await.unwrap().status,
            "ok"
        );
        assert!(configured.log.connections() >= 2);
        assert_eq!(trap.log.connections(), 0, "{:?}", trap.log.entries());
    });
}
