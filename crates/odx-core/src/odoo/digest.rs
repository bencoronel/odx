//! Digest proxy authentication. reqwest only sends Basic, so a loopback SOCKS5 relay opens each CONNECT tunnel with a fresh Digest challenge. The relay starts only after an http:// proxy rejects Basic and offers Digest alone.
use crate::proxy::ProxySettings;
use digest_auth::{AuthContext, HttpMethod};
use libsodium_rs::{random, utils};
use std::{io, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt, copy_bidirectional},
    net::{TcpListener, TcpStream},
    task::JoinHandle,
    time::timeout,
};

const HANDSHAKE: Duration = Duration::from_secs(5);
const MAX_HEAD: usize = 16 * 1024;
/// reqwest authenticates to the relay as this user with the per-relay random token.
pub(super) const RELAY_USER: &str = "odx";

/// Loopback relay owned by one pooled client; dropping it stops accepting tunnels.
pub(super) struct Relay {
    pub url: String,
    pub token: String,
    task: JoinHandle<()>,
}
impl Drop for Relay {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct Upstream {
    settings: ProxySettings,
    /// host:port of the http:// proxy.
    addr: String,
    token: String,
}

impl Relay {
    pub(super) async fn start(settings: ProxySettings) -> io::Result<Self> {
        let addr = http_proxy_addr(&settings)
            .ok_or_else(|| io::Error::other("Digest needs an http:// proxy"))?
            .to_owned();
        let mut bytes = [0u8; 32];
        random::fill_bytes(&mut bytes);
        let token = hex::encode(bytes);
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let url = format!("socks5h://{}", listener.local_addr()?);
        let upstream = Arc::new(Upstream {
            settings,
            addr,
            token: token.clone(),
        });
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let upstream = upstream.clone();
                tokio::spawn(async move {
                    let _ = relay(stream, &upstream).await;
                });
            }
        });
        Ok(Self { url, token, task })
    }
}

/// True when the proxy answers an unauthenticated CONNECT with Digest and without Basic.
pub(super) async fn offered(settings: &ProxySettings, authority: &str) -> bool {
    let Some(addr) = http_proxy_addr(settings).filter(|_| !settings.username.is_empty()) else {
        return false;
    };
    match timeout(HANDSHAKE, connect(addr, authority, None)).await {
        Ok(Ok((407, challenges, _))) => {
            challenges.iter().any(|c| scheme_is(c, "digest"))
                && !challenges.iter().any(|c| scheme_is(c, "basic"))
        }
        _ => false,
    }
}

// shortcut: TLS (https://) proxies keep Basic only; add a TLS client here if one needs Digest.
fn http_proxy_addr(settings: &ProxySettings) -> Option<&str> {
    settings.url.strip_prefix("http://")
}

fn scheme_is(challenge: &str, scheme: &str) -> bool {
    challenge
        .split_whitespace()
        .next()
        .is_some_and(|s| s.eq_ignore_ascii_case(scheme))
}

