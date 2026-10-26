//! Desktop integration: single instance, tray, close behavior, graceful shutdown and availability notices.
use odx_core::Core;
use serde::{Deserialize, Serialize};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use tauri::{AppHandle, Emitter, Manager};
use zeroize::Zeroizing;

/// Deadline for draining sent operations on exit.
mod inbox;
mod linux;
mod notify;
mod power;
mod tray;
mod window;
pub use inbox::*;
pub use linux::*;
pub use notify::*;
pub use power::*;
pub use tray::*;
pub use window::*;

pub const EXIT_DRAIN: std::time::Duration = std::time::Duration::from_secs(10);
/// Longest wait for the lock on quit, which includes the window's Terminal flush.
pub const EXIT_LOCK: std::time::Duration = std::time::Duration::from_secs(5);
/// Deadline for WebKit to report a suspended window as destroyed.
const DESTROY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Select startup mode from arguments; the connector bypasses single-instance handling.
#[derive(Debug, PartialEq, Eq)]
pub enum Launch {
    Connector,
    Gui,
}
pub fn launch_mode(mut args: impl Iterator<Item = String>) -> Launch {
    match args.nth(1).as_deref() {
        Some("mcp-stdio") => Launch::Connector,
        _ => Launch::Gui,
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum CloseAction {
    /// Save the UI and unload its window without changing service state.
    Suspend,
    /// Exit instead of hiding when the tray is unavailable.
    Close,
}
pub fn close_action(tray_usable: bool) -> CloseAction {
    if tray_usable {
        CloseAction::Suspend
    } else {
        CloseAction::Close
    }
}

#[derive(Default)]
pub struct Desktop {
    tray_error: Mutex<Option<String>>,
    tray_usable: AtomicBool,
    notifications_error: Mutex<Option<String>>,
    exit: ExitGate,
    window: Mutex<WindowSession>,
    /// Tray and notification texts in the window's language.
    texts: Mutex<DesktopTexts>,
    /// Approval whose notification was clicked, until the window takes it.
    open_approval: Mutex<Option<String>>,
    /// Agent requests (run a Terminal command, show a tab) waiting for the window, and their answers.
    agent: Mutex<AgentQueue>,
    /// Terminal tables opened in their own windows, by window label, in memory until that window takes them.
    tables: Mutex<(u64, std::collections::HashMap<String, serde_json::Value>)>,
    /// Threads blocked on a notification's action (`NotificationWaiter`).
    notification_waiters: std::sync::atomic::AtomicUsize,
    /// Lock waiting for the window to save pending Terminal changes.
    flush: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    /// One lock at a time: sleep and screen lock fire together, and a second flush request would cut the first short.
    locking: tokio::sync::Mutex<()>,
}

/// Text Rust shows itself (tray, notifications). The window owns the translations and sends them in its
/// language (`desktop_set_texts`); English until then. `{who}`, `{method}`, `{model}`, `{environment}`
/// and `{n}` are filled in here.
#[derive(Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct DesktopTexts {
    pub open: String,
    pub lock: String,
    pub quit: String,
    pub a_client: String,
    pub pending_approval: String,
    pub pending_approvals: String,
    pub request: String,
    pub more_one: String,
    pub more_other: String,
}
impl Default for DesktopTexts {
    fn default() -> Self {
        Self {
            open: "Open".into(),
            lock: "Lock".into(),
            quit: "Quit".into(),
            a_client: "A client".into(),
            pending_approval: "Pending approval".into(),
            pending_approvals: "Pending approvals".into(),
            request: "{who} requests {method} on {model} · {environment}".into(),
            more_one: "1 more approval is pending".into(),
            more_other: "{n} more approvals are pending".into(),
        }
    }
}

pub fn texts(app: &AppHandle) -> DesktopTexts {
    app.state::<Arc<Desktop>>().texts.lock().unwrap().clone()
}

/// Take the window's texts and relabel the tray menu when they change.
pub fn set_texts(app: &AppHandle, texts: DesktopTexts) {
    {
        let desktop = app.state::<Arc<Desktop>>();
        let mut current = desktop.texts.lock().unwrap();
        if *current == texts {
            return;
        }
        *current = texts;
    }
    if let (Some(tray), Ok(menu)) = (app.tray_by_id("main"), tray_menu(app)) {
        let _ = tray.set_menu(Some(menu));
    }
}

#[derive(Serialize, Debug, PartialEq)]
pub struct DesktopStatus {
    pub tray: bool,
    pub tray_error: Option<String>,
    pub notifications_error: Option<String>,
}

impl Desktop {
    pub fn tray_usable(&self) -> bool {
        self.tray_usable.load(Ordering::Acquire)
    }
    fn set_tray(&self, result: Result<(), String>) {
        self.tray_usable.store(result.is_ok(), Ordering::Release);
        *self.tray_error.lock().unwrap() = result.err();
    }
    /// Report state changes to notify the window.
    fn set_notifications(&self, error: Option<String>) -> bool {
        let mut current = self.notifications_error.lock().unwrap();
        let changed = *current != error;
        *current = error;
        changed
    }
    pub fn status(&self) -> DesktopStatus {
        DesktopStatus {
            tray: self.tray_usable(),
            tray_error: self.tray_error.lock().unwrap().clone(),
            notifications_error: self.notifications_error.lock().unwrap().clone(),
        }
    }
}

/// Hold the first exit request to lock and drain; allow only the sequence's final exit.
#[derive(Default)]
pub struct ExitGate {
    started: AtomicBool,
    drained: AtomicBool,
}
impl ExitGate {
    /// Only the first caller starts shutdown.
    pub fn begin(&self) -> bool {
        !self.started.swap(true, Ordering::AcqRel)
    }
    pub fn finish(&self) {
        self.drained.store(true, Ordering::Release);
    }
    /// Hold exit until locking and draining finish.
    pub fn must_hold(&self) -> bool {
        !self.drained.load(Ordering::Acquire)
    }
}

/// A snapshot stays in memory only and is invalidated whenever the vault locks.
#[derive(Default)]
struct WindowSession {
    snapshot: Option<Zeroizing<String>>,
    epoch: u64,
    ticket: u64,
    pending: Option<(u64, u64)>,
    destroying: bool,
    creating: bool,
    reopen: bool,
    bounds: Option<(tauri::PhysicalPosition<i32>, tauri::PhysicalSize<u32>)>,
    maximized: bool,
    /// Maximize the main window once it shows (see `window::present`).
    maximize_on_show: bool,
}
impl WindowSession {
    fn remember_bounds(
        &mut self,
        position: tauri::PhysicalPosition<i32>,
        size: tauri::PhysicalSize<u32>,
        maximized: bool,
    ) {
        if self.creating {
            return;
        }
        self.maximized = maximized;
        if !maximized {
            self.bounds = Some((position, size));
        }
    }

    fn request(&mut self) -> Option<u64> {
        if self.pending.is_some() || self.destroying {
            return None;
        }
        self.ticket = self.ticket.wrapping_add(1);
        self.pending = Some((self.ticket, self.epoch));
        Some(self.ticket)
    }
    fn save(&mut self, ticket: u64, snapshot: String, unlocked: bool) -> bool {
        let Some((pending, epoch)) = self.pending else {
            return false;
        };
        if pending != ticket {
            return false;
        }
        self.pending = None;
        self.snapshot = (unlocked && epoch == self.epoch).then(|| Zeroizing::new(snapshot));
        self.destroying = true;
        true
    }
    /// The suspension of `ticket` is still waiting for `WindowEvent::Destroyed`.
    fn stalled(&self, ticket: u64) -> bool {
        self.destroying && self.ticket == ticket
    }
    fn clear(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        self.snapshot = None;
    }
}

pub fn clear_window_state(app: &AppHandle) {
    app.state::<Arc<Desktop>>().window.lock().unwrap().clear();
}

pub fn pending_suspend(app: &AppHandle) -> Option<u64> {
    app.state::<Arc<Desktop>>()
        .window
        .lock()
        .unwrap()
        .pending
        .map(|(ticket, _)| ticket)
}

pub fn restore_window_state(app: &AppHandle, unlocked: bool) -> Option<String> {
    let desktop = app.state::<Arc<Desktop>>();
    let mut state = desktop.window.lock().unwrap();
    if !unlocked {
        state.clear();
    }
    state.snapshot.take().map(|snapshot| snapshot.to_string())
}

/// Show the existing window, cancel a pending suspension, or recreate its WebView.
pub fn show_main(app: &AppHandle) {
    let desktop = app.state::<Arc<Desktop>>();
    {
        let mut state = desktop.window.lock().unwrap();
        if state.destroying {
            state.reopen = true;
            return;
        }
        state.pending = None;
        if state.creating {
            return;
        }
        state.creating = true;
    }
    let result = (|| -> tauri::Result<()> {
        let (window, created) = match app.get_webview_window("main") {
            Some(window) => (window, false),
            None => {
                let config = app
                    .config()
                    .app
                    .windows
                    .iter()
                    .find(|w| w.label == "main")
                    .ok_or_else(|| tauri::Error::WindowNotFound)?;
                let window = tauri::WebviewWindowBuilder::from_config(app, config)?
                    .visible(false)
                    .build()?;
                configure_main(app);
                let (bounds, maximized) = {
                    let state = desktop.window.lock().unwrap();
                    (state.bounds, state.maximized)
                };
                if let Some((position, size)) = bounds {
                    let _ = window.set_size(size);
                    let _ = window.set_position(position);
                }
                if maximized {
                    window::maximize_on_show(app);
                }
                (window, true)
            }
        };
        // A new WebView shows itself after its first paint (WindowTitleBar); showing it now flashes black.
        if created {
            return Ok(());
        }
        let _ = window.emit("desktop-resume", ());
        let _ = window.unminimize();
        window.show()?;
        window.set_focus()?;
        std::thread::spawn(move || activate(&window));
        Ok(())
    })();
    desktop.window.lock().unwrap().creating = false;
    if let Err(error) = result {
        eprintln!("odx: could not open window: {error}");
    }
}

pub fn cancel_suspend(app: &AppHandle, ticket: u64) {
    let pending = app.state::<Arc<Desktop>>().window.lock().unwrap().pending;
    if pending.is_some_and(|(id, _)| id == ticket) {
        show_main(app);
    }
}

pub fn suspend_main(
    app: &AppHandle,
    window: &tauri::WebviewWindow,
    ticket: u64,
    snapshot: String,
    unlocked: bool,
) -> Result<bool, String> {
    const MAX_SNAPSHOT: usize = 16 * 1024 * 1024;
    if window.label() != "main"
        || snapshot.len() > MAX_SNAPSHOT
        || serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&snapshot).is_err()
    {
        cancel_suspend(app, ticket);
        return Err("invalid window state".into());
    }
    let desktop = app.state::<Arc<Desktop>>();
    if !desktop
        .window
        .lock()
        .unwrap()
        .save(ticket, snapshot, unlocked)
    {
        return Ok(false);
    }
    #[cfg(target_os = "linux")]
    terminate_web_process(window);
    if let Err(error) = window.destroy() {
        let mut state = desktop.window.lock().unwrap();
        state.destroying = false;
        state.snapshot = None;
        drop(state);
        let _ = window.reload();
        show_main(app);
        return Err(error.to_string());
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(DESTROY_TIMEOUT).await;
        recover_stalled_destroy(&app, ticket);
    });
    Ok(true)
}

