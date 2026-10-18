//! Matrix: all 21 tools across legacy JSON-RPC, XML-RPC and JSON-2 against simulated Odoo 19. Cover success/error per tool/protocol, diagnostics, paging, truncation, to_file paths and secret redaction. Fixtures: tests/support/composite.rs and tests/four_envs/mod.rs::fixture_with.
mod flow;
mod four_envs;
mod support;
use odx_core::{Outcome, Protocol, ToolError, ToolOutput, WriteMode, output, tool_definitions};
use serde_json::{Value, json};

const PROTOCOLS: [Protocol; 3] = [Protocol::LegacyJsonrpc, Protocol::Xmlrpc, Protocol::Json2];
const DENIED: &str = "fixture access denied";

#[tokio::test]
async fn known_call_reads_are_bounded_and_preserve_native_results_on_all_protocols() {
    for protocol in PROTOCOLS {
        let f = four_envs::fixture_with(protocol).await;
        for bad in [
            json!({"method":"search_read","kwargs":{"limit":0}}),
            json!({"method":"search","kwargs":{"limit":1001}}),
            json!({"method":"search","kwargs":{"limit":false}}),
            json!({"method":"search_count","kwargs":{"domain":"[]"}}),
            json!({"method":"read","kwargs":{"ids":[1,1]}}),
            json!({"method":"read","kwargs":{"ids":(1..=1001).collect::<Vec<_>>()}}),
            json!({"method":"read","kwargs":{"ids":[1],"fields":[]}}),
            json!({"method":"search_read","args":[[]],"kwargs":{"domain":[]}}),
        ] {
            let mut args = bad.as_object().unwrap().clone();
            args.insert("env".into(), json!("sim-a"));
            args.insert("model".into(), json!("res.partner"));
            let err = f.run("call", Value::Object(args)).await.unwrap_err();
            assert_eq!(
                (err.code, err.outcome),
                ("invalid_arguments", Outcome::NotSent),
                "{protocol:?}: {bad}"
            );
        }
        assert_eq!(f.sim.counts.logins(), 0);
        assert_eq!(f.sim.counts.queries(), 0);
        for method in ["search", "search_read"] {
            let mut kwargs = json!({"limit":2,"offset":1});
            let model = if method == "search_read" {
                kwargs["fields"] = json!(["name"]);
                "account.tax"
            } else {
                "res.partner"
            };
            let mut typed = kwargs.as_object().unwrap().clone();
            typed.insert("env".into(), json!("sim-a"));
            typed.insert("model".into(), json!(model));
            let specific = f.run(method, Value::Object(typed)).await.unwrap();
            let generic = f
                .run(
                    "call",
                    json!({"env":"sim-a","model":model,"method":method,"kwargs":kwargs}),
                )
                .await
                .unwrap();
            assert_eq!(generic.data, specific.data, "{protocol:?} {method}");
            assert_eq!(generic.meta["page"], specific.meta["page"]);
        }
        let names = f.run("call", json!({"env":"sim-a","model":"res.partner","method":"name_search","kwargs":{"name":"Al","limit":2}})).await.unwrap();
        assert!(
            names.data[0].is_array(),
            "name_search preserves pairs instead of typed tool objects"
        );
        assert_eq!(names.data.as_array().unwrap().len(), 2);
        assert_eq!(names.meta["warnings"][0]["code"], "refine_query");
        f.run(
            "call",
            json!({"env":"sim-a","model":"account.tax","method":"search_read"}),
        )
        .await
        .unwrap();
        let activity = f
            .core
            .activity(None, odx_core::ActivityFilter::default())
            .await
            .unwrap();
        let requests: Vec<Value> = activity
            .events
            .iter()
            .filter_map(|e| e.details.as_ref()?.request.as_ref())
            .map(|p| serde_json::from_str(&p.text).unwrap())
            .collect();
        assert!(requests.iter().any(|r| r["method"] == "search_read"
            && r["kwargs"]["limit"] == 51
            && r["kwargs"]["fields"] == json!(["id", "display_name"])
            && r["kwargs"]["order"] == "id asc"));
    }
}

