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
    use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
    let item = |id: &str, text: &str, enabled: bool| {
        MenuItem::with_id(app, id, text, enabled, None::<&str>)
    };
    let texts = texts(app);
    let state = app
        .state::<Arc<Desktop>>()
        .tray_state
        .lock()
        .unwrap()
        .clone();
    let approvals_label = if state.pending_approvals > 0 {
        format!("{} ({})", texts.approvals, state.pending_approvals)
    } else {
        texts.approvals.clone()
    };
    let writes_label = if state.open_writes > 0 {
        format!("{} ({})", texts.close_open_writes, state.open_writes)
    } else {
        texts.close_open_writes.clone()
    };
    let sep1 = PredefinedMenuItem::separator(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let open = item("open", &texts.open, true)?;
    let terminal = item("page:shell", &texts.terminal, !state.locked)?;
    let views = item("page:views", &texts.views, !state.locked)?;
    let approvals = item("page:approvals", &approvals_label, !state.locked)?;
    let close_writes = item(
        "close_writes",
        &writes_label,
        !state.locked && state.open_writes > 0,
    )?;
    let lock_item = item("lock", &texts.lock, !state.locked)?;
    let quit_item = item("quit", &texts.quit, true)?;
    Menu::with_items(
        app,
        &[
            &open,
            &sep1,
            &terminal,
            &views,
            &approvals,
            &close_writes,
            &sep2,
            &lock_item,
            &quit_item,
        ],
    )
}

pub(super) fn tray_tooltip(app: &AppHandle) -> String {
    let texts = texts(app);
    let state = app
        .state::<Arc<Desktop>>()
        .tray_state
        .lock()
        .unwrap()
        .clone();
    if state.locked {
        return format!("{} — {}", crate::APP_NAME, texts.locked);
    }
    let mut parts: Vec<String> = Vec::new();
    if state.pending_approvals > 0 {
        parts.push(format!("{} ({})", texts.approvals, state.pending_approvals));
    }
    if state.open_writes == 1 {
        parts.push(texts.open_writes_one);
    } else if state.open_writes > 1 {
        parts.push(
            texts
                .open_writes_other
                .replace("{n}", &state.open_writes.to_string()),
        );
    }
    if parts.is_empty() {
        crate::APP_NAME.into()
    } else {
        format!("{} — {}", crate::APP_NAME, parts.join(" · "))
    }
}

/// Update tray tooltip and menu when lock state, pending approvals or open write modes change.
pub fn update_tray_state(
    app: &AppHandle,
    locked: bool,
    pending_approvals: usize,
    open_writes: usize,
) {
    let next = TrayState {
        locked,
        pending_approvals,
        open_writes,
    };
    {
        let desktop = app.state::<Arc<Desktop>>();
        let mut current = desktop.tray_state.lock().unwrap();
        if *current == next {
            return;
        }
        *current = next;
    }
    if let Some(tray) = app.tray_by_id("main") {
        let _ = tray.set_tooltip(Some(tray_tooltip(app)));
        if let Ok(menu) = tray_menu(app) {
            let _ = tray.set_menu(Some(menu));
        }
    }
}

pub(super) fn build_tray(app: &tauri::App) -> Result<(), String> {
    let menu = tray_menu(app.handle()).map_err(|e| e.to_string())?;
    let mut builder = tauri::tray::TrayIconBuilder::with_id("main")
        .tooltip(crate::APP_NAME)
        .menu(&menu)
        .on_tray_icon_event(|tray, event| {
            if let tauri::tray::TrayIconEvent::Click {
                button: tauri::tray::MouseButton::Left,
                button_state: tauri::tray::MouseButtonState::Up,
                ..
            } = event
            {
                show_main(tray.app_handle());
            }
        })
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => show_main(app),
            "page:shell" => open_page_and_show(app, "shell"),
            "page:views" => open_page_and_show(app, "views"),
            "page:approvals" => open_page_and_show(app, "approvals"),
            "close_writes" => {
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    if let Some(core) = app.try_state::<Arc<Core>>()
                        && let Ok(envs) = core.environments().await
                    {
                        for env in envs {
                            if !matches!(env.write_mode, odx_core::WriteMode::ReadOnly) {
                                let _ = core.set_mode(&env.id, odx_core::WriteMode::ReadOnly).await;
                            }
                        }
                        update_tray_state(
                            &app,
                            false,
                            core.approvals().await.map_or(0, |p| p.len()),
                            0,
                        );
                        let _ = app.emit("environments-changed", ());
                    }
                });
            }
            "lock" => {
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    lock(&app).await;
                    update_tray_state(&app, true, 0, 0);
                });
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
