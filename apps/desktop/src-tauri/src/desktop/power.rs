//! Lock the vault when the computer sleeps, the screen locks or the clock jumps.
use super::*;

/// Lock when the computer goes to sleep, on logind's `PrepareForSleep`; systems without logind fall back to clock gaps.
pub fn lock_on_sleep(app: AppHandle) {
    #[cfg(target_os = "linux")]
    std::thread::spawn(move || {
        if let Err(error) = watch_sleep(&app) {
            eprintln!("odx: sleep is not watched through logind, using clock gaps: {error}");
            lock_on_clock_gap(app);
        }
    });
    #[cfg(not(target_os = "linux"))]
    lock_on_clock_gap(app);
}

#[cfg(target_os = "linux")]
pub(super) fn watch_sleep(app: &AppHandle) -> zbus::Result<()> {
    let connection = zbus::blocking::Connection::system()?;
    let manager = zbus::blocking::Proxy::new(
        &connection,
        "org.freedesktop.login1",
        "/org/freedesktop/login1",
        "org.freedesktop.login1.Manager",
    )?;
    for signal in manager.receive_signal("PrepareForSleep")? {
        // true before sleeping, false after waking.
        if signal.body().deserialize::<bool>()? {
            tauri::async_runtime::block_on(lock_if_unlocked(app));
        }
    }
    Ok(())
}

/// Lock after the computer sleeps: a monotonic sleep does not advance while suspended, so the wall clock jumps past it.
/// Moving the system clock forward by more than the gap also locks, which is why logind is preferred.
pub(super) fn lock_on_clock_gap(app: AppHandle) {
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

pub(super) async fn lock_if_unlocked(app: &AppHandle) {
    let unlocked = match app.try_state::<Arc<Core>>() {
        Some(core) => unlocked(core.inner()).await,
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
pub(super) fn watch_screen_lock(app: &AppHandle) -> zbus::Result<()> {
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