#[tokio::test]
async fn field_selection_and_totals_work_on_all_protocols() {
    for protocol in PROTOCOLS {
        let f = four_envs::fixture_with(protocol).await;
        for bad in [
            json!({"names":[]}),
            json!({"names":["name","name"]}),
            json!({"names":["not a field"]}),
        ] {
            let mut args = bad.as_object().unwrap().clone();
            args.insert("env".into(), json!("sim-a"));
            args.insert("model".into(), json!("account.tax"));
            assert_eq!(
                f.run("fields", Value::Object(args)).await.unwrap_err().code,
                "invalid_arguments"
            );
        }
        for fields in [json!([]), json!(["amount_total"])] {
            assert_eq!(
                f.run(
                    "read_group",
                    json!({"env":"sim-a","model":"res.partner","groupby":[],"fields":fields})
                )
                .await
                .unwrap_err()
                .code,
                "invalid_arguments"
            );
        }
        assert_eq!(f.sim.counts.queries(), 0);
        let selected = f
            .run(
                "fields",
                json!({"env":"sim-a","model":"account.tax","names":["name","amount"]}),
            )
            .await
            .unwrap();
        assert_eq!(
            selected
                .data
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r["name"].clone())
                .collect::<Vec<_>>(),
            vec![json!("amount"), json!("name")]
        );
        let filtered = f
            .run(
                "fields",
                json!({"env":"sim-a","model":"account.tax","names":["name","amount"],"q":"amount"}),
            )
            .await
            .unwrap();
        assert_eq!(filtered.data.as_array().unwrap().len(), 1);
        assert_eq!(filtered.data[0]["name"], "amount");
        let specific = f.run("read_group", json!({"env":"sim-a","model":"res.partner","groupby":[],"fields":["amount_total:sum"]})).await.unwrap();
        assert_eq!(specific.data, json!([{"count":15,"amount_total":150}]));
        let generic = f.run("call", json!({"env":"sim-a","model":"res.partner","method":"read_group","kwargs":{"groupby":[],"fields":["amount_total:sum"]}})).await.unwrap();
        assert_eq!(
            generic.data[0]["__count"], 15,
            "call preserves native field names"
        );
        assert_eq!(generic.data[0]["amount_total"], 150);
    }
}

struct Case {
    tool: &'static str,
    ok: Value,
    err: Value,
    /// Trigger AccessError on the first execute_kw call.
    fail_first: bool,
    code: &'static str,
    outcome: Outcome,
    /// Message fragment shared by all three protocols.
    message: &'static str,
}

fn case(tool: &'static str, ok: Value, err: Value) -> Case {
    Case {
        tool,
        ok,
        err,
        fail_first: false,
        code: "upstream_error",
        outcome: Outcome::Failed,
        message: "You are not allowed to access x.forbidden",
    }
}