/// Without this, a lost `Destroyed` event would leave the tray unable to reopen the window.
fn recover_stalled_destroy(app: &AppHandle, ticket: u64) {
    {
        let desktop = app.state::<Arc<Desktop>>();
        let mut state = desktop.window.lock().unwrap();
        if !state.stalled(ticket) {
            return;
        }
        eprintln!("odx: window was not destroyed within {DESTROY_TIMEOUT:?}");
        // A surviving WebView keeps its live state; the snapshot would be stale.
        if app.get_webview_window("main").is_some() {
            state.snapshot = None;
        }
    }
    on_destroyed(app);
}

pub fn on_destroyed(app: &AppHandle) {
    let reopen = {
        let desktop = app.state::<Arc<Desktop>>();
        let reopen = {
            let mut state = desktop.window.lock().unwrap();
            state.destroying = false;
            std::mem::take(&mut state.reopen)
        };
        // Agents waiting on this window get an answer now instead of waiting for their timeout.
        // A show not yet taken survives a reopen: the new window takes it.
        let failed = {
            let mut queue = desktop.agent.lock().unwrap();
            let AgentQueue {
                pending, replies, ..
            } = &mut *queue;
            pending.retain(|request| reopen && matches!(request, AgentRequest::Show { .. }));
            let kept: Vec<u64> = pending.iter().map(AgentRequest::id).collect();
            let mut failed = Vec::new();
            for (id, reply) in std::mem::take(replies) {
                if kept.contains(&id) {
                    replies.insert(id, reply);
                } else {
                    failed.push(reply);
                }
            }
            failed
        };
        for reply in failed {
            let _ = reply.send(Err("The Odx window closed before answering: the command may not have run. Call show, check the terminal, then run it again if needed.".into()));
        }
        reopen
    };
    if reopen {
        show_main(app);
    }
}

