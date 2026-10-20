//! Desktop integration: single instance, tray, close behavior, graceful shutdown and availability notices.
use odx_core::Core;
use serde::Serialize;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use tauri::{AppHandle, Emitter, Manager};
use zeroize::Zeroizing;

/// Deadline for draining sent operations on exit.
pub const EXIT_DRAIN: std::time::Duration = std::time::Duration::from_secs(10);
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

/// Apply the same WebKit settings to initial and recreated windows.
pub fn configure_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        remember_main_bounds(&window.as_ref().window());
    }
    #[cfg(target_os = "linux")]
    {
        disable_trackpad_zoom(app);
        enable_clipboard_access(app);
    }
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
        let window = match app.get_webview_window("main") {
            Some(window) => window,
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
                    let _ = window.maximize();
                }
                window
            }
        };
        let _ = window.emit("desktop-resume", ());
        let _ = window.unminimize();
        window.show()?;
        window.set_focus()
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

/// Destroying the window leaves its WebKitWebProcess (~200 MB) running, one more per suspension.
/// The snapshot is already saved, so the page has nothing left to lose.
#[cfg(target_os = "linux")]
fn terminate_web_process(window: &tauri::WebviewWindow) {
    if let Err(error) = window.with_webview(|webview| {
        use webkit2gtk::WebViewExt;
        webview.inner().terminate_web_process();
    }) {
        eprintln!("odx: could not stop the web process: {error}");
    }
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
        let mut state = desktop.window.lock().unwrap();
        state.destroying = false;
        std::mem::take(&mut state.reopen)
    };
    if reopen {
        show_main(app);
    }
}

/// Lock the core and revoke pending approvals, as in `commands::vault_lock`.
async fn lock(app: &AppHandle) {
    clear_window_state(app);
    if let Some(core) = app.try_state::<Arc<Core>>() {
        let core = core.inner().clone();
        core.lock().await;
    }
}

/// Lock after the computer sleeps: a monotonic sleep does not advance while suspended, so the wall clock jumps past it.
// shortcut: moving the system clock forward by more than the gap also locks; listen to OS sleep notifications if that bites.
pub fn lock_on_sleep(app: AppHandle) {
    const CHECK: std::time::Duration = std::time::Duration::from_secs(5);
    const GAP: std::time::Duration = std::time::Duration::from_secs(30);
    tauri::async_runtime::spawn(async move {
        loop {
            let before = std::time::SystemTime::now();
            tokio::time::sleep(CHECK).await;
            if before.elapsed().is_ok_and(|elapsed| elapsed > GAP) {
                lock_if_unlocked(&app).await;
            }
        }
    });
}

async fn lock_if_unlocked(app: &AppHandle) {
    let unlocked = match app.try_state::<Arc<Core>>() {
        Some(core) => core.inner().clone().status().await == "unlocked",
        None => false,
    };
    if unlocked {
        lock(app).await;
    }
}

/// Lock when the desktop locks the screen, which GNOME, KDE and others report as logind's `LockedHint`.
// shortcut: Linux only; Windows (WTS session notifications) and macOS (screenIsLocked) need their own listeners.
#[cfg(target_os = "linux")]
pub fn lock_on_screen_lock(app: AppHandle) {
    std::thread::spawn(move || {
        if let Err(error) = watch_screen_lock(&app) {
            eprintln!("odx: screen lock is not watched: {error}");
        }
    });
}
#[cfg(not(target_os = "linux"))]
pub fn lock_on_screen_lock(_app: AppHandle) {}

#[cfg(target_os = "linux")]
fn watch_screen_lock(app: &AppHandle) -> zbus::Result<()> {
    use zbus::{
        blocking::{Connection, Proxy},
        zvariant::OwnedObjectPath,
    };
    let connection = Connection::system()?;
    // The user's graphical session, even when Odx itself was started outside it.
    let user = Proxy::new(
        &connection,
        "org.freedesktop.login1",
        "/org/freedesktop/login1/user/self",
        "org.freedesktop.login1.User",
    )?;
    let (_, path): (String, OwnedObjectPath) = user.get_property("Display")?;
    let session = Proxy::new(
        &connection,
        "org.freedesktop.login1",
        path,
        "org.freedesktop.login1.Session",
    )?;
    for change in session.receive_property_changed::<bool>("LockedHint") {
        if change.get()? {
            tauri::async_runtime::block_on(lock_if_unlocked(app));
        }
    }
    Ok(())
}

/// Lock, drain sent operations up to `EXIT_DRAIN`, then exit.
pub fn quit(app: &AppHandle) {
    let desktop = app.state::<Arc<Desktop>>().inner().clone();
    if !desktop.exit.begin() {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        lock(&app).await;
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
        app.exit(0);
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
        quit(app);
    }
    held
}

pub fn remember_main_bounds(window: &tauri::Window) {
    if window.label() != "main"
        || window.is_minimized().unwrap_or(true)
        || window.is_fullscreen().unwrap_or(true)
    {
        return;
    }
    if let (Ok(position), Ok(size), Ok(maximized)) = (
        window.outer_position(),
        window.inner_size(),
        window.is_maximized(),
    ) {
        window
            .state::<Arc<Desktop>>()
            .window
            .lock()
            .unwrap()
            .remember_bounds(position, size, maximized);
    }
}