/// Success/error per tool. sim-a allows permanent writes; sim-b defaults to read-only. x.forbidden always raises AccessError.
fn cases() -> Vec<Case> {
    let not_sent = |mut c: Case, code, message| {
        c.code = code;
        c.outcome = Outcome::NotSent;
        c.message = message;
        c
    };
    let denied = |mut c: Case| {
        c.fail_first = true;
        c.message = DENIED;
        c
    };
    vec![
        not_sent(
            case("envs", json!({}), json!({"path": "/tmp"})),
            "invalid_arguments",
            "unknown field: path",
        ),
        not_sent(
            case("whoami", json!({"env":"sim-a"}), json!({"env":"nope"})),
            "environment_unavailable",
            "environment unavailable",
        ),
        case(
            "search_read",
            json!({"env":"sim-a","model":"res.partner","fields":["display_name"]}),
            json!({"env":"sim-a","model":"x.forbidden"}),
        ),
        case(
            "read",
            json!({"env":"sim-a","model":"res.partner","ids":[1,9],"fields":["display_name"]}),
            json!({"env":"sim-a","model":"x.forbidden","ids":[1]}),
        ),
        case(
            "search",
            json!({"env":"sim-a","model":"res.partner","limit":2}),
            json!({"env":"sim-a","model":"x.forbidden"}),
        ),
        case(
            "search_count",
            json!({"env":"sim-a","model":"res.partner"}),
            json!({"env":"sim-a","model":"x.forbidden"}),
        ),
        case(
            "wait",
            json!({"env":"sim-a","model":"x.queue","domain":[["create_date","=",u64::MAX / 2]]}),
            json!({"env":"sim-a","model":"x.forbidden","domain":[]}),
        ),
        case(
            "read_group",
            json!({"env":"sim-a","model":"res.partner","groupby":["parent_id"],"fields":["amount_total:sum"],"limit":2}),
            json!({"env":"sim-a","model":"x.forbidden","groupby":["parent_id"]}),
        ),
        case(
            "fields",
            json!({"env":"sim-a","model":"res.partner","q":"mail"}),
            json!({"env":"sim-a","model":"x.forbidden"}),
        ),
        denied(case(
            "models",
            json!({"env":"sim-a","q":"res."}),
            json!({"env":"sim-a","q":"res."}),
        )),
        case(
            "name_search",
            json!({"env":"sim-a","model":"res.partner","name":"Al","limit":2}),
            json!({"env":"sim-a","model":"x.forbidden","name":"Al"}),
        ),
        Case {
            code: "not_found",
            message: "XML ID base.missing not found",
            ..case(
                "ref",
                json!({"env":"sim-a","xmlid":"base.main_company"}),
                json!({"env":"sim-a","xmlid":"base.missing"}),
            )
        },
        // history fails when the first message read is denied.
        denied(case(
            "history",
            json!({"env":"sim-a","model":"res.partner","id":1}),
            json!({"env":"sim-a","model":"res.partner","id":1}),
        )),
        Case {
            message: "View types not defined",
            ..case(
                "view",
                json!({"env":"sim-a","model":"sale.order"}),
                json!({"env":"sim-a","model":"x.unknown"}),
            )
        },
        // access converts Odoo denials to null plus warnings; its error occurs earlier.
        not_sent(
            case(
                "access",
                json!({"env":"sim-a","model":"res.partner"}),
                json!({"env":"nope","model":"res.partner"}),
            ),
            "environment_unavailable",
            "environment unavailable",
        ),
        not_sent(
            case(
                "compare",
                json!({"env_a":"sim-a","env_b":"sim-b","model":"account.tax","key":"name","fields":["amount"]}),
                json!({"env_a":"sim-a","env_b":"sim-a","model":"account.tax","fields":["amount"]}),
            ),
            "invalid_arguments",
            "must be distinct environments",
        ),
        Case {
            code: "not_found",
            message: "is empty; no file created",
            ..case(
                "download",
                json!({"env":"sim-a","model":"ir.attachment","id":9}),
                json!({"env":"sim-a","model":"ir.attachment","id":10}),
            )
        },
        case(
            "create",
            json!({"env":"sim-a","model":"res.partner","values":[{"name":"x"},{"name":"y"}]}),
            json!({"env":"sim-a","model":"x.forbidden","values":{"name":"x"}}),
        ),
        case(
            "write",
            json!({"env":"sim-a","model":"res.partner","ids":[1],"values":{"name":"y"}}),
            json!({"env":"sim-a","model":"x.forbidden","ids":[1],"values":{"name":"y"}}),
        ),
        not_sent(
            case(
                "unlink",
                json!({"env":"sim-a","model":"res.partner","ids":[1]}),
                json!({"env":"sim-b","model":"res.partner","ids":[1]}),
            ),
            "forbidden",
            "read-only",
        ),
        case(
            "call",
            json!({"env":"sim-a","model":"res.partner","method":"search_count","kwargs":{"domain":[]}}),
            json!({"env":"sim-a","model":"x.forbidden","method":"search_count","kwargs":{"domain":[]}}),
        ),
    ]
}

async fn run(f: &four_envs::Fixture, tool: &str, args: Value) -> Result<ToolOutput, ToolError> {
    if tool != "unlink" || args["env"] != "sim-a" {
        return f.run(tool, args).await;
    }
    // unlink requires individual approval in every write-enabled mode.
    let (call, pending) = flow::queue(&f.core, &f.client, tool, args).await;
    f.core.decide(&pending.id, true).await.unwrap();
    call.await.unwrap()
}

/// Normalize timestamps, output files and envs protocol metadata for cross-protocol comparison.
fn comparable(tool: &str, out: &ToolOutput) -> Value {
    let mut data = out.data.clone();
    match tool {
        "wait" => data["elapsed_seconds"] = Value::Null,
        "download" => {
            for k in ["file_id", "path", "expires_at"] {
                data[k] = Value::Null;
            }
        }
        "envs" => {
            for env in data.as_array_mut().unwrap() {
                env["protocol"] = Value::Null;
            }
        }
        _ => {}
    }
    json!({"data": data, "page": out.meta.get("page"), "continuations": out.meta.get("continuations"),
           "warnings": out.meta.get("warnings"), "missing": out.meta.get("missing_ids")})
}

