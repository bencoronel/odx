//! Minimal HTTP CONNECT/forwarding and SOCKS5 test proxies (RFC 1928/1929). Tokio only, without extra dependencies.
#![allow(dead_code)]
use std::{
    net::SocketAddr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt, copy_bidirectional},
    net::{TcpListener, TcpStream},
    task::JoinHandle,
};

/// Only the proxy resolves this name to loopback. Direct DNS fails, so a response proves proxy traversal.
pub const HIDDEN_HOST: &str = "odoo.invalid";

#[derive(Clone, Default)]
pub struct ProxyLog {
    entries: Arc<Mutex<Vec<String>>>,
    connections: Arc<AtomicUsize>,
}
impl ProxyLog {
    /// CONNECT host:port, METHOD absolute-url, 407, SOCKS target or SOCKS auth-failed.
    pub fn entries(&self) -> Vec<String> {
        self.entries.lock().unwrap().clone()
    }
    pub fn connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }
    fn push(&self, entry: String) {
        self.entries.lock().unwrap().push(entry);
    }
}

pub struct TestProxy {
    pub addr: SocketAddr,
    pub log: ProxyLog,
    task: JoinHandle<()>,
}
impl TestProxy {
    /// Credential-free proxy URL with http or socks5h scheme.
    pub fn url(&self, scheme: &str) -> String {
        format!("{scheme}://{}", self.addr)
    }
}
impl Drop for TestProxy {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Basic encoding of user:password, matching HTTP clients.
fn basic(user: &str, password: &str) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let input = format!("{user}:{password}").into_bytes();
    let mut out = String::from("Basic ");
    for chunk in input.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Resolve the hidden hostname to loopback; resolve other names normally.
fn resolve(authority: &str) -> String {
    authority.replace(HIDDEN_HOST, "127.0.0.1")
}

async fn serve(
    auth: Option<(String, String)>,
    handler: fn(TcpStream, Option<(String, String)>, ProxyLog) -> ConnFuture,
) -> TestProxy {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let log = ProxyLog::default();
    let shared = log.clone();
    let task = tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            shared.connections.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(handler(stream, auth.clone(), shared.clone()));
        }
    });
    TestProxy { addr, log, task }
}
type ConnFuture = std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>;

/// HTTP proxy optionally requiring matching Proxy-Authorization: Basic; otherwise return 407.
pub async fn http_proxy(auth: Option<(&str, &str)>) -> TestProxy {
    serve(
        auth.map(|(u, p)| (u.into(), p.into())),
        |stream, auth, log| {
            Box::pin(async move {
                let _ = http_connection(stream, auth, log, false).await;
            })
        },
    )
    .await
}

/// HTTP proxy that only offers and accepts Proxy-Authorization: Digest (MD5, qop=auth).
pub async fn digest_proxy(user: &str, password: &str) -> TestProxy {
    serve(Some((user.into(), password.into())), |stream, auth, log| {
        Box::pin(async move {
            let _ = http_connection(stream, auth, log, true).await;
        })
    })
    .await
}

const DIGEST_NONCE: &str = "fixture-nonce";

/// Recompute the client's Digest response with the known password.
fn digest_valid(given: &str, user: &str, password: &str, method: &str, target: &str) -> bool {
    let Ok(mut header) = digest_auth::AuthorizationHeader::parse(given) else {
        return false;
    };
    let claimed = header.response.clone();
    let uri = header.uri.clone();
    header.digest(&digest_auth::AuthContext::new_with_method(
        user,
        password,
        uri.as_str(),
        None::<&[u8]>,
        digest_auth::HttpMethod(method.to_owned().into()),
    ));
    header.nonce == DIGEST_NONCE && uri == target && header.response == claimed
}

/// SOCKS5 optionally accepts only the configured username/password method.
pub async fn socks_proxy(auth: Option<(&str, &str)>) -> TestProxy {
    serve(
        auth.map(|(u, p)| (u.into(), p.into())),
        |stream, auth, log| {
            Box::pin(async move {
                let _ = socks_connection(stream, auth, log).await;
            })
        },
    )
    .await
}

/// Unused local port simulating an unavailable proxy.
pub async fn closed_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    listener.local_addr().unwrap().port()
}

async fn read_head(stream: &mut TcpStream) -> std::io::Result<(String, Vec<u8>)> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        if let Some(end) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
            let rest = buffer.split_off(end + 4);
            return Ok((String::from_utf8_lossy(&buffer).into_owned(), rest));
        }
        if buffer.len() > 64 * 1024 {
            return Err(std::io::Error::other("header too long"));
        }
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        buffer.extend_from_slice(&chunk[..n]);
    }
}

