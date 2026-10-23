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
    /// Language chosen in the window, for text Rust shows itself (tray, notifications).
    spanish: AtomicBool,
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

#[derive(Default)]
struct AgentQueue {
    next: u64,
    pending: Vec<serde_json::Value>,
    replies: std::collections::HashMap<u64, AgentReply>,
}

type AgentReply = tokio::sync::oneshot::Sender<Result<serde_json::Value, String>>;

/// Pick the text for the window's language; the window owns the full translations.
pub fn tr(app: &AppHandle, en: &'static str, es: &'static str) -> &'static str {
    if spanish(app) { es } else { en }
}

pub fn spanish(app: &AppHandle) -> bool {
    app.state::<Arc<Desktop>>().spanish.load(Ordering::Acquire)
}

/// Follow the window's language and relabel the tray menu.
pub fn set_language(app: &AppHandle, language: &str) {
    let desktop = app.state::<Arc<Desktop>>();
    let es = language == "es";
    if desktop.spanish.swap(es, Ordering::AcqRel) == es {
        return;
    }
    if let (Some(tray), Ok(menu)) = (app.tray_by_id("main"), tray_menu(app)) {
        let _ = tray.set_menu(Some(menu));
    }
}

/// Wayland gives no app a way to stay above others: GTK's hint is silently ignored and only the compositor decides.
fn wayland() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some()
        && std::env::var("GDK_BACKEND").map_or(true, |backend| backend != "x11")
}
fn kde() -> bool {
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
fn kwin_window_script(caption: &str, action: &str) -> Result<(), String> {
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

/// Open a Terminal table in a window of its own. Its data stays in memory, never on disk, and only that window can
/// take it (`take_table` reads the caller's label).
pub fn open_table_window(
    app: &AppHandle,
    table: serde_json::Value,
    title: &str,
) -> tauri::Result<()> {
    let (label, number) = {
        let desktop = app.state::<Arc<Desktop>>();
        let mut tables = desktop.tables.lock().unwrap();
        tables.0 += 1;
        let label = format!("table-{}", tables.0);
        tables.1.insert(label.clone(), table);
        (label, tables.0)
    };
    let window = tauri::WebviewWindowBuilder::new(
        app,
        &label,
        tauri::WebviewUrl::App(format!("index.html?window={label}").into()),
    )
    .title(format!("{title}{}", invisible_tag(number)))
    .inner_size(960.0, 620.0)
    .min_inner_size(420.0, 260.0)
    // ODX's own frame, as the main window: no system title bar, rounded, shown after its first paint.
    .decorations(false)
    .transparent(true)
    .visible(false)
    .build()?;
    #[cfg(target_os = "linux")]
    enable_clipboard_access(&window);
    Ok(())
}

/// `number` in zero-width characters, so tables of the same command still get distinct window titles.
fn invisible_tag(number: u64) -> String {
    format!("{number:b}")
        .chars()
        .map(|bit| if bit == '1' { '\u{200C}' } else { '\u{200B}' })
        .collect()
}

pub fn take_table(app: &AppHandle, label: &str) -> Option<serde_json::Value> {
    app.state::<Arc<Desktop>>()
        .tables
        .lock()
        .unwrap()
        .1
        .remove(label)
}

/// Queue the core's requests for the window, which takes them with `agent_take` (also after it is recreated)
/// and answers with `agent_reply`. A run needs a live window; showing brings one back.
pub fn serve_agents(
    app: AppHandle,
    mut requests: tokio::sync::mpsc::UnboundedReceiver<odx_core::DesktopRequest>,
) {
    tauri::async_runtime::spawn(async move {
        while let Some(request) = requests.recv().await {
            let desktop = app.state::<Arc<Desktop>>();
            let (request, reply, show) = match request {
                odx_core::DesktopRequest::Run {
                    terminal,
                    env_id,
                    command,
                    client_id,
                    client_label,
                    allow_writes,
                    reply,
                } => {
                    if app.get_webview_window("main").is_none() {
                        let _ = reply.send(Err("The Odx window is closed: call show to open it, then run the command again.".into()));
                        continue;
                    }
                    let request = serde_json::json!({ "kind": "run", "terminal": terminal, "envId": env_id, "command": command,
                        "clientId": client_id, "clientLabel": client_label, "allowWrites": allow_writes });
                    (request, reply, false)
                }
                odx_core::DesktopRequest::Show { terminal, reply } => (
                    serde_json::json!({ "kind": "show", "terminal": terminal }),
                    reply,
                    true,
                ),
            };
            {
                let mut queue = desktop.agent.lock().unwrap();
                queue.next += 1;
                let id = queue.next;
                let mut request = request;
                request["id"] = id.into();
                queue.pending.push(request);
                queue.replies.insert(id, reply);
            }
            if show {
                show_main(&app);
            }
            let _ = app.emit("agent-request", ());
        }
    });
}

/// Requests the window has not taken yet; ones whose caller stopped waiting are dropped.
pub fn take_agent_requests(app: &AppHandle) -> Vec<serde_json::Value> {
    let desktop = app.state::<Arc<Desktop>>();
    let mut queue = desktop.agent.lock().unwrap();
    let AgentQueue {
        pending, replies, ..
    } = &mut *queue;
    std::mem::take(pending)
        .into_iter()
        .filter(|request| {
            request["id"]
                .as_u64()
                .and_then(|id| replies.get(&id))
                .is_some_and(|reply| !reply.is_closed())
        })
        .collect()
}

/// The window's answer to request `id`: `error`, or `result` for a run.
pub fn answer_agent(
    app: &AppHandle,
    id: u64,
    result: Option<serde_json::Value>,
    error: Option<String>,
) {
    let reply = app
        .state::<Arc<Desktop>>()
        .agent
        .lock()
        .unwrap()
        .replies
        .remove(&id);
    if let Some(reply) = reply {
        let _ = reply.send(error.map_or_else(|| Ok(result.unwrap_or_default()), Err));
    }
}

/// Hand the clicked approval to the window once; `None` when there is none.
pub fn take_open_approval(app: &AppHandle) -> Option<String> {
    app.state::<Arc<Desktop>>()
        .open_approval
        .lock()
        .unwrap()
        .take()
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

/// Main window placement kept for the next launch, in physical pixels. `bounds` is the unmaximized rectangle.
#[derive(Serialize, Deserialize)]
struct SavedWindow {
    bounds: Option<(i32, i32, u32, u32)>,
    maximized: bool,
}

fn saved_window_path(app: &AppHandle) -> Option<std::path::PathBuf> {
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
        let _ = window.maximize();
    }
}

/// Best effort: a failed write only means the next launch opens at the default size.
fn save_main_placement(app: &AppHandle) {
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
fn fit_to_monitor(window: &tauri::WebviewWindow) {
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
                    let _ = window.maximize();
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
    let desktop = app.state::<Arc<Desktop>>().inner().clone();
    let _one_at_a_time = desktop.locking.lock().await;
    flush_window(app).await;
    clear_window_state(app);
    if let Some(core) = app.try_state::<Arc<Core>>() {
        let core = core.inner().clone();
        core.lock().await;
    }
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
fn watch_sleep(app: &AppHandle) -> zbus::Result<()> {
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
fn lock_on_clock_gap(app: AppHandle) {
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

/// Let the app's own context menu cut, copy and paste: WebKitGTK blocks script clipboard access by default.
/// The webview only loads the bundled UI (CSP `default-src 'self'`).
#[cfg(target_os = "linux")]
/// WebKitGTK keeps the async clipboard API off per web view; every ODX window copies.
#[cfg(target_os = "linux")]
fn enable_clipboard_access(window: &tauri::WebviewWindow) {
    if let Err(error) = window.with_webview(|webview| {
        use webkit2gtk::{SettingsExt, WebViewExt};
        if let Some(settings) = webview.inner().settings() {
            settings.set_javascript_can_access_clipboard(true);
        }
    }) {
        eprintln!("odx: could not enable clipboard access: {error}");
    }
}

fn tray_menu(app: &AppHandle) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    use tauri::menu::{Menu, MenuItem};
    let item = |id: &str, text: &str| MenuItem::with_id(app, id, text, true, None::<&str>);
    Menu::with_items(
        app,
        &[
            &item("open", tr(app, "Open", "Abrir"))?,
            &item("lock", tr(app, "Lock", "Bloquear"))?,
            &item("quit", tr(app, "Quit", "Salir"))?,
        ],
    )
}

fn build_tray(app: &tauri::App) -> Result<(), String> {
    let menu = tray_menu(app.handle()).map_err(|e| e.to_string())?;
    let mut builder = tauri::tray::TrayIconBuilder::with_id("main")
        .tooltip("Odx")
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
fn with_attention_dot(icon: &tauri::image::Image<'_>) -> tauri::image::Image<'static> {
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

/// A slot for a thread blocked on a notification's action, released when it ends; bounds the threads that
/// notifications left open in the tray (GNOME keeps them until dismissed) can hold.
struct NotificationWaiter(AppHandle);

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
pub fn notify(app: &AppHandle, title: &'static str, body: String, approval: Option<String>) {
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
    // One blocking thread waits per notification with an action; past the cap, new ones show without "Open".
    let waiter =
        approval.and_then(|approval| NotificationWaiter::claim(app).map(|w| (w, approval)));
    let action = waiter.as_ref().map(|_| tr(app, "Open", "Abrir"));
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let shown = tauri::async_runtime::spawn_blocking(move || {
            let mut notification = notify_rust::Notification::new();
            notification.appname("Odx").summary(title).body(&body);
            #[cfg(target_os = "linux")]
            notification.icon(&icon.to_string_lossy());
            #[cfg(all(unix, not(target_os = "macos")))]
            if let Some(label) = action {
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