#[tokio::test]
async fn every_tool_succeeds_and_fails_alike_on_every_protocol() {
    let names: Vec<&str> = tool_definitions().iter().map(|d| d.name).collect();
    let cases = cases();
    let mut tools: Vec<&str> = cases.iter().map(|c| c.tool).collect();
    tools.sort();
    assert_eq!(tools, names, "one row per catalog tool");
    assert_eq!(names.len(), 21);

    let mut by_protocol: Vec<Vec<Value>> = vec![];
    for protocol in PROTOCOLS {
        let f = four_envs::fixture_with(protocol).await;
        f.core
            .set_mode(&f.id("sim-a"), WriteMode::Permanent)
            .await
            .unwrap();
        let mut outputs = vec![];
        let mut row = vec![];
        for c in &cases {
            let out = run(&f, c.tool, c.ok.clone())
                .await
                .unwrap_or_else(|e| panic!("{protocol:?} {}: {e:?}", c.tool));
            row.push(comparable(c.tool, &out));
            outputs.push(out);

            let queries = f.sim.counts.queries();
            if c.fail_first {
                f.sim.counts.fail_next_execute(DENIED);
            }
            let e = run(&f, c.tool, c.err.clone()).await.unwrap_err();
            assert_eq!(
                (e.code, e.outcome),
                (c.code, c.outcome),
                "{protocol:?} {}: {e:?}",
                c.tool
            );
            assert!(
                e.message.contains(c.message),
                "{protocol:?} {}: {}",
                c.tool,
                e.message
            );
            assert!(!format!("{e:?}").contains("fixture-secret"));
            if c.outcome == Outcome::NotSent {
                assert_eq!(f.sim.counts.queries(), queries, "{protocol:?} {}", c.tool);
            }
        }
        // Only successful download creates a file; errors create none.
        assert_eq!(f.files().len(), 1, "{protocol:?}: {:?}", f.files());
        let refs: Vec<&ToolOutput> = outputs.iter().collect();
        four_envs::assert_no_secret(&f, &refs).await;
        // Only JSON-2's Bearer header carries the credential.
        for h in f.sim.counts.headers() {
            for (name, value) in &h {
                if name != "authorization" {
                    assert!(!value.to_str().unwrap_or("").contains("fixture-secret"));
                }
            }
        }
        by_protocol.push(row);
    }
    assert_eq!(
        by_protocol[0], by_protocol[1],
        "XML-RPC differs from JSON-RPC"
    );
    assert_eq!(
        by_protocol[0], by_protocol[2],
        "JSON-2 differs from JSON-RPC"
    );
}

#[tokio::test]
async fn protocol_specific_diagnostics_are_fixed() {
    for protocol in PROTOCOLS {
        let f = four_envs::fixture_with(protocol).await;

        // JSON-2 call rejects positional arguments instead of inferring names.
        let queries = f.sim.counts.queries();
        let positional = f
            .run(
                "call",
                json!({"env":"sim-a","model":"res.partner","method":"search_count","args":[[]]}),
            )
            .await;
        match protocol {
            Protocol::Json2 => {
                let e = positional.unwrap_err();
                assert_eq!((e.code, e.outcome), ("invalid_arguments", Outcome::NotSent));
                assert!(e.message.contains("named arguments"), "{}", e.message);
                assert_eq!(f.sim.counts.queries(), queries);
            }
            _ => assert_eq!(positional.unwrap().data, 5, "{protocol:?}"),
        }

        // XML-RPC cannot serialize get_external_id's integer dictionary keys.
        let by_xmlid = f
            .run(
                "compare",
                json!({"env_a":"sim-a","env_b":"sim-b","model":"account.tax","fields":["amount"]}),
            )
            .await;
        // Relations keyed by XML ID remain incomparable over XML-RPC.
        let relations = f
            .run(
                "compare",
                json!({"env_a":"sim-a","env_b":"sim-b","model":"account.tax","key":"name","fields":["tag_ids"]}),
            )
            .await
            .unwrap();
        let tags = relations.data["differences"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["field"] == "tag_ids");
        let non_comparable = relations.data["non_comparable"].as_array().unwrap();
        if protocol == Protocol::Xmlrpc {
            let e = by_xmlid.unwrap_err();
            assert_eq!(
                (e.code, e.outcome, e.http_status()),
                ("incompatible_protocol", Outcome::NotSent, 409)
            );
            assert!(e.message.contains("get_external_id"), "{}", e.message);
            assert!(!tags, "without XML IDs, relation differences are unknown");
            assert!(non_comparable.iter().any(|n| n["field"] == "tag_ids"));
            let warnings = relations.meta["warnings"].to_string();
            assert!(warnings.contains("relations_unresolved"), "{warnings}");
            // Never send get_external_id over XML-RPC.
            assert!(
                f.sim
                    .requests
                    .lock()
                    .unwrap()
                    .iter()
                    .all(|b| !b.contains("get_external_id"))
            );
        } else {
            let out = by_xmlid.unwrap();
            assert_eq!(out.meta["criteria"]["key"], "xmlid");
            assert!(!out.data["differences"].as_array().unwrap().is_empty());
            assert!(tags, "{protocol:?}: {}", relations.envelope());
            assert!(non_comparable.is_empty(), "{protocol:?}");
        }
    }
}