async fn http_connection(
    mut client: TcpStream,
    auth: Option<(String, String)>,
    log: ProxyLog,
    digest: bool,
) -> std::io::Result<()> {
    let (head, rest) = read_head(&mut client).await?;
    let mut lines = head.split("\r\n").filter(|l| !l.is_empty());
    let first = lines.next().unwrap_or_default().to_owned();
    let headers: Vec<&str> = lines.collect();
    let mut parts = first.split(' ');
    let (method, target, version) = (
        parts.next().unwrap_or_default(),
        parts.next().unwrap_or_default(),
        parts.next().unwrap_or("HTTP/1.1"),
    );
    if let Some((user, password)) = &auth {
        let given = headers.iter().find_map(|h| {
            h.split_once(':')
                .filter(|(name, _)| name.trim().eq_ignore_ascii_case("proxy-authorization"))
                .map(|(_, value)| value.trim().to_owned())
        });
        let valid = match (&given, digest) {
            (Some(given), true) => digest_valid(given, user, password, method, target),
            (given, false) => given.as_deref() == Some(basic(user, password).as_str()),
            (None, true) => false,
        };
        if !valid {
            log.push(format!("407 {method} {target}"));
            let challenge = if digest {
                format!(
                    "Digest realm=\"fixture\", nonce=\"{DIGEST_NONCE}\", qop=\"auth\", algorithm=MD5"
                )
            } else {
                "Basic realm=\"fixture\"".into()
            };
            client
                .write_all(
                    format!("HTTP/1.1 407 Proxy Authentication Required\r\nProxy-Authenticate: {challenge}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes(),
                )
                .await?;
            return Ok(());
        }
    }
    if method == "CONNECT" {
        log.push(format!("CONNECT {target}"));
        let mut upstream = TcpStream::connect(resolve(target)).await?;
        client
            .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
            .await?;
        upstream.write_all(&rest).await?;
        copy_bidirectional(&mut client, &mut upstream).await?;
        return Ok(());
    }
    // Forward absolute http://host:port/path in origin form; close afterward to record every request.
    log.push(format!("{method} {target}"));
    let without_scheme = target
        .strip_prefix("http://")
        .ok_or_else(|| std::io::Error::other("http:// targets only"))?;
    let (authority, path) = match without_scheme.find('/') {
        Some(i) => (&without_scheme[..i], &without_scheme[i..]),
        None => (without_scheme, "/"),
    };
    let mut upstream = TcpStream::connect(resolve(authority)).await?;
    let mut out = format!("{method} {path} {version}\r\n");
    for header in headers {
        let name = header.split(':').next().unwrap_or("").trim();
        if ["proxy-authorization", "proxy-connection", "connection"]
            .iter()
            .any(|n| name.eq_ignore_ascii_case(n))
        {
            continue;
        }
        out.push_str(header);
        out.push_str("\r\n");
    }
    out.push_str("Connection: close\r\n\r\n");
    upstream.write_all(out.as_bytes()).await?;
    upstream.write_all(&rest).await?;
    copy_bidirectional(&mut client, &mut upstream).await?;
    Ok(())
}

async fn socks_connection(
    mut client: TcpStream,
    auth: Option<(String, String)>,
    log: ProxyLog,
) -> std::io::Result<()> {
    let mut header = [0u8; 2];
    client.read_exact(&mut header).await?;
    let mut methods = vec![0u8; header[1] as usize];
    client.read_exact(&mut methods).await?;
    let wanted = if auth.is_some() { 0x02 } else { 0x00 };
    if header[0] != 5 || !methods.contains(&wanted) {
        log.push("SOCKS auth-failed".into());
        client.write_all(&[5, 0xFF]).await?;
        return Ok(());
    }
    client.write_all(&[5, wanted]).await?;
    if let Some((user, password)) = &auth {
        let mut version_len = [0u8; 2];
        client.read_exact(&mut version_len).await?;
        let mut given_user = vec![0u8; version_len[1] as usize];
        client.read_exact(&mut given_user).await?;
        let mut len = [0u8; 1];
        client.read_exact(&mut len).await?;
        let mut given_password = vec![0u8; len[0] as usize];
        client.read_exact(&mut given_password).await?;
        if given_user != user.as_bytes() || given_password != password.as_bytes() {
            log.push("SOCKS auth-failed".into());
            client.write_all(&[1, 1]).await?;
            return Ok(());
        }
        client.write_all(&[1, 0]).await?;
    }
    let mut request = [0u8; 4];
    client.read_exact(&mut request).await?;
    let host = match request[3] {
        1 => {
            let mut ip = [0u8; 4];
            client.read_exact(&mut ip).await?;
            std::net::Ipv4Addr::from(ip).to_string()
        }
        3 => {
            let mut len = [0u8; 1];
            client.read_exact(&mut len).await?;
            let mut name = vec![0u8; len[0] as usize];
            client.read_exact(&mut name).await?;
            String::from_utf8_lossy(&name).into_owned()
        }
        4 => {
            let mut ip = [0u8; 16];
            client.read_exact(&mut ip).await?;
            format!("[{}]", std::net::Ipv6Addr::from(ip))
        }
        _ => return Ok(()),
    };
    let mut port = [0u8; 2];
    client.read_exact(&mut port).await?;
    let target = format!("{host}:{}", u16::from_be_bytes(port));
    log.push(format!("SOCKS {target}"));
    let Ok(mut upstream) = TcpStream::connect(resolve(&target)).await else {
        // 0x05 means connection refused.
        client.write_all(&[5, 5, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
        return Ok(());
    };
    client.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
    copy_bidirectional(&mut client, &mut upstream).await?;
    Ok(())
}
