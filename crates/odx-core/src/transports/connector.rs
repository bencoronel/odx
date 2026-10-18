//! mcp-stdio bridges MCP stdin/stdout to the app socket. Stdout carries one MCP message per line; diagnostics use stderr. Stores no vault or Odoo credentials and requires no proxy token.
use crate::{
    config::validate_json,
    transports::socket::{MAX_FRAME, WIRE_VERSION, read_frame, write_frame},
};
use serde_json::{Value, json};
use std::{path::Path, time::Duration};
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt},
    net::UnixStream,
};

/// Read at most MAX_FRAME bytes per line; Ok(None) on EOF.
async fn read_line<R: AsyncBufRead + Unpin>(
    input: &mut R,
) -> std::io::Result<Option<Result<Vec<u8>, ()>>> {
    let mut line = Vec::new();
    let mut too_long = false;
    loop {
        let buf = input.fill_buf().await?;
        if buf.is_empty() {
            return Ok(if line.is_empty() && !too_long {
                None
            } else {
                Some(if too_long { Err(()) } else { Ok(line) })
            });
        }
        let (chunk, done) = match buf.iter().position(|b| *b == b'\n') {
            Some(i) => (&buf[..i], Some(i + 1)),
            None => (buf, None),
        };
        if !too_long {
            if line.len() + chunk.len() > MAX_FRAME {
                too_long = true;
                line.clear();
            } else {
                line.extend_from_slice(chunk);
            }
        }
        let consumed = done.unwrap_or(buf.len());
        input.consume(consumed);
        if done.is_some() {
            return Ok(Some(if too_long { Err(()) } else { Ok(line) }));
        }
    }
}

async fn emit<W: AsyncWrite + Unpin>(out: &mut W, message: &Value) -> std::io::Result<()> {
    let mut line = serde_json::to_vec(message)?;
    line.push(b'\n');
    out.write_all(&line).await?;
    out.flush().await
}

/// Answer the client's first request with `reason`, so MCP clients can show why Odx is unavailable instead of a bare failure.
/// Notifications get no reply; stop after one reply, EOF or 10 s.
pub async fn report<R, W>(mut input: R, mut output: W, reason: &str)
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let first_request = async {
        while let Ok(Some(Ok(line))) = read_line(&mut input).await {
            if let Ok(message) = validate_json(&line)
                && let Some(id) = message
                    .get("id")
                    .filter(|id| id.is_string() || id.is_number())
            {
                return Some(id.clone());
            }
        }
        None
    };
    if let Ok(Some(id)) = tokio::time::timeout(Duration::from_secs(10), first_request).await {
        let reply = json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32000, "message": format!("Odx unavailable: {reason}")}});
        let _ = emit(&mut output, &reply).await;
    }
}

/// Run until stdin EOF or socket loss; return errors for stderr. Connection failures are also reported to the client.
pub async fn run<R, W>(socket: &Path, mut input: R, mut output: W) -> Result<(), String>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (mut reader, mut writer) = match connect(socket).await {
        Ok(halves) => halves,
        Err(reason) => {
            report(&mut input, &mut output, &reason).await;
            return Err(reason);
        }
    };
    // One stdout writer handles app messages and local framing errors.
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel::<Value>(64);
    let stdout_task = tokio::spawn(async move {
        while let Some(message) = out_rx.recv().await {
            if emit(&mut output, &message).await.is_err() {
                break;
            }
        }
    });
    // Socket loss ends forwarding without reconnecting.
    let socket_tx = out_tx.clone();
    let mut pump = tokio::spawn(async move {
        loop {
            match read_frame(&mut reader).await {
                Ok(Some(frame))
                    if frame["v"] == WIRE_VERSION
                        && matches!(frame["kind"].as_str(), Some("response" | "notification")) =>
                {
                    if socket_tx.send(frame["message"].clone()).await.is_err() {
                        return;
                    }
                }
                _ => return,
            }
        }
    });
    let mut seq: u64 = 0;
    let result = loop {
        tokio::select! {
            line = read_line(&mut input) => {
                let line = match line {
                    Ok(Some(line)) => line,
                    Ok(None) => break Ok(()),
                    Err(_) => break Err("could not read stdin".to_string()),
                };
                let message = match line {
                    Err(()) => Err(json!({"jsonrpc":"2.0","id":null,"error":{"code":-32600,"message":"Invalid Request: message exceeds 1 MiB"}})),
                    Ok(bytes) if bytes.iter().all(u8::is_ascii_whitespace) => continue,
                    Ok(bytes) => validate_json(&bytes).map_err(|_| json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"Parse error"}})),
                };
                match message {
                    Ok(message) => {
                        seq += 1;
                        let frame = json!({"v": WIRE_VERSION, "kind": "request", "id": seq.to_string(), "message": message});
                        // Reject envelopes exceeding the frame limit even if the input line fits; keep the session open.
                        if !serde_json::to_vec(&frame).is_ok_and(|b| b.len() <= MAX_FRAME) {
                            let id = message.get("id").filter(|id| id.is_string() || id.is_number()).cloned().unwrap_or(Value::Null);
                            let _ = out_tx.send(json!({"jsonrpc":"2.0","id":id,"error":{"code":-32600,"message":"Invalid Request: message exceeds 1 MiB"}})).await;
                            continue;
                        }
                        if write_frame(&mut writer, &frame).await.is_err() {
                            break Err("Odx connection lost".to_string());
                        }
                    }
                    Err(reply) => { let _ = out_tx.send(reply).await; }
                }
            }
            _ = &mut pump => break Err("Odx closed the connection; check whether the app locked or exited".to_string()),
        }
    };
    drop(writer); // App EOF cancels this connection's unsent work.
    if result.is_ok() {
        // Deliver buffered messages without retrying.
        let _ = tokio::time::timeout(Duration::from_millis(500), &mut pump).await;
    }
    pump.abort();
    drop(out_tx);
    let _ = stdout_task.await;
    result
}

/// Connect to the app socket, check its user and complete the hello handshake.
async fn connect(
    socket: &Path,
) -> Result<
    (
        tokio::net::unix::OwnedReadHalf,
        tokio::net::unix::OwnedWriteHalf,
    ),
    String,
> {
    let stream = tokio::time::timeout(Duration::from_secs(5), UnixStream::connect(socket))
        .await
        .map_err(|_| "timed out connecting to Odx".to_string())?
        .map_err(|_| "Odx is not open; could not connect to its local socket".to_string())?;
    let peer = stream
        .peer_cred()
        .map_err(|_| "could not verify the Odx user")?;
    // SAFETY: geteuid takes no pointers and does not modify the process.
    if peer.uid() != unsafe { libc::geteuid() } {
        return Err("MCP socket belongs to another user".into());
    }
    let (mut reader, mut writer) = stream.into_split();
    write_frame(&mut writer, &json!({"v": WIRE_VERSION, "kind": "hello"}))
        .await
        .map_err(|_| "could not establish the Odx connection".to_string())?;
    let reply = tokio::time::timeout(Duration::from_secs(5), read_frame(&mut reader))
        .await
        .map_err(|_| "Odx did not respond to the connection".to_string())?
        .map_err(|_| "Odx connection lost".to_string())?
        .ok_or("Odx connection closed")?;
    if reply["v"] != WIRE_VERSION || reply["kind"] != "ready" {
        return Err(format!(
            "Odx rejected the connection: {}",
            reply["message"].as_str().unwrap_or("unknown error")
        ));
    }
    Ok((reader, writer))
}
