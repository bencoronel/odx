//! download/to_file server-selected paths, limits and atomic publication.
mod flow;
mod four_envs;
mod support;
use four_envs::fixture;
use odx_core::Protocol;
use serde_json::{Value, json};
use std::os::unix::fs::PermissionsExt;

fn descriptor_path(out: &odx_core::ToolOutput) -> std::path::PathBuf {
    out.data["path"].as_str().unwrap().into()
}

#[tokio::test]
async fn download_saves_the_binary_without_base64_in_the_answer() {
    let f = fixture().await;
    let out = f
        .run(
            "download",
            json!({"env":"sim-a","model":"ir.attachment","id":9}),
        )
        .await
        .unwrap();
    let path = descriptor_path(&out);
    assert_eq!(path.parent().unwrap(), f.outputs(), "server-selected path");
    // Treat ../../etc/passwd attachment names as sanitized hints only.
    let name = path.file_name().unwrap().to_str().unwrap();
    assert!(name.ends_with("-_.._etc_passwd"), "{name}");
    assert_eq!(std::fs::read(&path).unwrap(), b"%PDF fixture\0\x01");
    assert_eq!(out.data["bytes"], 14);
    assert_eq!(out.data["mimetype"], "application/pdf");
    assert!(out.data["expires_at"].is_string() && out.data["file_id"].is_string());
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    let text = odx_core::output::render(&out).text;
    assert!(!text.contains("JVBER") && !out.envelope().to_string().contains("JVBER"));
    four_envs::assert_no_secret(&f, &[&out]).await;
}

#[tokio::test]
async fn download_errors_create_no_file() {
    let f = fixture().await;
    for (args, code) in [
        (json!({"id":10}), "not_found"),
        (json!({"id":404}), "not_found"),
        (json!({"id":11}), "upstream_error"),
        (json!({"id":9,"field":"name"}), "invalid_arguments"),
        (json!({"id":9,"field":"missing"}), "not_found"),
        (json!({"id":9,"path":"/tmp/x"}), "invalid_arguments"),
        (json!({"id":0}), "invalid_arguments"),
    ] {
        let mut full = json!({"env":"sim-a","model":"ir.attachment"});
        for (k, v) in args.as_object().unwrap() {
            full[k] = v.clone();
        }
        let e = f.run("download", full).await.unwrap_err();
        assert_eq!(e.code, code, "{args}");
        assert!(f.files().is_empty(), "{args}: {:?}", f.files());
    }
}

const PROTOCOLS: [Protocol; 3] = [Protocol::LegacyJsonrpc, Protocol::Xmlrpc, Protocol::Json2];
const MIB: u64 = 1024 * 1024;

#[tokio::test]
async fn download_reaches_32_mib_on_every_protocol() {
    // Only binary reads allow 48 MiB encoded plus envelope.
    for protocol in PROTOCOLS {
        let f = four_envs::fixture_with(protocol).await;
        for (id, mib) in [(12, 13), (13, 30)] {
            let out = f
                .run(
                    "download",
                    json!({"env":"sim-a","model":"ir.attachment","id":id}),
                )
                .await
                .unwrap_or_else(|e| panic!("{protocol:?} {id}: {e:?}"));
            // Generate base64 from mib·MiB/3 groups of three bytes.
            let expected = mib * MIB / 3 * 3;
            assert_eq!(out.data["bytes"], expected, "{protocol:?} {id}");
            let path = descriptor_path(&out);
            assert_eq!(std::fs::metadata(&path).unwrap().len(), expected);
            std::fs::remove_file(path).unwrap();
        }
    }
}

#[tokio::test]
async fn download_over_the_binary_limits_fails_clearly_without_file() {
    for protocol in PROTOCOLS {
        let f = four_envs::fixture_with(protocol).await;
        // 33 MiB decoded (44 MiB base64) and 50 MiB encoded.
        for (id, limit) in [(14, "33554432"), (15, "48 MiB")] {
            let e = f
                .run(
                    "download",
                    json!({"env":"sim-a","model":"ir.attachment","id":id}),
                )
                .await
                .unwrap_err();
            assert_eq!(e.code, "output_too_large", "{protocol:?} {id}: {e:?}");
            assert_eq!(e.http_status(), 413);
            assert_eq!(e.outcome, odx_core::Outcome::Failed);
            assert!(
                e.message.contains(limit),
                "{protocol:?} {id}: {}",
                e.message
            );
            assert!(f.files().is_empty(), "{protocol:?} {id}: {:?}", f.files());
        }
    }
}

