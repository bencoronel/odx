//! Desktop notifications for approvals, with an Open action on Linux.
use super::*;

/// Probe notification availability at startup without showing a notification.
pub fn probe_notifications(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        #[cfg(all(unix, not(target_os = "macos")))]
        let probe = tauri::async_runtime::spawn_blocking(|| {
            notify_rust::get_server_information().map(drop)
        })
        .await;
        #[cfg(not(all(unix, not(target_os = "macos"))))]
        let probe: Result<Result<(), String>, ()> = Ok(Ok(()));
        match probe {
            Ok(Ok(())) => {}
            Ok(Err(e)) => record_notification(&app, Some(e.to_string())),
            Err(_) => record_notification(&app, Some("availability check interrupted".into())),
        }
    });
}

pub(super) fn record_notification(app: &AppHandle, error: Option<String>) {
    if let Some(e) = &error {
        eprintln!("odx: notifications unavailable: {e}");
    }
    let changed = app.state::<Arc<Desktop>>().set_notifications(error);
    if changed {
        let _ = app.emit("desktop-status-changed", ());
    }
}

/// A slot for a thread blocked on a notification's action, released when it ends; bounds the threads that
/// notifications left open in the tray (GNOME keeps them until dismissed) can hold.
pub(super) struct NotificationWaiter(pub(super) AppHandle);

impl NotificationWaiter {
    const MAX: usize = 8;
    fn claim(app: &AppHandle) -> Option<Self> {
        let waiting = &app.state::<Arc<Desktop>>().notification_waiters;
        waiting
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < Self::MAX).then_some(n + 1)
            })
            .ok()
            .map(|_| Self(app.clone()))
    }
}

impl Drop for NotificationWaiter {
    fn drop(&mut self) {
        self.0
            .state::<Arc<Desktop>>()
            .notification_waiters
            .fetch_sub(1, Ordering::AcqRel);
    }
}

/// Notify new approval requests. Report failures in stderr and the window; requests remain visible in Approvals and the status bar.
/// A click on the notification (Linux "default" action) opens the window on `approval`.
pub fn notify(app: &AppHandle, title: String, body: String, approval: Option<String>) {
    #[cfg(target_os = "linux")]
    const ICON: &str = if cfg!(debug_assertions) {
        "icons/icon-dev.png"
    } else {
        "icons/icon.png"
    };
    #[cfg(target_os = "linux")]
    let icon = app
        .path()
        .resource_dir()
        .ok()
        .map(|dir| dir.join(ICON))
        .filter(|path| path.is_file())
        .unwrap_or_else(|| std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(ICON));
    // One blocking thread waits per notification with an action; past the cap, new ones show without "Open".
    let waiter =
        approval.and_then(|approval| NotificationWaiter::claim(app).map(|w| (w, approval)));
    let action = waiter.as_ref().map(|_| texts(app).open);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let shown = tauri::async_runtime::spawn_blocking(move || {
            let mut notification = notify_rust::Notification::new();
            notification
                .appname(crate::APP_NAME)
                .summary(&title)
                .body(&body);
            #[cfg(target_os = "linux")]
            notification.icon(&icon.to_string_lossy());
            #[cfg(all(unix, not(target_os = "macos")))]
            if let Some(label) = &action {
                notification.action("default", label);
            }
            notification.show().map_err(|e| e.to_string())
        })
        .await
        .unwrap_or_else(|_| Err("notification interrupted".into()));
        let handle = match shown {
            Ok(handle) => {
                record_notification(&app, None);
                handle
            }
            Err(error) => return record_notification(&app, Some(error)),
        };
        #[cfg(all(unix, not(target_os = "macos")))]
        if let Some((waiter, approval)) = waiter {
            let _ = tauri::async_runtime::spawn_blocking(move || {
                let _waiter = waiter;
                handle.wait_for_action(|action| {
                    if action == "default" {
                        *app.state::<Arc<Desktop>>().open_approval.lock().unwrap() = Some(approval);
                        show_main(&app);
                        let _ = app.emit("desktop-open-approval", ());
                    }
                });
            })
            .await;
        }
        #[cfg(not(all(unix, not(target_os = "macos"))))]
        let _ = (handle, waiter);
    });
}

/// Notify new requests. If notifications fail, show availability in the window; requests remain in Approvals and the status bar.
/// A burst shows its first request, then one summary when the cooldown ends; the tray icon is marked while any wait.
pub fn notify_approvals(
    app: tauri::AppHandle,
    core: Arc<odx_core::Core>,
    mut notices: tokio::sync::broadcast::Receiver<odx_core::ApprovalNotice>,
) {
    use std::time::Duration;
    use tauri::Emitter;
    use tokio::time::Instant;
    const COOLDOWN: Duration = Duration::from_secs(10);
    const ATTENTION_CHECK: Duration = Duration::from_secs(5);
    tauri::async_runtime::spawn(async move {
        let mut last_shown: Option<Instant> = None;
        // Requests that arrived during the cooldown, for the summary.
        let mut held: Vec<String> = Vec::new();
        let mut attention = false;
        let mut check = tokio::time::interval(ATTENTION_CHECK);
        loop {
            let summary_at = last_shown.map_or_else(Instant::now, |shown| shown + COOLDOWN);
            tokio::select! {
                received = notices.recv() => {
                    let notice = match received {
                        Ok(notice) => notice,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(_) => break,
                    };
                    let _ = app.emit("approvals-changed", &notice.approval_id);
                    if !attention {
                        attention = true;
                        set_tray_attention(&app, true);
                    }
                    if last_shown.is_some_and(|shown| shown.elapsed() < COOLDOWN) {
                        held.push(notice.approval_id.clone());
                    } else {
                        last_shown = Some(Instant::now());
                        notify_approval(&app, &notice);
                    }
                    if let Some(window) = app.get_webview_window("main") {
                        let _ = window
                            .request_user_attention(Some(tauri::UserAttentionType::Informational));
                    }
                }
                _ = tokio::time::sleep_until(summary_at), if !held.is_empty() => {
                    // Only held requests still waiting; decided, expired or already notified ones are no longer news.
                    let held = std::mem::take(&mut held);
                    let waiting = core.approvals().await.map_or(0, |pending| {
                        pending.iter().filter(|p| held.contains(&p.id)).count()
                    });
                    if waiting > 0 {
                        last_shown = Some(Instant::now());
                        notify_more(&app, waiting);
                    }
                }
                _ = check.tick() => {
                    let pending = core.approvals().await.is_ok_and(|pending| !pending.is_empty());
                    if pending != attention {
                        attention = pending;
                        set_tray_attention(&app, pending);
                    }
                }
            }
        }
    });
}

fn notify_approval(app: &tauri::AppHandle, notice: &odx_core::ApprovalNotice) {
    let texts = texts(app);
    let who = notice.client_label.as_deref().unwrap_or(&texts.a_client);
    let body = texts
        .request
        .replace("{who}", who)
        .replace("{method}", &notice.method)
        .replace("{model}", &notice.model)
        .replace("{environment}", &notice.environment_name);
    notify(
        app,
        texts.pending_approval,
        body,
        Some(notice.approval_id.clone()),
    );
}

fn notify_more(app: &tauri::AppHandle, count: usize) {
    let texts = texts(app);
    let body = if count == 1 {
        texts.more_one
    } else {
        texts.more_other.replace("{n}", &count.to_string())
    };
    notify(app, texts.pending_approvals, body, None);
}
