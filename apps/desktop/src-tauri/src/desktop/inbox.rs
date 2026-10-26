//! What the window takes from the core: agent requests (with their replies), table windows and the clicked approval.
use super::*;

#[derive(Default)]
pub(super) struct AgentQueue {
    pub(super) next: u64,
    pub(super) pending: Vec<AgentRequest>,
    pub(super) replies: std::collections::HashMap<u64, AgentReply>,
}

/// What the window takes with `agent_take`; `ipc.ts` `AgentRequest` mirrors it.
#[derive(Serialize, Debug, Clone)]
#[serde(
    tag = "kind",
    rename_all = "lowercase",
    rename_all_fields = "camelCase"
)]
pub enum AgentRequest {
    Run {
        id: u64,
        terminal: Option<String>,
        env_id: String,
        command: String,
        client_id: String,
        client_revision: u64,
        client_label: String,
        allow_writes: bool,
    },
    Show {
        id: u64,
        terminal: String,
    },
}

impl AgentRequest {
    pub(super) fn id(&self) -> u64 {
        match self {
            AgentRequest::Run { id, .. } | AgentRequest::Show { id, .. } => *id,
        }
    }
}

pub(super) type AgentReply = tokio::sync::oneshot::Sender<Result<serde_json::Value, String>>;

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
    // ODX's own frame, as the main window: no system title bar, shown after its first paint.
    .decorations(false)
    .visible(false)
    .build()?;
    #[cfg(target_os = "linux")]
    enable_clipboard_access(&window);
    Ok(())
}

/// `number` in zero-width characters, so tables of the same command still get distinct window titles.
pub(super) fn invisible_tag(number: u64) -> String {
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
            let id = {
                let mut queue = desktop.agent.lock().unwrap();
                queue.next += 1;
                queue.next
            };
            let (request, reply, show) = match request {
                odx_core::DesktopRequest::Run {
                    terminal,
                    env_id,
                    command,
                    client_id,
                    client_revision,
                    client_label,
                    allow_writes,
                    reply,
                } => {
                    if app.get_webview_window("main").is_none() {
                        let _ = reply.send(Err("The Odx window is closed: call show to open it, then run the command again.".into()));
                        continue;
                    }
                    let request = AgentRequest::Run {
                        id,
                        terminal,
                        env_id,
                        command,
                        client_id,
                        client_revision,
                        client_label,
                        allow_writes,
                    };
                    (request, reply, false)
                }
                odx_core::DesktopRequest::Show { terminal, reply } => {
                    (AgentRequest::Show { id, terminal }, reply, true)
                }
            };
            {
                let mut queue = desktop.agent.lock().unwrap();
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
pub fn take_agent_requests(app: &AppHandle) -> Vec<AgentRequest> {
    let desktop = app.state::<Arc<Desktop>>();
    let mut queue = desktop.agent.lock().unwrap();
    let AgentQueue {
        pending, replies, ..
    } = &mut *queue;
    // A caller that timed out never reads its reply: drop it so replies do not pile up.
    replies.retain(|_, reply| !reply.is_closed());
    std::mem::take(pending)
        .into_iter()
        .filter(|request| replies.contains_key(&request.id()))
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