#[tokio::test]
async fn ordinary_reads_keep_the_16_mib_response_limit() {
    // The same 13 MiB binary (17.3 MiB base64) that download can save.
    for protocol in PROTOCOLS {
        let f = four_envs::fixture_with(protocol).await;
        let e = f
            .run(
                "read",
                json!({"env":"sim-a","model":"ir.attachment","ids":[12],"fields":["datas"]}),
            )
            .await
            .unwrap_err();
        assert_eq!(e.code, "output_too_large", "{protocol:?}: {e:?}");
        assert_eq!(e.outcome, odx_core::Outcome::Failed);
        assert!(e.message.contains("16 MiB"), "{protocol:?}: {}", e.message);
    }
}

#[tokio::test]
async fn to_file_saves_only_the_requested_page_in_its_format() {
    let f = fixture().await;
    let cases: [(&str, Value, &str); 6] = [
        (
            "search_read",
            json!({"env":"sim-a","model":"res.partner","limit":1}),
            ".tsv",
        ),
        (
            "read_group",
            json!({"env":"sim-a","model":"res.partner","groupby":["parent_id"],"limit":2}),
            ".tsv",
        ),
        (
            "history",
            json!({"env":"sim-a","model":"res.partner","id":1}),
            ".tsv",
        ),
        ("view", json!({"env":"sim-a","model":"sale.order"}), ".xml"),
        (
            "compare",
            json!({"env_a":"sim-a","env_b":"sim-b","model":"account.tax","fields":["amount"]}),
            ".tsv",
        ),
        (
            "call",
            json!({"env":"sim-a","model":"res.partner","method":"read_group","kwargs":{"domain":[],"fields":[],"groupby":["parent_id"],"lazy":false}}),
            ".json",
        ),
    ];
    for (tool, mut args, ext) in cases {
        args["to_file"] = json!(true);
        let out = f.run(tool, args).await.unwrap();
        assert!(out.data["path"].is_string(), "{tool}: {}", out.envelope());
        let path = descriptor_path(&out);
        assert!(path.to_str().unwrap().ends_with(ext), "{tool}: {path:?}");
        let body = std::fs::read_to_string(&path).unwrap();
        assert_eq!(out.data["bytes"], body.len(), "{tool}");
        match tool {
            "read_group" => {
                // Header plus two page rows, excluding the continuation row.
                assert_eq!(body.lines().count(), 3, "{body}");
                assert_eq!(out.data["rows"], 2);
                assert_eq!(out.meta["page"]["has_more"], true);
            }
            "view" => assert!(body.starts_with("<form>") && body.contains("x_foo")),
            "history" => assert!(body.contains("Draft") && body.contains("Done")),
            "call" => assert!(serde_json::from_str::<Value>(&body).unwrap().is_array()),
            _ => {}
        }
        // Content stays in the file, not the response.
        assert!(out.data.get("arch").is_none() && !out.envelope().to_string().contains("<field"));
    }
    assert_eq!(f.files().len(), 6);
    assert!(f.files().iter().all(|n| !n.ends_with(".part")));
}

#[tokio::test]
async fn a_symlinked_output_directory_is_refused() {
    let f = fixture().await;
    let elsewhere = f.tmp.path().join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    std::os::unix::fs::symlink(&elsewhere, f.outputs()).unwrap();
    let e = f
        .run(
            "download",
            json!({"env":"sim-a","model":"ir.attachment","id":9}),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code, "output_storage_error");
    assert_eq!(e.http_status(), 507);
    assert_eq!(std::fs::read_dir(&elsewhere).unwrap().count(), 0);
}
