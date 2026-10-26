//! Linux desktop specifics: KWin scripting for keep-above and activation, WebKitGTK tweaks.
use super::*;

/// Wayland gives no app a way to stay above others: GTK's hint is silently ignored and only the compositor decides.
pub(super) fn wayland() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some()
        && std::env::var("GDK_BACKEND").map_or(true, |backend| backend != "x11")
}

pub(super) fn kde() -> bool {
    std::env::var("XDG_CURRENT_DESKTOP")
        .is_ok_and(|desktop| desktop.split(':').any(|d| d.eq_ignore_ascii_case("KDE")))
}

/// Whether "keep above" works here: X11, Windows and macOS through the window hint, KDE on Wayland through KWin.
pub fn keep_above_supported() -> bool {
    !wayland() || kde()
}

/// Keep a window above the others, or let it go back.
pub fn keep_above(window: &tauri::WebviewWindow, above: bool) -> Result<(), String> {
    if !wayland() {
        return window.set_always_on_top(above).map_err(|e| e.to_string());
    }
    if !kde() {
        return Err("not supported on this desktop".into());
    }
    kwin_window_script(
        &window.title().map_err(|e| e.to_string())?,
        &format!("w.keepAbove = {above};"),
    )
}

/// Bring a window to the front where the app alone cannot: KWin on Wayland ignores focus requests from apps
/// (focus stealing prevention) and only marks them as wanting attention, so ask KWin itself. Blocks ~200 ms.
pub fn activate(window: &tauri::WebviewWindow) {
    if !wayland() || !kde() {
        return;
    }
    // Plasma 6 names it activeWindow, Plasma 5 activeClient.
    const ACTIVATE: &str = "w.minimized = false; \
        if (\"activeWindow\" in workspace) workspace.activeWindow = w; else workspace.activeClient = w;";
    let result = window
        .title()
        .map_err(|e| e.to_string())
        .and_then(|title| kwin_window_script(&title, ACTIVATE));
    if let Err(error) = result {
        eprintln!("odx: could not bring the window to the front: {error}");
    }
}

/// Ask KWin with a one-off script, loaded and unloaded over D-Bus, to run `action` on this process's window `w`.
/// Windows are matched by title, which `invisible_tag` keeps unique between table windows.
pub(super) fn kwin_window_script(caption: &str, action: &str) -> Result<(), String> {
    use std::sync::atomic::AtomicU64;
    static SCRIPTS: AtomicU64 = AtomicU64::new(0);
    let plugin = format!(
        "odx-window-{}-{}",
        std::process::id(),
        SCRIPTS.fetch_add(1, Ordering::Relaxed)
    );
    let path = std::env::temp_dir().join(format!("{plugin}.js"));
    let caption = serde_json::to_string(caption).map_err(|e| e.to_string())?;
    let script = format!(
        "const windows = workspace.windowList ? workspace.windowList() : workspace.clientList();\n\
         for (const w of windows) if (w.pid === {} && w.caption === {caption}) {{ {action} }}\n",
        std::process::id()
    );
    std::fs::write(&path, script).map_err(|e| e.to_string())?;
    let run = || -> zbus::Result<()> {
        let bus = zbus::blocking::Connection::session()?;
        let script = path.to_string_lossy();
        let id: i32 = bus
            .call_method(
                Some("org.kde.KWin"),
                "/Scripting",
                Some("org.kde.kwin.Scripting"),
                "loadScript",
                &(script.as_ref(), plugin.as_str()),
            )?
            .body()
            .deserialize()?;
        bus.call_method(
            Some("org.kde.KWin"),
            format!("/Scripting/Script{id}").as_str(),
            Some("org.kde.kwin.Script"),
            "run",
            &(),
        )?;
        // KWin runs it on its own loop; give it a moment before unloading.
        std::thread::sleep(std::time::Duration::from_millis(200));
        bus.call_method(
            Some("org.kde.KWin"),
            "/Scripting",
            Some("org.kde.kwin.Scripting"),
            "unloadScript",
            &(plugin.as_str(),),
        )?;
        Ok(())
    };
    let result = run();
    let _ = std::fs::remove_file(&path);
    result.map_err(|e| e.to_string())
}

/// Destroying the window leaves its WebKitWebProcess (~200 MB) running, one more per suspension.
/// The snapshot is already saved, so the page has nothing left to lose.
#[cfg(target_os = "linux")]
pub(super) fn terminate_web_process(window: &tauri::WebviewWindow) {
    if let Err(error) = window.with_webview(|webview| {
        use webkit2gtk::WebViewExt;
        webview.inner().terminate_web_process();
    }) {
        eprintln!("odx: could not stop the web process: {error}");
    }
}

/// Disable WebKitGTK's native trackpad pinch zoom on Linux.
#[cfg(target_os = "linux")]
pub fn disable_trackpad_zoom(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main")
        && let Err(error) = window.with_webview(|webview| {
            use webkit2gtk::glib::{gobject_ffi, prelude::ObjectExt};

            let view = webview.inner();
            // SAFETY: WebKitGTK stores a live GObject gesture under this private key; we only remove its handlers and leave the object owned by WebKit.
            let gesture = unsafe { view.data::<gobject_ffi::GObject>("wk-view-zoom-gesture") };
            if let Some(gesture) = gesture {
                unsafe { gobject_ffi::g_signal_handlers_destroy(gesture.as_ptr()) };
            } else {
                eprintln!("odx: WebKitGTK pinch handler unavailable");
            }
        })
    {
        eprintln!("odx: could not disable WebKitGTK trackpad zoom: {error}");
    }
}

/// Let the app's own context menu cut, copy and paste: WebKitGTK keeps script clipboard access off per web view.
/// The webview only loads the bundled UI (CSP `default-src 'self'`).
#[cfg(target_os = "linux")]
pub(super) fn enable_clipboard_access(window: &tauri::WebviewWindow) {
    if let Err(error) = window.with_webview(|webview| {
        use webkit2gtk::{SettingsExt, WebViewExt};
        if let Some(settings) = webview.inner().settings() {
            settings.set_javascript_can_access_clipboard(true);
        }
    }) {
        eprintln!("odx: could not enable clipboard access: {error}");
    }
}