/// Lock the core and revoke pending approvals, as in `commands::vault_lock`.
pub async fn lock(app: &AppHandle) {
    let desktop = app.state::<Arc<Desktop>>().inner().clone();
    let _one_at_a_time = desktop.locking.lock().await;
    flush_window(app).await;
    clear_window_state(app);
    if let Some(core) = app.try_state::<Arc<Core>>() {
        let core = core.inner().clone();
        core.lock().await;
    }
}

pub async fn unlocked(core: &Core) -> bool {
    core.status().await == "unlocked"
}

/// Let the window save pending Terminal changes before the vault starts rejecting them.
async fn flush_window(app: &AppHandle) {
    const WAIT: std::time::Duration = std::time::Duration::from_secs(2);
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let (sender, done) = tokio::sync::oneshot::channel();
    *app.state::<Arc<Desktop>>().flush.lock().unwrap() = Some(sender);
    if window.emit("desktop-flush", ()).is_ok() {
        let _ = tokio::time::timeout(WAIT, done).await;
    }
}

pub fn flushed(app: &AppHandle) {
    if let Some(sender) = app.state::<Arc<Desktop>>().flush.lock().unwrap().take() {
        let _ = sender.send(());
    }
}

/// Lock (up to `EXIT_LOCK`), drain sent operations up to `EXIT_DRAIN`, then exit or restart.
/// Restart goes through here because Tauri ignores `prevent_exit` for `RESTART_EXIT_CODE`.
pub fn quit(app: &AppHandle, restart: bool) {
    let desktop = app.state::<Arc<Desktop>>().inner().clone();
    if !desktop.exit.begin() {
        return;
    }
    if let Some(window) = app.get_webview_window("main") {
        remember_main_bounds(&window.as_ref().window());
    }
    save_main_placement(app);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        // Quit must always finish: a stuck lock (a peer hung after sleep, a blocked store) cannot hold the exit.
        if tokio::time::timeout(EXIT_LOCK, lock(&app)).await.is_err() {
            eprintln!(
                "odx: exiting after {} s without finishing the lock",
                EXIT_LOCK.as_secs()
            );
        }
        if let Some(core) = app.try_state::<Arc<Core>>() {
            let core = core.inner().clone();
            if !core.drain(EXIT_DRAIN).await {
                eprintln!(
                    "odx: exiting after {} s with operations still in progress",
                    EXIT_DRAIN.as_secs()
                );
            }
        }
        desktop.exit.finish();
        if restart {
            app.request_restart();
        } else {
            app.exit(0);
        }
    });
}

