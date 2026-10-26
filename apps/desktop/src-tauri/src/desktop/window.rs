//! Main window placement: saved bounds, monitor fit, first configuration.
use super::*;

/// Main window placement kept for the next launch, in physical pixels. `bounds` is the unmaximized rectangle.
#[derive(Serialize, Deserialize)]
pub(super) struct SavedWindow {
    pub(super) bounds: Option<(i32, i32, u32, u32)>,
    pub(super) maximized: bool,
}

pub(super) fn saved_window_path(app: &AppHandle) -> Option<std::path::PathBuf> {
    app.path()
        .app_data_dir()
        .ok()
        .map(|dir| dir.join("window.json"))
}

/// Place the initial window where the last session left it, or size it to the monitor on first launch.
pub fn place_main(app: &AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let saved = saved_window_path(app)
        .and_then(|path| std::fs::read(path).ok())
        .and_then(|bytes| serde_json::from_slice::<SavedWindow>(&bytes).ok());
    let Some(saved) = saved else {
        fit_to_monitor(&window);
        return;
    };
    match saved.bounds {
        Some((x, y, width, height)) => {
            let _ = window.set_size(tauri::PhysicalSize::new(width, height));
            // A monitor unplugged since the last session would leave the window off-screen.
            let visible = window
                .available_monitors()
                .unwrap_or_default()
                .iter()
                .any(|monitor| {
                    let area = monitor.work_area();
                    (area.position.x..area.position.x + area.size.width as i32).contains(&x)
                        && (area.position.y..area.position.y + area.size.height as i32).contains(&y)
                });
            if visible {
                let _ = window.set_position(tauri::PhysicalPosition::new(x, y));
            } else {
                let _ = window.center();
            }
        }
        None => fit_to_monitor(&window),
    }
    if saved.maximized {
        maximize_on_show(app);
    }
}

/// Maximize a restored window when it shows instead of while it is hidden: maximizing a WebView before its first
/// paint can leave the page at its old size in a larger window (GNOME).
pub(super) fn maximize_on_show(app: &AppHandle) {
    app.state::<Arc<Desktop>>()
        .window
        .lock()
        .unwrap()
        .maximize_on_show = true;
}

/// Show a window after its first paint, maximized first when it was restored maximized.
pub fn present(window: &tauri::WebviewWindow) {
    let maximize = window.label() == "main"
        && std::mem::take(
            &mut window
                .state::<Arc<Desktop>>()
                .window
                .lock()
                .unwrap()
                .maximize_on_show,
        );
    if maximize {
        let _ = window.maximize();
    }
    let _ = window.show();
    let _ = window.set_focus();
    activate(window);
}

/// Best effort: a failed write only means the next launch opens at the default size.
pub(super) fn save_main_placement(app: &AppHandle) {
    let desktop = app.state::<Arc<Desktop>>();
    let saved = {
        let state = desktop.window.lock().unwrap();
        SavedWindow {
            bounds: state.bounds.map(|(p, s)| (p.x, p.y, s.width, s.height)),
            maximized: state.maximized,
        }
    };
    let Some(path) = saved_window_path(app) else {
        return;
    };
    let written = serde_json::to_vec(&saved)
        .map_err(std::io::Error::from)
        .and_then(|bytes| std::fs::write(&path, bytes));
    if let Err(error) = written {
        eprintln!("odx: could not save window placement: {error}");
    }
}

/// Open at 68% of the screen's work area, between the configured minimum and 1440×900; the config size is the fallback when no screen is known.
/// Recreated windows then restore their remembered bounds over this.
pub(super) fn fit_to_monitor(window: &tauri::WebviewWindow) {
    let logical = |m: &tauri::Monitor| m.work_area().size.to_logical::<f64>(m.scale_factor());
    let Some(monitor) = window
        .current_monitor()
        .ok()
        .flatten()
        .or_else(|| window.primary_monitor().ok().flatten())
        // Wayland reports neither before the window is shown; size for the smallest screen so it fits wherever it opens.
        .or_else(|| {
            window
                .available_monitors()
                .ok()?
                .into_iter()
                .min_by(|a, b| {
                    let (a, b) = (logical(a), logical(b));
                    (a.width * a.height).total_cmp(&(b.width * b.height))
                })
        })
    else {
        return;
    };
    let area = logical(&monitor);
    let width = (area.width * 0.68).clamp(900.0, 1440.0);
    let height = (area.height * 0.68).clamp(650.0, 900.0);
    let _ = window.set_size(tauri::LogicalSize::new(width, height));
    let _ = window.center();
}

/// Apply the same WebKit settings to initial and recreated windows.
pub fn configure_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        remember_main_bounds(&window.as_ref().window());
    }
    #[cfg(target_os = "linux")]
    {
        disable_trackpad_zoom(app);
        if let Some(window) = app.get_webview_window("main") {
            enable_clipboard_access(&window);
        }
    }
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