#[tokio::test]
async fn pagination_and_truncation_on_every_protocol() {
    for protocol in PROTOCOLS {
        let f = four_envs::fixture_with(protocol).await;
        let second = f
            .run(
                "search",
                json!({"env":"sim-a","model":"res.partner","limit":2,"offset":2}),
            )
            .await
            .unwrap();
        assert_eq!(second.data, json!([3, 4]), "{protocol:?}");
        assert_eq!(
            second.meta["page"],
            json!({"limit":2,"offset":2,"returned":2,"has_more":true,"next_offset":4})
        );
        let last = f
            .run(
                "read_group",
                json!({"env":"sim-a","model":"res.partner","groupby":["parent_id"],"limit":2,"offset":4}),
            )
            .await
            .unwrap();
        assert_eq!(last.meta["page"]["has_more"], false, "{protocol:?}");
        assert_eq!(last.data.as_array().unwrap().len(), 1);
        // Truncate 1000 × 1000-character rows to 256 KiB using marked whole-row cuts.
        let big = f
            .run(
                "search_read",
                json!({"env":"sim-a","model":"limits.big","limit":1000,"trunc":0}),
            )
            .await
            .unwrap();
        let (fitted, rendered) = output::fit(big).unwrap();
        let returned = fitted.data.as_array().unwrap().len();
        assert!(returned > 0 && returned < 1000, "{protocol:?} {returned}");
        assert!(rendered.text.len() <= 256 * 1024);
        assert_eq!(fitted.meta["truncated"], true);
        assert_eq!(fitted.meta["page"]["next_offset"], returned);
        // trunc affects compact text; JSON preserves full values.
        let text = f
            .run(
                "search_read",
                json!({"env":"sim-a","model":"limits.big","limit":1,"trunc":10}),
            )
            .await
            .unwrap();
        assert!(!output::render(&text).text.contains(&"x".repeat(11)));
        let json_out = f
            .run(
                "search_read",
                json!({"env":"sim-a","model":"limits.big","limit":1,"trunc":10,"format":"json"}),
            )
            .await
            .unwrap();
        assert_eq!(json_out.data[0]["display_name"], "x".repeat(1000));
    }
}

#[tokio::test]
async fn to_file_paths_are_chosen_by_the_server_on_every_protocol() {
    for protocol in PROTOCOLS {
        let f = four_envs::fixture_with(protocol).await;
        std::fs::create_dir_all(f.outputs()).unwrap();
        let foreign = f.outputs().join("notes.txt");
        std::fs::write(&foreign, "ajeno").unwrap();
        // Client-supplied paths and filenames are unknown fields.
        for extra in [
            json!({"path":"/tmp/x.tsv"}),
            json!({"file":"../../x"}),
            json!({"dir":"/tmp"}),
            json!({"filename":"notes.txt"}),
            json!({"to_file":"../x.tsv"}),
        ] {
            let mut args = json!({"env":"sim-a","model":"res.partner","to_file":true});
            for (k, v) in extra.as_object().unwrap() {
                args[k] = v.clone();
            }
            let e = f.run("search_read", args).await.unwrap_err();
            assert_eq!(e.code, "invalid_arguments", "{protocol:?} {extra}");
        }
        assert_eq!(f.files(), vec!["notes.txt".to_string()], "{protocol:?}");
        for (tool, args, ext) in [
            (
                "search_read",
                json!({"env":"sim-a","model":"res.partner","to_file":true}),
                ".tsv",
            ),
            (
                "call",
                json!({"env":"sim-a","model":"res.partner","method":"search_count","kwargs":{"domain":[]},"to_file":true}),
                ".json",
            ),
            (
                "download",
                json!({"env":"sim-a","model":"ir.attachment","id":9}),
                "_.._etc_passwd",
            ),
        ] {
            let out = f.run(tool, args).await.unwrap();
            let path = std::path::PathBuf::from(out.data["path"].as_str().unwrap());
            assert_eq!(path.parent().unwrap(), f.outputs(), "{protocol:?} {tool}");
            assert!(
                path.to_str().unwrap().ends_with(ext),
                "{protocol:?} {tool}: {path:?}"
            );
            four_envs::assert_no_secret(&f, &[&out]).await;
            let body = std::fs::read(&path).unwrap();
            assert!(!String::from_utf8_lossy(&body).contains("fixture-secret"));
        }
        assert_eq!(std::fs::read_to_string(&foreign).unwrap(), "ajeno");
        assert_eq!(f.files().len(), 4, "{protocol:?}");
    }
}
