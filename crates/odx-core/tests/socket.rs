//! Local socket v2 integration: hello deadline, connection cap and invalid lengths. No Odoo needed; reject before upstream access.
use odx_core::{Core, transports::socket};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
};

struct Server {
    core: Arc<Core>,
    path: PathBuf,
    _tmp: tempfile::TempDir,
}

async fn server(unlocked: bool) -> Server {
    let tmp = tempfile::tempdir().unwrap();
    let core = Core::new(tmp.path().join("vault.sqlite3")).unwrap();
    if unlocked {
        core.create("fictional master password").await.unwrap();
    }
    let path = tmp.path().join("run").join("core.sock");
    let listener = socket::bind(&path).await.unwrap();
    tokio::spawn(socket::serve(core.clone(), listener));
    Server {
        core,
        path,
        _tmp: tmp,
    }
}

async fn hello(path: &PathBuf) -> (UnixStream, Value) {
    let mut s = UnixStream::connect(path).await.unwrap();
    socket::write_frame(&mut s, &json!({"v": socket::WIRE_VERSION, "kind": "hello"}))
        .await
        .unwrap();
    let reply = tokio::time::timeout(Duration::from_secs(5), socket::read_frame(&mut s))
        .await
        .expect("hello response")
        .unwrap()
        .expect("frame");
    (s, reply)
}

/// Read frames until EOF within a deadline.
async fn until_closed(s: &mut UnixStream, within: Duration) -> Vec<Value> {
    tokio::time::timeout(within, async {
        let mut frames = vec![];
        while let Ok(Some(frame)) = socket::read_frame(s).await {
            frames.push(frame);
        }
        frames
    })
    .await
    .expect("server closes the connection")
}

#[tokio::test]
async fn silent_connections_close_after_five_seconds_and_cap_is_32() {
    let srv = server(false).await;
    let start = Instant::now();
    let mut silent = vec![];
    for _ in 0..32 {
        silent.push(UnixStream::connect(&srv.path).await.unwrap());
    }
    // Close the 33rd connection immediately without a frame.
    let mut extra = UnixStream::connect(&srv.path).await.unwrap();
    assert!(
        until_closed(&mut extra, Duration::from_secs(1))
            .await
            .is_empty()
    );
    assert!(start.elapsed() < Duration::from_secs(4));

    // Close 32 peers without hello after 5 s with a connection error.
    for s in &mut silent {
        let frames = until_closed(s, Duration::from_secs(8)).await;
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0]["kind"], "error");
        assert_eq!(frames[0]["code"], "incompatible_connector");
    }
    let elapsed = start.elapsed();
    assert!(
        elapsed >= Duration::from_millis(4900) && elapsed < Duration::from_secs(8),
        "{elapsed:?}"
    );

    // After capacity is released, new peers can greet and receive locked responses.
    let (_s, reply) = hello(&srv.path).await;
    assert_eq!(reply["code"], "locked");
    assert_eq!(srv.core.status().await, "absent", "vault not created");
}

#[tokio::test]
async fn zero_or_oversized_frames_close_only_that_connection() {
    let srv = server(true).await;
    let header = |len: u32| len.to_be_bytes();

    // Before hello, reject zero/oversized lengths without awaiting or allocating a body.
    for len in [0, socket::MAX_FRAME as u32 + 1, u32::MAX] {
        let mut s = UnixStream::connect(&srv.path).await.unwrap();
        s.write_all(&header(len)).await.unwrap();
        let frames = until_closed(&mut s, Duration::from_secs(2)).await;
        assert_eq!(frames.len(), 1, "longitud {len}");
        assert_eq!(frames[0]["code"], "incompatible_connector");
    }

    // After hello, invalid frames close without a response.
    let (mut keep, reply) = hello(&srv.path).await;
    assert_eq!(reply["kind"], "ready");
    for len in [0, socket::MAX_FRAME as u32 + 1] {
        let (mut s, reply) = hello(&srv.path).await;
        assert_eq!(reply["kind"], "ready");
        s.write_all(&header(len)).await.unwrap();
        assert!(
            until_closed(&mut s, Duration::from_secs(2))
                .await
                .is_empty(),
            "longitud {len}"
        );
    }
    // A 1 MiB frame meets the limit; invalid JSON closes without panic.
    let (mut s, _) = hello(&srv.path).await;
    s.write_all(&header(socket::MAX_FRAME as u32))
        .await
        .unwrap();
    s.write_all(&vec![b' '; socket::MAX_FRAME]).await.unwrap();
    assert!(
        until_closed(&mut s, Duration::from_secs(2))
            .await
            .is_empty()
    );

    // The server and other connections remain responsive.
    let discover = json!({"jsonrpc":"2.0","id":1,"method":"server/discover","params":{"_meta":{
        "io.modelcontextprotocol/protocolVersion":"2026-07-28",
        "io.modelcontextprotocol/clientCapabilities":{}}}});
    socket::write_frame(
        &mut keep,
        &json!({"v": socket::WIRE_VERSION, "kind": "request", "id": "1", "message": discover}),
    )
    .await
    .unwrap();
    let r = socket::read_frame(&mut keep).await.unwrap().unwrap();
    assert_eq!(r["message"]["id"], 1);
    assert_eq!(
        r["message"]["result"]["supportedVersions"],
        json!(["2026-07-28"])
    );
    let (_s, reply) = hello(&srv.path).await;
    assert_eq!(reply["kind"], "ready");
    let mut probe = [0u8; 1];
    assert!(
        tokio::time::timeout(Duration::from_millis(50), keep.read(&mut probe))
            .await
            .is_err(),
        "valid connection remains open without pending data"
    );
}
