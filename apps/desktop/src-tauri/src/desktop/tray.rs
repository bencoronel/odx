//! Tray icon, its menu and the pending-approvals dot.
use super::*;

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

pub(super) fn tray_menu(app: &AppHandle) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    use tauri::menu::{Menu, MenuItem};
    let item = |id: &str, text: &str| MenuItem::with_id(app, id, text, true, None::<&str>);
    let texts = texts(app);
    Menu::with_items(
        app,
        &[
            &item("open", &texts.open)?,
            &item("lock", &texts.lock)?,
            &item("quit", &texts.quit)?,
        ],
    )
}

pub(super) fn build_tray(app: &tauri::App) -> Result<(), String> {
    let menu = tray_menu(app.handle()).map_err(|e| e.to_string())?;
    let mut builder = tauri::tray::TrayIconBuilder::with_id("main")
        .tooltip(crate::APP_NAME)
        .menu(&menu)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => show_main(app),
            "lock" => {
                let app = app.clone();
                tauri::async_runtime::spawn(async move { lock(&app).await });
            }
            "quit" => quit(app, false),
            _ => {}
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app).map(drop).map_err(|e| e.to_string())
}

/// Linux requires StatusNotifierWatcher. Without it, closing exits instead of hiding.
#[cfg(target_os = "linux")]
pub(super) fn status_notifier_available() -> Result<(), String> {
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
pub(super) fn status_notifier_available() -> Result<(), String> {
    Ok(())
}

/// Mark the tray icon while approvals wait, so they show even with the window closed.
pub fn set_tray_attention(app: &AppHandle, on: bool) {
    let (Some(tray), Some(icon)) = (app.tray_by_id("main"), app.default_window_icon()) else {
        return;
    };
    let icon = if on {
        with_attention_dot(icon)
    } else {
        icon.clone().to_owned()
    };
    if let Err(error) = tray.set_icon(Some(icon)) {
        eprintln!("odx: could not update the tray icon: {error}");
    }
}

/// The icon with a filled dot in its top-right corner.
pub(super) fn with_attention_dot(icon: &tauri::image::Image<'_>) -> tauri::image::Image<'static> {
    let (width, height) = (icon.width(), icon.height());
    let mut rgba = icon.rgba().to_vec();
    let radius = width.min(height) as f32 * 0.22;
    let (cx, cy) = (width as f32 - radius, radius);
    for y in 0..height {
        for x in 0..width {
            let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
            if dx * dx + dy * dy <= radius * radius {
                let i = ((y * width + x) * 4) as usize;
                rgba[i..i + 4].copy_from_slice(&[0xe5, 0x48, 0x4d, 0xff]);
            }
        }
    }
    tauri::image::Image::new_owned(rgba, width, height)
}
