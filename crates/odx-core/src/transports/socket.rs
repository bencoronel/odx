//! Local socket v2 between mcp-stdio and the app. JSON frames use big-endian u32 lengths, 1 B–1 MiB. Same-UID peers must send {"v":2,"kind":"hello"} within 5 s, then request frames with id and MCP message (frame id ignored). The app returns MCP messages in notification frames. No admin commands.
use crate::{error::OdxError, execution::Core, transports::mcp::McpConnection};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{UnixListener, UnixStream},
    sync::{Semaphore, mpsc},
};

pub const WIRE_VERSION: u8 = 2;
pub const MAX_FRAME: usize = 1024 * 1024;
const HELLO_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_CONNECTIONS: usize = 32;
const ACCEPT_RETRY: Duration = Duration::from_millis(100);

/// Use $XDG_RUNTIME_DIR/<dir>/core.sock; fail if XDG_RUNTIME_DIR is invalid instead of guessing another location.
pub fn default_socket_path(dir_name: &str) -> Result<PathBuf, String> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute() && p.is_dir())
        .ok_or("XDG_RUNTIME_DIR is missing or is not a valid directory")?;
    Ok(dir.join(dir_name).join("core.sock"))
}

pub async fn read_frame<R: AsyncRead + Unpin>(reader: &mut R) -> std::io::Result<Option<Value>> {
    let mut len = [0u8; 4];
    match reader.read_exact(&mut len).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_be_bytes(len) as usize;
    if len == 0 || len > MAX_FRAME {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "invalid frame length",
        ));
    }
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf).await?;
    serde_json::from_slice(&buf)
        .map(Some)
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid JSON frame"))
}

pub async fn write_frame<W: AsyncWrite + Unpin>(
    writer: &mut W,
    value: &Value,
) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.is_empty() || bytes.len() > MAX_FRAME {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "frame too large",
        ));
    }
    writer
        .write_all(&(bytes.len() as u32).to_be_bytes())
        .await?;
    writer.write_all(&bytes).await?;
    writer.flush().await
}

/// Prepare the 0700 directory and remove stale sockets; fail if another app answers.
pub async fn bind(path: &Path) -> Result<UnixListener, String> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    let dir = path.parent().ok_or("invalid socket path")?;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    if path.exists() {
        if UnixStream::connect(path).await.is_ok() {
            return Err("another Odx instance is serving the MCP socket".into());
        }
        std::fs::remove_file(path).map_err(|e| format!("could not remove stale socket: {e}"))?;
    }
    let listener = UnixListener::bind(path).map_err(|e| format!("could not bind socket: {e}"))?;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    Ok(listener)
}

/// Accept connections until task cancellation.
pub async fn serve(core: Arc<Core>, listener: UnixListener) {
    let slots = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            // Back off on repeated errors such as EMFILE to avoid a busy loop.
            tokio::time::sleep(ACCEPT_RETRY).await;
            continue;
        };
        let Ok(permit) = slots.clone().try_acquire_owned() else {
            drop(stream); // Sin hueco: cierre inmediato, finito.
            continue;
        };
        let core = core.clone();
        tokio::spawn(async move {
            let _permit = permit;
            handle(core, stream).await;
        });
    }
}

fn connection_error(code: &str, message: &str) -> Value {
    json!({"v": WIRE_VERSION, "kind": "error", "code": code, "message": message})
}

async fn handle(core: Arc<Core>, stream: UnixStream) {
    // Authenticate with kernel identity, never clientInfo.
    let Ok(peer) = stream.peer_cred() else {
        return;
    };
    // SAFETY: geteuid takes no pointers and does not modify the process.
    if peer.uid() != unsafe { libc::geteuid() } {
        return;
    }
    let (mut reader, mut writer) = stream.into_split();
    let first = tokio::time::timeout(HELLO_TIMEOUT, read_frame(&mut reader)).await;
    if !matches!(first, Ok(Ok(Some(ref frame))) if frame["v"] == WIRE_VERSION && frame["kind"] == "hello")
    {
        let _ = write_frame(
            &mut writer,
            &connection_error(
                "incompatible_connector",
                "update the MCP connector together with the app",
            ),
        )
        .await;
        return;
    }
    let identity = match core.stdio_identity().await {
        Ok(identity) => identity,
        Err(OdxError::Locked) => {
            let _ = write_frame(
                &mut writer,
                &connection_error("locked", "Odx is locked; unlock it in the app"),
            )
            .await;
            return;
        }
        Err(_) => {
            let _ = write_frame(
                &mut writer,
                &connection_error("unavailable", "could not open the local MCP profile"),
            )
            .await;
            return;
        }
    };
    let _registered = core.register_connection(&identity.client_id);
    if write_frame(&mut writer, &json!({"v": WIRE_VERSION, "kind": "ready"}))
        .await
        .is_err()
    {
        return;
    }
    // Send MCP replies/notifications through one bounded channel; the connector forwards both without input-frame correlation.
    let (mcp_tx, mut mcp_rx) = mpsc::channel::<Value>(64);
    let mut writer_task = tokio::spawn(async move {
        while let Some(message) = mcp_rx.recv().await {
            let frame = json!({"v": WIRE_VERSION, "kind": "notification", "message": message});
            match write_frame(&mut writer, &frame).await {
                Ok(()) => {}
                // Too large for one frame (nothing was written): answer the request instead of dropping it.
                Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
                    let Some(id) = message.get("id").cloned() else {
                        continue;
                    };
                    let error = json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32603,
                        "message": "response exceeds the 1 MiB message limit; request less data or use to_file"}});
                    let frame =
                        json!({"v": WIRE_VERSION, "kind": "notification", "message": error});
                    if write_frame(&mut writer, &frame).await.is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
    let connection = McpConnection::new(core, identity, mcp_tx);
    loop {
        // Stop reading once replies can no longer be written, so no request runs unseen.
        let frame = tokio::select! {
            frame = read_frame(&mut reader) => frame,
            _ = &mut writer_task => break,
        };
        let frame = match frame {
            Ok(Some(frame)) => frame,
            _ => break, // EOF or invalid frames close and cancel pending requests.
        };
        if frame["v"] != WIRE_VERSION || frame["kind"] != "request" {
            break;
        }
        let raw = serde_json::to_vec(&frame["message"]).unwrap_or_default();
        connection.on_message(&raw).await;
    }
    drop(connection);
    writer_task.abort();
}