/// Send CONNECT and return the status, Proxy-Authenticate values and the stream positioned after the response head.
async fn connect(
    addr: &str,
    authority: &str,
    authorization: Option<&str>,
) -> io::Result<(u16, Vec<String>, TcpStream)> {
    let mut stream = TcpStream::connect(addr).await?;
    let mut head = format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n");
    if let Some(value) = authorization {
        head.push_str(&format!("Proxy-Authorization: {value}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).await?;
    // Byte reads leave tunnel bytes after the head in the socket.
    let mut buffer = Vec::new();
    while !buffer.ends_with(b"\r\n\r\n") {
        if buffer.len() > MAX_HEAD {
            return Err(io::Error::other("proxy response head too long"));
        }
        buffer.push(stream.read_u8().await?);
    }
    let text = String::from_utf8_lossy(&buffer);
    let mut lines = text.split("\r\n");
    let status = lines
        .next()
        .and_then(|l| l.split(' ').nth(1))
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| io::Error::other("invalid proxy response"))?;
    let challenges = lines
        .filter_map(|l| l.split_once(':'))
        .filter(|(name, _)| name.trim().eq_ignore_ascii_case("proxy-authenticate"))
        .flat_map(|(_, value)| split_challenges(value))
        .collect();
    Ok((status, challenges, stream))
}

/// One header may list several challenges: `Negotiate, Digest realm="a, b", nonce="n"`.
fn split_challenges(value: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let (mut start, mut quoted) = (0, false);
    for (i, c) in value.char_indices() {
        match c {
            '"' => quoted = !quoted,
            ',' if !quoted => {
                parts.push(&value[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&value[start..]);
    // A part whose first word has no '=' starts a new challenge; the rest are its parameters.
    let mut challenges: Vec<String> = Vec::new();
    for part in parts.iter().map(|p| p.trim()).filter(|p| !p.is_empty()) {
        let first = part.split_whitespace().next().unwrap_or_default();
        match challenges.last_mut() {
            Some(last) if first.contains('=') => {
                last.push_str(", ");
                last.push_str(part);
            }
            _ => challenges.push(part.to_owned()),
        }
    }
    challenges
}

/// Open a tunnel, answering a Digest challenge when the proxy sends one.
async fn tunnel(upstream: &Upstream, authority: &str) -> io::Result<TcpStream> {
    let (status, challenges, stream) = connect(&upstream.addr, authority, None).await?;
    if (200..300).contains(&status) {
        return Ok(stream);
    }
    let challenge = challenges
        .iter()
        .find(|c| scheme_is(c, "digest"))
        .filter(|_| status == 407)
        .ok_or_else(|| io::Error::other("proxy rejected CONNECT"))?;
    let mut prompt = digest_auth::parse(challenge).map_err(io::Error::other)?;
    let context = AuthContext::new_with_method(
        upstream.settings.username.as_str(),
        upstream.settings.password.as_str(),
        authority,
        None::<&[u8]>,
        HttpMethod::CONNECT,
    );
    let answer = prompt.respond(&context).map_err(io::Error::other)?;
    // shortcut: one extra connection and challenge per tunnel; reuse the nonce with nc+1 if tunnels become frequent.
    let (status, _, stream) =
        connect(&upstream.addr, authority, Some(&answer.to_header_string())).await?;
    if (200..300).contains(&status) {
        Ok(stream)
    } else {
        Err(io::Error::other("proxy rejected Digest credentials"))
    }
}

/// SOCKS5 CONNECT with username/password only (RFC 1928/1929).
async fn relay(mut client: TcpStream, upstream: &Upstream) -> io::Result<()> {
    let mut header = [0u8; 2];
    client.read_exact(&mut header).await?;
    let mut methods = vec![0u8; header[1] as usize];
    client.read_exact(&mut methods).await?;
    if header[0] != 5 || !methods.contains(&2) {
        return client.write_all(&[5, 0xFF]).await;
    }
    client.write_all(&[5, 2]).await?;
    let mut user = [0u8; 2];
    client.read_exact(&mut user).await?;
    let mut name = vec![0u8; user[1] as usize];
    client.read_exact(&mut name).await?;
    let mut len = [0u8; 1];
    client.read_exact(&mut len).await?;
    let mut password = vec![0u8; len[0] as usize];
    client.read_exact(&mut password).await?;
    if name != RELAY_USER.as_bytes() || !utils::memcmp(&password, upstream.token.as_bytes()) {
        return client.write_all(&[1, 1]).await;
    }
    client.write_all(&[1, 0]).await?;
    let mut request = [0u8; 4];
    client.read_exact(&mut request).await?;
    let host = match request {
        [5, 1, _, 1] => {
            let mut ip = [0u8; 4];
            client.read_exact(&mut ip).await?;
            std::net::Ipv4Addr::from(ip).to_string()
        }
        [5, 1, _, 3] => {
            client.read_exact(&mut len).await?;
            let mut name = vec![0u8; len[0] as usize];
            client.read_exact(&mut name).await?;
            String::from_utf8(name).map_err(io::Error::other)?
        }
        [5, 1, _, 4] => {
            let mut ip = [0u8; 16];
            client.read_exact(&mut ip).await?;
            format!("[{}]", std::net::Ipv6Addr::from(ip))
        }
        // 0x07: command not supported.
        _ => return client.write_all(&[5, 7, 0, 1, 0, 0, 0, 0, 0, 0]).await,
    };
    let mut port = [0u8; 2];
    client.read_exact(&mut port).await?;
    let authority = format!("{host}:{}", u16::from_be_bytes(port));
    match timeout(HANDSHAKE, tunnel(upstream, &authority)).await {
        Ok(Ok(mut stream)) => {
            client.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
            copy_bidirectional(&mut client, &mut stream).await?;
            Ok(())
        }
        // 0x02: connection not allowed by ruleset.
        _ => client.write_all(&[5, 2, 0, 1, 0, 0, 0, 0, 0, 0]).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_header_with_several_challenges_keeps_quoted_commas() {
        assert_eq!(
            split_challenges(
                r#"Negotiate, Digest realm="a, b", nonce="n", qop="auth", Basic realm="c""#
            ),
            [
                "Negotiate",
                r#"Digest realm="a, b", nonce="n", qop="auth""#,
                r#"Basic realm="c""#
            ]
        );
        assert_eq!(split_challenges("Basic realm=x"), ["Basic realm=x"]);
    }
}