pub fn on_close_requested(window: &tauri::Window, api: &tauri::CloseRequestApi) {
    let desktop = window.state::<Arc<Desktop>>();
    if window.label() == "main" && close_action(desktop.tray_usable()) == CloseAction::Suspend {
        api.prevent_close();
        remember_main_bounds(window);
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

/// Create the tray and record availability. Treat a missing Linux indicator library panic as unavailable.
pub fn setup_tray(app: &tauri::App, desktop: &Desktop) {
    let built = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| build_tray(app)))
        .unwrap_or_else(|_| {
            Err("could not load libayatana-appindicator3 or libappindicator3".into())
        });
    let result = built.and_then(|()| status_notifier_available());
    if let Err(e) = &result {
        eprintln!("odx: tray unavailable: {e}");
    }
    desktop.set_tray(result);
}

/// Disable WebKitGTK's native trackpad pinch zoom on Linux.
#[cfg(target_os = "linux")]
pub fn disable_trackpad_zoom(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        if let Err(error) = window.with_webview(|webview| {
            use webkit2gtk::glib::{gobject_ffi, prelude::ObjectExt};

            let view = webview.inner();
            // SAFETY: WebKitGTK stores a live GObject gesture under this private key; we only remove its handlers and leave the object owned by WebKit.
            let gesture = unsafe { view.data::<gobject_ffi::GObject>("wk-view-zoom-gesture") };
            if let Some(gesture) = gesture {
                unsafe { gobject_ffi::g_signal_handlers_destroy(gesture.as_ptr()) };
            } else {
                eprintln!("odx: WebKitGTK pinch handler unavailable");
            }
        }) {
            eprintln!("odx: could not disable WebKitGTK trackpad zoom: {error}");
        }
    }
}

/// Let the app's own context menu cut, copy and paste: WebKitGTK blocks script clipboard access by default.
/// The webview only loads the bundled UI (CSP `default-src 'self'`).
#[cfg(target_os = "linux")]
pub fn enable_clipboard_access(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main")
        && let Err(error) = window.with_webview(|webview| {
            use webkit2gtk::{SettingsExt, WebViewExt};
            if let Some(settings) = webview.inner().settings() {
                settings.set_javascript_can_access_clipboard(true);
            }
        })
    {
        eprintln!("odx: could not enable clipboard access: {error}");
    }
}

fn build_tray(app: &tauri::App) -> Result<(), String> {
    use tauri::menu::{Menu, MenuItem};
    let item = |id: &str, text: &str| MenuItem::with_id(app, id, text, true, None::<&str>);
    let (open, lock_item, quit_item) = (
        item("open", "Open").map_err(|e| e.to_string())?,
        item("lock", "Lock").map_err(|e| e.to_string())?,
        item("quit", "Quit").map_err(|e| e.to_string())?,
    );
    let menu =
        Menu::with_items(app, &[&open, &lock_item, &quit_item]).map_err(|e| e.to_string())?;
    let mut builder = tauri::tray::TrayIconBuilder::with_id("main")
        .tooltip("Odx")
        .menu(&menu)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => show_main(app),
            "lock" => {
                let app = app.clone();
                tauri::async_runtime::spawn(async move { lock(&app).await });
            }
            "quit" => quit(app),
            _ => {}
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app).map(drop).map_err(|e| e.to_string())
}

/// Linux requires StatusNotifierWatcher. Without it, closing exits instead of hiding.
#[cfg(target_os = "linux")]
fn status_notifier_available() -> Result<(), String> {
    use zbus::blocking::{Connection, fdo::DBusProxy};
    let connection = Connection::session().map_err(|e| format!("no session bus: {e}"))?;
    let proxy = DBusProxy::new(&connection).map_err(|e| e.to_string())?;
    let name = zbus::names::BusName::try_from("org.kde.StatusNotifierWatcher")
        .map_err(|e| e.to_string())?;
    match proxy.name_has_owner(name) {
        Ok(true) => Ok(()),
        Ok(false) => Err("desktop has no notification area (StatusNotifierWatcher)".into()),
        Err(e) => Err(e.to_string()),
    }
}
#[cfg(not(target_os = "linux"))]
fn status_notifier_available() -> Result<(), String> {
    Ok(())
}

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

fn record_notification(app: &AppHandle, error: Option<String>) {
    if let Some(e) = &error {
        eprintln!("odx: notifications unavailable: {e}");
    }
    let changed = app.state::<Arc<Desktop>>().set_notifications(error);
    if changed {
        let _ = app.emit("desktop-status-changed", ());
    }
}

/// Notify new approval requests. Report failures in stderr and the window; requests remain visible in Approvals and the status bar.
pub fn notify(app: &AppHandle, title: &'static str, body: String) {
    #[cfg(target_os = "linux")]
    let icon = app
        .path()
        .resource_dir()
        .ok()
        .map(|dir| dir.join("icons/icon.png"))
        .filter(|path| path.is_file())
        .unwrap_or_else(|| {
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("icons/icon.png")
        });
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let shown = tauri::async_runtime::spawn_blocking(move || {
            let mut notification = notify_rust::Notification::new();
            notification.appname("Odx").summary(title).body(&body);
            #[cfg(target_os = "linux")]
            notification.icon(&icon.to_string_lossy());
            notification.show().map(drop).map_err(|e| e.to_string())
        })
        .await
        .unwrap_or_else(|_| Err("notification interrupted".into()));
        record_notification(&app, shown.err());
    });
}

#[cfg(test)]
mod tests {
    use super::*;

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