/// Hold `RunEvent::ExitRequested` until locking and draining finish.
///
/// `code` is `None` only when the last window closes, which with a usable tray means a
/// suspended main window: keep running. Explicit exits (`app.exit`) carry a code.
/// Odx is built for Linux only; review this if macOS (Cmd+Q) is ever supported.
pub fn hold_exit(app: &AppHandle, code: Option<i32>) -> bool {
    if code.is_none() && app.state::<Arc<Desktop>>().tray_usable() {
        return true;
    }
    let held = app.state::<Arc<Desktop>>().exit.must_hold();
    if held {
        quit(app, false);
    }
    held
}

pub fn on_close_requested(window: &tauri::Window, api: &tauri::CloseRequestApi) {
    let desktop = window.state::<Arc<Desktop>>();
    if window.label() == "main" && close_action(desktop.tray_usable()) == CloseAction::Suspend {
        api.prevent_close();
        remember_main_bounds(window);
        save_main_placement(window.app_handle());
        let ticket = {
            let mut state = desktop.window.lock().unwrap();
            state.request()
        };
        if let Some(ticket) = ticket {
            let _ = window.hide();
            if window.emit("desktop-suspend", ticket).is_err() {
                cancel_suspend(window.app_handle(), ticket);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn agent_requests_keep_the_shape_ipc_ts_reads() {
        let run = AgentRequest::Run {
            id: 1,
            terminal: None,
            env_id: "e".into(),
            command: "count res.partner".into(),
            client_id: "c".into(),
            client_revision: 2,
            client_label: "Claude".into(),
            allow_writes: true,
        };
        assert_eq!(
            serde_json::to_value(run).unwrap(),
            serde_json::json!({"kind": "run", "id": 1, "terminal": null, "envId": "e", "command": "count res.partner",
                "clientId": "c", "clientRevision": 2, "clientLabel": "Claude", "allowWrites": true})
        );
        assert_eq!(
            serde_json::to_value(AgentRequest::Show {
                id: 3,
                terminal: "t1".into()
            })
            .unwrap(),
            serde_json::json!({"kind": "show", "id": 3, "terminal": "t1"})
        );
    }

    #[test]
    fn maximized_and_recreated_windows_keep_the_normal_bounds() {
        let mut state = WindowSession::default();
        let position = tauri::PhysicalPosition::new(120, 80);
        let size = tauri::PhysicalSize::new(1180, 780);
        state.remember_bounds(position, size, false);
        state.remember_bounds(
            tauri::PhysicalPosition::new(0, 0),
            tauri::PhysicalSize::new(1920, 1080),
            true,
        );
        assert_eq!(state.bounds, Some((position, size)));
        assert!(state.maximized);

        state.creating = true;
        state.remember_bounds(position, tauri::PhysicalSize::new(900, 650), false);
        assert_eq!(state.bounds, Some((position, size)));
        assert!(state.maximized);

        state.creating = false;
        state.remember_bounds(position, size, false);
        assert!(!state.maximized);
        let resized = tauri::PhysicalSize::new(1250, 850);
        state.remember_bounds(position, resized, false);
        assert_eq!(state.bounds, Some((position, resized)));
    }

    #[test]
    fn a_cancelled_close_cannot_destroy_a_reopened_window() {
        let mut state = WindowSession::default();
        let old = state.request().unwrap();
        assert!(state.request().is_none());
        state.pending = None; // Opening the tray cancels this close.
        let current = state.request().unwrap();
        assert!(!state.save(old, "{}".into(), true));
        assert!(!state.destroying);
        assert!(state.save(current, "{\"shell\":true}".into(), true));
        assert!(state.destroying);
        assert!(state.snapshot.is_some());
    }

    #[test]
    fn attention_dot_fills_only_the_top_right_corner() {
        let icon = tauri::image::Image::new_owned(vec![0; 32 * 32 * 4], 32, 32);
        let marked = with_attention_dot(&icon);
        let pixel = |x: u32, y: u32| &marked.rgba()[((y * 32 + x) * 4) as usize..][..4];
        assert_eq!(pixel(28, 3), [0xe5, 0x48, 0x4d, 0xff]);
        assert_eq!(pixel(3, 28), [0, 0, 0, 0]);
        assert_eq!(pixel(3, 3), [0, 0, 0, 0]);
    }

    #[test]
    fn invisible_tags_are_distinct_and_zero_width() {
        let tags: std::collections::HashSet<_> = (1..=64).map(invisible_tag).collect();
        assert_eq!(tags.len(), 64);
        assert!(
            tags.iter()
                .flat_map(|tag| tag.chars())
                .all(|c| c == '\u{200B}' || c == '\u{200C}')
        );
    }

    #[test]
    fn only_the_current_destroy_can_be_recovered_as_stalled() {
        let mut state = WindowSession::default();
        let old = state.request().unwrap();
        assert!(!state.stalled(old), "not yet saved");
        assert!(state.save(old, "{}".into(), true));
        assert!(state.stalled(old));
        state.destroying = false; // `WindowEvent::Destroyed` arrived.
        assert!(!state.stalled(old));
        let current = state.request().unwrap();
        assert!(state.save(current, "{}".into(), true));
        assert!(
            !state.stalled(old),
            "a late watchdog cannot reset a newer close"
        );
        assert!(state.stalled(current));
    }

    #[test]
    fn locking_invalidates_saved_and_in_flight_snapshots() {
        let mut state = WindowSession::default();
        let ticket = state.request().unwrap();
        state.clear();
        assert!(state.save(ticket, "{\"old\":true}".into(), true));
        assert!(
            state.snapshot.is_none(),
            "a later unlock cannot retain the old session"
        );
        state.destroying = false;
        let ticket = state.request().unwrap();
        assert!(state.save(ticket, "{}".into(), true));
        assert!(state.snapshot.is_some());
        state.clear();
        assert!(state.snapshot.is_none());
    }

    #[test]
    fn closing_suspends_only_with_a_usable_tray() {
        assert_eq!(close_action(true), CloseAction::Suspend);
        assert_eq!(close_action(false), CloseAction::Close);
        let desktop = Desktop::default();
        assert_eq!(close_action(desktop.tray_usable()), CloseAction::Close);
        desktop.set_tray(Err("no StatusNotifierWatcher".into()));
        assert_eq!(close_action(desktop.tray_usable()), CloseAction::Close);
        assert_eq!(
            desktop.status().tray_error.as_deref(),
            Some("no StatusNotifierWatcher")
        );
        desktop.set_tray(Ok(()));
        assert_eq!(close_action(desktop.tray_usable()), CloseAction::Suspend);
        assert_eq!(desktop.status().tray_error, None);
    }

    #[test]
    fn connector_mode_never_reaches_the_single_instance() {
        let args = |a: &[&str]| {
            a.iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
                .into_iter()
        };
        assert_eq!(launch_mode(args(&["Odx", "mcp-stdio"])), Launch::Connector);
        assert_eq!(launch_mode(args(&["Odx"])), Launch::Gui);
        assert_eq!(launch_mode(args(&["Odx", "--other"])), Launch::Gui);
        assert_eq!(launch_mode(args(&["Odx", "x", "mcp-stdio"])), Launch::Gui);
    }

    #[test]
    fn exit_is_held_until_drained_and_started_once() {
        let gate = ExitGate::default();
        assert!(gate.must_hold());
        assert!(gate.begin(), "the first request starts shutdown");
        assert!(
            !gate.begin(),
            "repeated exit requests cannot duplicate shutdown"
        );
        assert!(gate.must_hold());
        gate.finish();
        assert!(!gate.must_hold(), "allow final exit");
    }

    #[test]
    fn notification_state_reports_only_changes() {
        let desktop = Desktop::default();
        assert!(!desktop.set_notifications(None));
        assert!(desktop.set_notifications(Some("no server".into())));
        assert!(!desktop.set_notifications(Some("no server".into())));
        assert_eq!(
            desktop.status().notifications_error.as_deref(),
            Some("no server")
        );
        assert!(desktop.set_notifications(None));
    }
}
