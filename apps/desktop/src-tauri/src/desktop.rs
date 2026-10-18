//! Desktop integration: single instance, tray, close behavior, graceful shutdown and availability notices.
use odx_core::Core;
use serde::Serialize;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use tauri::{AppHandle, Emitter, Manager};

/// Deadline for draining sent operations on exit.
pub const EXIT_DRAIN: std::time::Duration = std::time::Duration::from_secs(10);

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
    /// Hide to tray without changing service state.
    Hide,
    /// Exit instead of hiding when the tray is unavailable.
    Close,
}
pub fn close_action(tray_usable: bool) -> CloseAction {
    if tray_usable {
        CloseAction::Hide
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

/// Show and focus the main window from the tray or a second launch.
pub fn show_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// Lock the core and revoke pending approvals, as in `commands::vault_lock`.
async fn lock(app: &AppHandle) {
    if let Some(core) = app.try_state::<Arc<Core>>() {
        let core = core.inner().clone();
        core.lock().await;
    }
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
pub fn hold_exit(app: &AppHandle) -> bool {
    let held = app.state::<Arc<Desktop>>().exit.must_hold();
    if held {
        quit(app);
    }
    held
}

pub fn on_close_requested(window: &tauri::Window, api: &tauri::CloseRequestApi) {
    let desktop = window.state::<Arc<Desktop>>();
    if window.label() == "main" && close_action(desktop.tray_usable()) == CloseAction::Hide {
        api.prevent_close();
        let _ = window.hide();
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
pub fn disable_trackpad_zoom(app: &tauri::App) {
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
pub fn enable_clipboard_access(app: &tauri::App) {
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
    fn closing_hides_only_with_a_usable_tray() {
        assert_eq!(close_action(true), CloseAction::Hide);
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
        assert_eq!(close_action(desktop.tray_usable()), CloseAction::Hide);
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
