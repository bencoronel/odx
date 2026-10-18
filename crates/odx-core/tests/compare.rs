//! Compare two simulated environments.
mod flow;
mod four_envs;
mod support;
use four_envs::fixture;
use odx_core::{Capability, ClientScope, Outcome};
use serde_json::{Value, json};

fn find<'a>(list: &'a Value, key: &str, field: &str) -> Option<&'a Value> {
    list.as_array()?
        .iter()
        .find(|r| r["key"] == json!(key) && r["field"] == json!(field))
}
fn codes(out: &odx_core::ToolOutput) -> Vec<String> {
    out.meta["warnings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|w| w["code"].as_str().map(String::from))
        .collect()
}
const FIELDS: [&str; 6] = [
    "name",
    "amount",
    "country_id",
    "tag_ids",
    "description",
    "only_a",
];

#[tokio::test]
async fn compare_by_xmlid_resolves_relations_by_stable_identity() {
    let f = fixture().await;
    let out = f
        .run(
            "compare",
            json!({"env_a":"sim-a","env_b":"sim-b","model":"account.tax","fields":FIELDS,"format":"json"}),
        )
        .await
        .unwrap();
    let d = &out.data;
    // Different many2one IDs/names with the same XML ID compare equal.
    assert!(find(&d["differences"], "l10n.tax21", "country_id").is_none());
    assert!(find(&d["differences"], "l10n.tax21", "amount").is_none());
    // Equal label counts with different identities compare unequal.
    assert_eq!(
        find(&d["differences"], "l10n.tax21", "tag_ids").unwrap()["env_b"],
        json!(["l10n.tag_a", "l10n.tag_c"])
    );
    assert_eq!(
        find(&d["differences"], "l10n.tax10", "amount").unwrap()["env_b"],
        10.5
    );
    // Missing XML ID in B makes the relation incomparable.
    assert!(find(&d["non_comparable"], "l10n.tax10", "tag_ids").is_some());
    assert!(find(&d["differences"], "l10n.tax10", "tag_ids").is_none());
    // Missing fields or incompatible field types.
    let fields: Vec<_> = d["non_comparable"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["key"].is_null())
        .map(|r| r["field"].as_str().unwrap())
        .collect();
    assert_eq!(fields, ["description", "only_a"]);
    // Preserve missing rows and rows without keys.
    let only_b = find(&d["differences"], "l10n.onlyb", "*").unwrap();
    assert_eq!(
        (only_b["env_a"].as_str(), only_b["certain"].as_bool()),
        (Some("absent"), Some(true))
    );
    assert_eq!(
        d["missing_keys"],
        json!([{"env":"env_a","id":3},{"env":"env_a","id":4}])
    );
    assert_eq!(d["scanned"], json!({"env_a":4,"env_b":3}));
    assert_eq!(out.meta["criteria"]["key"], "xmlid");
    let w = codes(&out);
    assert!(
        w.contains(&"separate_reads".into()) && w.contains(&"inconclusive".into()),
        "{w:?}"
    );
    four_envs::assert_no_secret(&f, &[&out]).await;
}

#[tokio::test]
async fn duplicated_keys_keep_the_rows_of_both_environments() {
    let f = fixture().await;
    let out = f
        .run(
            "compare",
            json!({"env_a":"sim-a","env_b":"sim-b","model":"account.tax","fields":["amount"],"key":"name"}),
        )
        .await
        .unwrap();
    let d = &out.data;
    // Duplicate VAT 10 in A (2 and 4); preserve B row 12 as incomparable too.
    assert_eq!(
        d["ambiguous"],
        json!([{"key":"VAT 10","ids_a":[2,4],"ids_b":[12]}])
    );
    assert!(find(&d["differences"], "VAT 10", "amount").is_none());
    assert_eq!(
        find(&d["differences"], "Manual", "*").unwrap()["env_b"],
        "absent"
    );
    assert_eq!(
        find(&d["differences"], "Only B", "*").unwrap()["env_a"],
        "absent"
    );
    assert!(find(&d["differences"], "VAT 21", "amount").is_none());
}

#[tokio::test]
async fn partial_pages_have_separate_continuations_and_no_certain_absence() {
    let f = fixture().await;
    let out = f
        .run(
            "compare",
            json!({"env_a":"sim-a","env_b":"sim-b","model":"account.tax","fields":["amount"],"key":"name","limit":1}),
        )
        .await
        .unwrap();
    let c = &out.meta["continuations"];
    assert_eq!(c["env_a"]["next_offset"], 1);
    assert_eq!(c["env_b"]["next_offset"], 1);
    assert!(out.meta.get("page").is_none());
    for r in out.data["differences"].as_array().unwrap() {
        if r["field"] == "*" {
            assert_eq!(r["certain"], false, "{r}");
        }
    }
    assert!(codes(&out).contains(&"inconclusive".into()));
}

#[tokio::test]
async fn invalid_keys_and_same_environment_are_rejected_before_odoo() {
    let f = fixture().await;
    let base = json!({"env_a":"sim-a","env_b":"sim-b","model":"account.tax","fields":["amount"]});
    let queries = f.sim.counts.queries();
    for (patch, code) in [
        (json!({"key":"id"}), "invalid_arguments"),
        (json!({"env_b":"sim-a"}), "invalid_arguments"),
        (json!({"fields":[]}), "invalid_arguments"),
        (json!({"key":"a b"}), "invalid_arguments"),
    ] {
        let mut args = base.clone();
        for (k, v) in patch.as_object().unwrap() {
            args[k] = v.clone();
        }
        let e = f.run("compare", args).await.unwrap_err();
        assert_eq!((e.code, e.outcome), (code, Outcome::NotSent), "{patch}");
    }
    assert_eq!(f.sim.counts.queries(), queries);
    // Detect relational/missing keys through Odoo metadata.
    for key in ["country_id", "nope"] {
        let mut args = base.clone();
        args["key"] = json!(key);
        let e = f.run("compare", args).await.unwrap_err();
        assert_eq!(e.code, "invalid_arguments", "{key}");
    }
}

#[tokio::test]
async fn each_environment_is_authorised_separately_before_any_call() {
    let f = fixture().await;
    let issued = f
        .core
        .create_client(
            "only-a",
            ClientScope::Only(vec![f.id("sim-a")]),
            vec![Capability::OdooRead],
        )
        .await
        .unwrap();
    let only_a = f.core.authenticate_client(&issued.token).await.unwrap();
    let queries = f.sim.counts.queries();
    for (a, b) in [("sim-a", "sim-b"), ("sim-b", "sim-a")] {
        let e = f
            .core
            .run_tool(
                &only_a,
                "compare",
                json!({"env_a":a,"env_b":b,"model":"account.tax","fields":["amount"]}),
            )
            .await
            .unwrap_err();
        assert_eq!(
            (e.code, e.outcome),
            ("environment_unavailable", Outcome::NotSent)
        );
    }
    assert_eq!(f.sim.counts.queries(), queries, "no partial reads from A");
}
