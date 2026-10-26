//! Settings: core validation, encrypted persistence and live application. Fixed limits are not settings.
mod flow;
mod support;
use odx_core::{Core, Settings, WriteMode, error::OdxError};
use serde_json::json;

async fn core() -> (std::sync::Arc<Core>, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let core = support::unlocked_core(&tmp).await;
    (core, tmp)
}

#[tokio::test]
async fn defaults_are_pc07_and_out_of_range_values_are_rejected() {
    let (core, _tmp) = core().await;
    let view = core.settings().await.unwrap();
    assert_eq!(view.settings, Settings::default());
    let d = Settings::default();
    assert_eq!(
        (
            d.global_concurrency,
            d.environment_concurrency,
            d.upstream_timeout_secs,
            d.approval_minutes,
            d.activity_days,
            d.activity_mib,
            d.output_hours,
            d.output_mib
        ),
        (16, 4, 60, 5, 30, 256, 24, 512)
    );
    // Every range is finite and bounded.
    for (name, bound) in &view.bounds {
        assert!(bound.min >= 1 && bound.min <= bound.max, "{name}");
    }
    let invalid: Vec<fn(&mut Settings)> = vec![
        |s| s.global_concurrency = 0,
        |s| s.global_concurrency = 17,
        |s| s.environment_concurrency = 5,
        |s| {
            s.global_concurrency = 2;
            s.environment_concurrency = 3
        },
        |s| s.upstream_timeout_secs = 4,
        |s| s.upstream_timeout_secs = 61,
        |s| s.approval_minutes = 0,
        |s| s.approval_minutes = u32::MAX,
        |s| s.activity_days = 31,
        |s| s.activity_mib = 257,
        |s| s.output_hours = 25,
        |s| s.output_mib = 31,
    ];
    for change in invalid {
        let mut s = Settings::default();
        change(&mut s);
        assert!(matches!(
            core.save_settings(s).await,
            Err(OdxError::Validation {
                code: "validation.settings.out_of_range" | "validation.settings.concurrency_order",
                ..
            })
        ));
    }
    assert_eq!(core.settings().await.unwrap().settings, Settings::default());
    // Reject unknown fields from the window.
    assert!(serde_json::from_value::<Settings>(json!({"queue": 1000})).is_err());
}

#[tokio::test]
async fn settings_persist_encrypted_and_need_the_unlocked_vault() {
    let (core, tmp) = core().await;
    let s = Settings {
        global_concurrency: 8,
        environment_concurrency: 2,
        upstream_timeout_secs: 30,
        approval_minutes: 2,
        activity_days: 7,
        activity_mib: 64,
        output_hours: 6,
        output_mib: 128,
    };
    core.save_settings(s).await.unwrap();
    core.lock().await;
    assert!(matches!(core.settings().await, Err(OdxError::Locked)));
    assert!(matches!(core.save_settings(s).await, Err(OdxError::Locked)));
    let raw = std::fs::read(tmp.path().join("vault.sqlite3")).unwrap();
    assert!(
        !raw.windows(b"approval_minutes".len())
            .any(|w| w == b"approval_minutes"),
        "persist settings encrypted"
    );
    let reopened = Core::new(tmp.path().join("vault.sqlite3")).unwrap();
    reopened.unlock("fictional master password").await.unwrap();
    assert_eq!(reopened.settings().await.unwrap().settings, s);
}

#[tokio::test]
async fn approval_expiry_setting_applies_to_new_requests_only() {
    let (core, _tmp) = core().await;
    let sim = support::simulator().await;
    let env = core
        .save_environment(support::draft("A", &sim.url))
        .await
        .unwrap();
    core.set_mode(&env.id, WriteMode::Approval).await.unwrap();
    let client = flow::client(&core).await;
    let write = || json!({"env":"A","model":"res.partner","ids":[1],"values":{"name":"fictional"}});
    let (first_call, first) = flow::queue(&core, &client, "write", write()).await;
    core.save_settings(Settings {
        approval_minutes: 1,
        ..Default::default()
    })
    .await
    .unwrap();
    let (second_call, second) = flow::queue(&core, &client, "write", write()).await;
    let first_left = first.expires_at - chrono::Utc::now();
    let second_left = second.expires_at - chrono::Utc::now();
    assert!(
        first_left > chrono::Duration::minutes(4),
        "pending approvals retain their deadlines"
    );
    assert!(
        second_left <= chrono::Duration::minutes(1) && second_left > chrono::Duration::seconds(50),
        "new approval uses the setting: {second_left}"
    );
    flow::abandon(&core, first_call, &first.id).await;
    flow::abandon(&core, second_call, &second.id).await;
    assert_eq!(sim.counts.writes(), 0);
}
