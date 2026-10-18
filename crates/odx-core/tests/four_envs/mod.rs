//! Shared fixtures with four simulated environments.
#![allow(dead_code)]
use crate::{flow, support};
use odx_core::{ClientIdentity, Core, EnvironmentDraft, Protocol, ToolError, ToolOutput};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};

pub struct Fixture {
    pub core: Arc<Core>,
    pub sim: support::ProtocolSimulator,
    pub client: ClientIdentity,
    /// Environment ID by name.
    pub ids: Vec<(String, String)>,
    pub tmp: tempfile::TempDir,
}
impl Fixture {
    pub async fn run(&self, tool: &str, args: Value) -> Result<ToolOutput, ToolError> {
        self.core.run_tool(&self.client, tool, args).await
    }
    pub fn id(&self, name: &str) -> String {
        self.ids.iter().find(|(n, _)| n == name).unwrap().1.clone()
    }
    pub fn outputs(&self) -> std::path::PathBuf {
        self.tmp.path().join("outputs")
    }
    /// Files present in the managed directory.
    pub fn files(&self) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(self.outputs())
            .map(|d| {
                d.map(|e| e.unwrap().file_name().into_string().unwrap())
                    .collect()
            })
            .unwrap_or_default();
        v.sort();
        v
    }
}

/// Legacy JSON-RPC fixtures: sim-a/sim-b (base-a/base-b), sim-16 and sim-19 with version-specific columns.
pub async fn fixture() -> Fixture {
    fixture_with(Protocol::LegacyJsonrpc).await
}

/// The same four databases via the selected protocol on the Odoo 19 simulator; fixture data is protocol-independent.
pub async fn fixture_with(protocol: Protocol) -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let sim = support::protocol_simulator(19).await;
    let core = Core::new(tmp.path().join("vault.sqlite3")).unwrap();
    core.create("fictional master password").await.unwrap();
    let mut ids = Vec::new();
    for (name, db) in [
        ("sim-a", "base-a"),
        ("sim-b", "base-b"),
        ("sim-16", "base-16"),
        ("sim-19", "base-19"),
    ] {
        let env = core
            .save_environment(EnvironmentDraft {
                name: name.into(),
                url: sim.url.clone(),
                database: db.into(),
                login: "fixture-user".into(),
                protocol,
                context: json!({"lang": "es_ES"}),
                secret: "fixture-secret".into(),
            })
            .await
            .unwrap();
        ids.push((name.to_string(), env.id));
    }
    let client = flow::client(&core).await;
    Fixture {
        core,
        sim,
        client,
        ids,
        tmp,
    }
}

pub async fn until(what: &str, mut ok: impl FnMut() -> bool) {
    for _ in 0..500 {
        if ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("condition not met: {what}");
}

/// Assert credentials are absent from results and activity.
pub async fn assert_no_secret(f: &Fixture, outputs: &[&ToolOutput]) {
    for out in outputs {
        let text = odx_core::output::render(out).text;
        assert!(!text.contains("fixture-secret"), "{text}");
        assert!(!out.envelope().to_string().contains("fixture-secret"));
    }
    let activity = f
        .core
        .activity(None, odx_core::ActivityFilter::default())
        .await
        .unwrap();
    let dump = format!("{:?}", activity.events);
    assert!(!dump.contains("fixture-secret"));
}
