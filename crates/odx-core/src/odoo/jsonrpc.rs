use super::{json2, xmlrpc};
use crate::proxy::{EnvironmentProxy, ProxySettings};
use crate::{
    config::{Environment, Protocol},
    error::{OdxError, Result},
};
use reqwest::{Client, redirect::Policy};
use serde::Serialize;
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::{Mutex, OnceCell};
use zeroize::Zeroizing;

/// Ordinary response limit; `download` has a separate limit.
pub(crate) const MAX_RESPONSE: usize = 16 * 1024 * 1024;

#[derive(Serialize)]
struct RpcParams<'a, A> {
    service: &'a str,
    method: &'a str,
    args: A,
}
#[derive(Serialize)]
struct RpcRequest<'a, A> {
    jsonrpc: &'static str,
    method: &'static str,
    params: RpcParams<'a, A>,
    id: u32,
}

/// Cache uid by environment, revision and protocol. Share concurrent logins; leave failures uncached for the next attempt.
type AuthCache = HashMap<(String, u64, Protocol), Arc<OnceCell<i64>>>;
type ProtocolCache = HashMap<(String, u64), Arc<OnceCell<Protocol>>>;

/// One pooled HTTP client per effective proxy; `None` is direct. A Digest proxy's client goes through its own relay.
type ClientPool = Vec<(Option<ProxySettings>, Client, Option<super::digest::Relay>)>;

#[derive(Clone)]
pub struct OdooClient {
    clients: Arc<std::sync::Mutex<ClientPool>>,
    auth: Arc<Mutex<AuthCache>>,
    detected: Arc<Mutex<ProtocolCache>>,
    /// Request timeout in seconds; the HTTP client caps it at 60 s.
    timeout_secs: Arc<std::sync::atomic::AtomicU64>,
}
impl OdooClient {
    pub fn new() -> Result<Self> {
        Ok(Self {
            clients: Arc::new(std::sync::Mutex::new(Vec::new())),
            auth: Arc::new(Mutex::new(HashMap::new())),
            detected: Arc::new(Mutex::new(HashMap::new())),
            timeout_secs: Arc::new(std::sync::atomic::AtomicU64::new(60)),
        })
    }
    /// Use the environment's resolved proxy, ignoring system HTTP_PROXY/HTTPS_PROXY/ALL_PROXY/NO_PROXY. Never default unresolved proxies to direct access.
    pub(super) fn http(&self, env: &Environment) -> Result<Client> {
        let proxy = match &env.proxy {
            EnvironmentProxy::Custom(p) => Some(p),
            EnvironmentProxy::Direct => None,
            EnvironmentProxy::Inherit | EnvironmentProxy::Profile { .. } => {
                return Err(OdxError::validation(
                    "validation.proxy.configuration_invalid",
                    "unresolved environment proxy",
                ));
            }
        };
        let mut clients = self.clients.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((_, client, _)) = clients.iter().find(|(p, _, _)| p.as_ref() == proxy) {
            return Ok(client.clone());
        }
        let config = match proxy {
            None => None,
            Some(p) => {
                // Keep credentials outside URLs to exclude them from errors.
                let mut config = reqwest::Proxy::all(p.url.as_str()).map_err(|_| {
                    OdxError::validation("validation.proxy.url_invalid", "invalid proxy URL")
                })?;
                if !p.username.is_empty() {
                    config = config.basic_auth(p.username.as_str(), p.password.as_str());
                }
                Some(config)
            }
        };
        let client = build_client(config)?;
        clients.push((proxy.cloned(), client.clone(), None));
        Ok(client)
    }
    /// Send through the environment's proxy. When the proxy rejects Basic and offers only Digest, its pooled client switches to the Digest relay and the request is retried once; a rejected request never reached Odoo.
    pub(super) async fn send(
        &self,
        env: &Environment,
        request: impl Fn(Client) -> reqwest::RequestBuilder,
    ) -> Result<reqwest::Response> {
        let attempt = async || {
            request(self.http(env)?)
                .send()
                .await
                .map_err(|e| network_error(&e, env))
                .and_then(|r| proxy_rejected(r, env))
        };
        let result = match attempt().await {
            Err(e) if auth_rejected(&e) && self.switch_to_digest(env).await => attempt().await,
            result => result,
        };
        match result {
            // The relay reports failures as SOCKS errors; name the proxy the user configured.
            Err(OdxError::Network { kind: "proxy", .. }) if self.relayed(env) => {
                Err(OdxError::Network {
                    kind: "proxy",
                    message: "proxy rejected the Digest login or the tunnel".into(),
                })
            }
            result => result,
        }
    }
    fn relayed(&self, env: &Environment) -> bool {
        let EnvironmentProxy::Custom(p) = &env.proxy else {
            return false;
        };
        relayed(&self.clients.lock().unwrap_or_else(|e| e.into_inner()), p)
    }
    async fn switch_to_digest(&self, env: &Environment) -> bool {
        let EnvironmentProxy::Custom(p) = &env.proxy else {
            return false;
        };
        // Another request switched after this one used the Basic client.
        if self.relayed(env) {
            return true;
        }
        let Some(authority) = url::Url::parse(&env.url)
            .ok()
            .and_then(|u| Some(format!("{}:{}", u.host_str()?, u.port_or_known_default()?)))
        else {
            return false;
        };
        if !super::digest::offered(p, &authority).await {
            return false;
        }
        let Ok(relay) = super::digest::Relay::start(p.clone()).await else {
            return false;
        };
        let Ok(config) = reqwest::Proxy::all(relay.url.as_str()) else {
            return false;
        };
        let Ok(client) = build_client(Some(
            config.basic_auth(super::digest::RELAY_USER, &relay.token),
        )) else {
            return false;
        };
        let mut clients = self.clients.lock().unwrap_or_else(|e| e.into_inner());
        // A concurrent request may have switched first; keep its relay so in-flight retries still reach it.
        if !relayed(&clients, p) {
            clients.retain(|(q, _, _)| q.as_ref() != Some(p));
            clients.push((Some(p.clone()), client, Some(relay)));
        }
        true
    }
    /// Drop unused proxy clients; retain the direct client.
    pub(crate) fn retain_clients(&self, active: &[Option<ProxySettings>]) {
        self.clients
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|(p, _, _)| p.is_none() || active.contains(p));
    }
    pub(crate) fn clear_clients(&self) {
        self.clients
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }
    pub(crate) fn set_timeout(&self, secs: u32) {
        self.timeout_secs
            .store(u64::from(secs), std::sync::atomic::Ordering::Release);
    }
    pub(crate) fn request_timeout(&self) -> Duration {
        Duration::from_secs(
            self.timeout_secs
                .load(std::sync::atomic::Ordering::Acquire)
                .min(60),
        )
    }
    async fn rpc<A: Serialize>(
        &self,
        env: &Environment,
        service: &str,
        method: &str,
        args: A,
    ) -> Result<Value> {
        self.rpc_measured(env, service, method, args, MAX_RESPONSE)
            .await
            .map(|(value, _)| value)
    }
    /// JSON-RPC with a bounded response; return value and received bytes.
    async fn rpc_measured<A: Serialize>(
        &self,
        env: &Environment,
        service: &str,
        method: &str,
        args: A,
        max_response: usize,
    ) -> Result<(Value, usize)> {
        let request = RpcRequest {
            jsonrpc: "2.0",
            method: "call",
            params: RpcParams {
                service,
                method,
                args,
            },
            id: 1,
        };
        let body = bytes::Bytes::from_owner(crate::security::WipeBody(Zeroizing::new(
            serde_json::to_vec(&request)?,
        )));
        let headers = crate::headers::request_headers(&env.headers)?;
        let response = self
            .send(env, |http| {
                http.post(format!("{}/jsonrpc", env.url))
                    .header(reqwest::header::CONTENT_TYPE, "application/json")
                    .body(body.clone())
                    .headers(headers.clone())
                    .timeout(self.request_timeout())
            })
            .await?;
        if !response.status().is_success() {
            return Err(OdxError::HttpStatus {
                status: response.status().as_u16(),
            });
        }
        let bytes = read_limited(response, env, max_response).await?;
        let size = bytes.len();
        let value = parse_json(bytes)
            .await
            .ok_or_else(|| OdxError::Upstream("invalid JSON-RPC response".into()))?;
        if let Some(err) = value.get("error") {
            let message = err
                .pointer("/data/message")
                .and_then(Value::as_str)
                .or_else(|| err.get("message").and_then(Value::as_str))
                .unwrap_or("remote error");
            return Err(OdxError::Remote(scrub(message, env)));
        }
        value
            .get("result")
            .cloned()
            .map(|v| (v, size))
            .ok_or_else(|| OdxError::Upstream("incomplete JSON-RPC response".into()))
    }
    pub(crate) async fn favicon_page(
        &self,
        env: &Environment,
    ) -> Result<crate::favicon::FaviconPage> {
        let mut url =
            url::Url::parse(&format!("{}/", env.url.trim_end_matches('/'))).map_err(|_| {
                OdxError::validation("validation.environment.url_invalid", "invalid URL")
            })?;
        url.query_pairs_mut().append_pair("db", &env.database);
        let (url, _, bytes) = self.favicon_resource(env, url, "text/html").await?;
        Ok(crate::favicon::FaviconPage {
            url: url.to_string(),
            html: String::from_utf8_lossy(&bytes).into_owned(),
        })
    }

    pub(crate) async fn favicon_image(
        &self,
        env: &Environment,
        url: &str,
    ) -> Result<crate::favicon::FaviconImage> {
        let url = url::Url::parse(url).map_err(|_| {
            OdxError::validation("validation.environment.url_invalid", "invalid URL")
        })?;
        let (_, content_type, data) = self.favicon_resource(env, url, "image/*").await?;
        if !content_type.starts_with("image/") {
            return Err(OdxError::Upstream("favicon is not an image".into()));
        }
        Ok(crate::favicon::FaviconImage { content_type, data })
    }

    async fn favicon_resource(
        &self,
        env: &Environment,
        mut url: url::Url,
        accept: &str,
    ) -> Result<(url::Url, String, Vec<u8>)> {
        const MAX_RESOURCE: usize = 256 * 1024;
        // Enforce the environment origin (scheme, host and port), including redirects.
        let origin = url::Url::parse(&env.url)
            .map_err(|_| OdxError::validation("validation.environment.url_invalid", "invalid URL"))?
            .origin();
        let allowed = |u: &url::Url| {
            matches!(u.scheme(), "http" | "https")
                && u.username().is_empty()
                && u.password().is_none()
                && u.origin() == origin
        };
        if !allowed(&url) {
            return Err(OdxError::validation(
                "validation.environment.favicon_url_not_allowed",
                "favicon URL not allowed",
            ));
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        for _ in 0..=3 {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            let headers = crate::headers::request_headers(&env.headers)?;
            let response = self
                .send(env, |http| {
                    http.get(url.clone())
                        .header(reqwest::header::ACCEPT, accept)
                        .headers(headers.clone())
                        .timeout(remaining)
                })
                .await?;
            if response.status().is_redirection() {
                let target = response
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| url.join(v).ok())
                    .filter(allowed)
                    .ok_or_else(|| OdxError::Upstream("invalid favicon redirect".into()))?;
                url = target;
                continue;
            }
            if !response.status().is_success() {
                return Err(OdxError::HttpStatus {
                    status: response.status().as_u16(),
                });
            }
            let content_type = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            let mut bytes =
                read_limited(response, env, MAX_RESOURCE)
                    .await
                    .map_err(|e| match e {
                        OdxError::ResponseTooLarge { .. } => {
                            OdxError::Upstream("favicon resource too large".into())
                        }
                        e => e,
                    })?;
            return Ok((url, content_type, std::mem::take(&mut *bytes)));
        }
        Err(OdxError::Upstream("could not fetch favicon page".into()))
    }
    pub async fn authenticate(&self, env: &Environment) -> Result<i64> {
        self.authenticate_measured(env).await.map(|(uid, _)| uid)
    }
    /// Environment uid and login response bytes, if this call logged in.
    pub async fn authenticate_measured(&self, env: &Environment) -> Result<(i64, Option<usize>)> {
        self.authenticate_inner(env, false).await
    }
    async fn authenticate_inner(
        &self,
        env: &Environment,
        probe: bool,
    ) -> Result<(i64, Option<usize>)> {
        let cell = {
            let mut cache = self.auth.lock().await;
            let key = (env.id.clone(), env.revision, env.protocol);
            if !cache.contains_key(&key) {
                cache.retain(|(id, revision, _), _| id != &env.id || *revision == env.revision);
            }
            cache.entry(key).or_default().clone()
        };
        let mut bytes = None;
        let uid = *cell
            .get_or_try_init(|| async {
                // JSON-2 sends the key on every call and resolves uid through res.users/context_get, per Odoo 19 documentation.
                let (uid, size) = match env.protocol {
                    Protocol::Auto => return Err(unresolved_protocol()),
                    Protocol::LegacyJsonrpc => {
                        self.rpc_measured(
                            env,
                            "common",
                            "login",
                            (&env.database, &env.login, env.secret.as_str()),
                            MAX_RESPONSE,
                        )
                        .await?
                    }
                    Protocol::Xmlrpc => {
                        xmlrpc::call(
                            self,
                            env,
                            "common",
                            "login",
                            &[
                                xmlrpc::Param::Str(&env.database),
                                xmlrpc::Param::Str(&env.login),
                                xmlrpc::Param::Str(env.secret.as_str()),
                            ],
                            MAX_RESPONSE,
                        )
                        .await?
                    }
                    Protocol::Json2 if probe => json2::probe_user_id(self, env).await?,
                    Protocol::Json2 => json2::user_id(self, env).await?,
                };
                bytes = Some(size);
                uid.as_i64()
                    .filter(|v| *v > 0)
                    .ok_or(OdxError::AuthenticationRejected)
            })
            .await?;
        Ok((uid, bytes))
    }
    /// Unauthenticated common.version verifies the URL serves Odoo.
    pub async fn version(&self, env: &Environment) -> Result<Option<String>> {
        let value = match env.protocol {
            Protocol::Auto => return Err(unresolved_protocol()),
            Protocol::LegacyJsonrpc => self.rpc(env, "common", "version", json!([])).await?,
            Protocol::Xmlrpc => {
                xmlrpc::call(self, env, "common", "version", &[], MAX_RESPONSE)
                    .await?
                    .0
            }
            Protocol::Json2 => return json2::version(self, env).await,
        };
        Ok(value
            .get("server_version")
            .and_then(Value::as_str)
            .map(|v| v.chars().take(64).collect()))
    }
    /// `db.list` over JSON-RPC, falling back to XML-RPC where `/jsonrpc` is missing.
    pub async fn list_databases(&self, env: &Environment) -> Result<Vec<String>> {
        let value = match self.rpc(env, "db", "list", json!([])).await {
            Err(OdxError::HttpStatus { status: 404 | 405 }) => {
                xmlrpc::call(self, env, "db", "list", &[], MAX_RESPONSE)
                    .await?
                    .0
            }
            other => other?,
        };
        let names = value
            .as_array()
            .ok_or_else(|| OdxError::Upstream("invalid database list".into()))?;
        Ok(names
            .iter()
            .filter_map(Value::as_str)
            .take(500)
            .map(str::to_owned)
            .collect())
    }
    /// Discover using version/authentication probes only. Only 404/405, or a redirect from the credential-less `/web/version` probe (Odoo ≤18), allow another endpoint; JSON-2 401 requires successful RPC authentication with the same credential before accepting legacy RPC.
    pub(crate) async fn detect_protocol<F, Fut>(
        &self,
        env: &Environment,
        check: F,
    ) -> Result<Protocol>
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = Result<()>>,
    {
        let cell = {
            let mut cache = self.detected.lock().await;
            cache.retain(|(id, revision), _| id != &env.id || *revision == env.revision);
            cache
                .entry((env.id.clone(), env.revision))
                .or_default()
                .clone()
        };
        let detected = cell
            .get_or_try_init(|| async {
                let mut candidate = env.clone();
                candidate.protocol = Protocol::Json2;
                let mut key_rejected = false;
                check().await?;
                match json2::probe_version(self, &candidate).await {
                    Ok(Some(version)) => {
                        let major = version
                            .split('.')
                            .next()
                            .and_then(|v| v.parse::<u32>().ok());
                        if major.is_some_and(|v| v >= 19) {
                            check().await?;
                            match self.authenticate_inner(&candidate, true).await {
                                Ok(_) => return Ok(Protocol::Json2),
                                Err(OdxError::HttpStatus { status: 404 | 405 }) => {}
                                Err(OdxError::AuthenticationRejected) => key_rejected = true,
                                Err(error) => return Err(error),
                            }
                        }
                    }
                    // Redirects are not followed; some Odoo 16 servers redirect this unknown route.
                    Err(OdxError::HttpStatus {
                        status: 300..=399 | 404 | 405,
                    }) => {}
                    Ok(None) => {
                        return Err(OdxError::Upstream(
                            "incomplete Odoo version response".into(),
                        ));
                    }
                    Err(error) => return Err(error),
                }
                for protocol in [Protocol::LegacyJsonrpc, Protocol::Xmlrpc] {
                    candidate.protocol = protocol;
                    check().await?;
                    match self.version(&candidate).await {
                        Ok(Some(_)) => {
                            check().await?;
                            self.authenticate_inner(&candidate, true).await?;
                            return Ok(protocol);
                        }
                        Err(OdxError::HttpStatus { status: 404 | 405 }) => {}
                        Ok(None) => {
                            return Err(OdxError::Upstream(
                                "incomplete Odoo version response".into(),
                            ));
                        }
                        Err(error) => return Err(error),
                    }
                }
                if key_rejected {
                    Err(OdxError::AuthenticationRejected)
                } else {
                    Err(OdxError::Upstream(
                        "no compatible Odoo API found; check the URL or select a protocol manually"
                            .into(),
                    ))
                }
            })
            .await?;
        Ok(*detected)
    }

    pub async fn invalidate_environment(&self, id: &str) {
        self.auth
            .lock()
            .await
            .retain(|(env_id, _, _), _| env_id != id);
        self.detected
            .lock()
            .await
            .retain(|(env_id, _), _| env_id != id);
    }
    pub async fn clear_auth(&self) {
        self.auth.lock().await.clear();
        self.detected.lock().await.clear();
    }
    /// Call `execute_kw`, with response byte counts.
    pub async fn execute_kw_measured(
        &self,
        env: &Environment,
        uid: i64,
        model: &str,
        method: &str,
        args: Value,
        kwargs: Value,
    ) -> Result<(Value, usize)> {
        self.execute_kw_limited(env, uid, model, method, args, kwargs, MAX_RESPONSE)
            .await
    }
    /// Bound response bytes across all three protocols; overflow returns ResponseTooLarge without partial data.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn execute_kw_limited(
        &self,
        env: &Environment,
        uid: i64,
        model: &str,
        method: &str,
        args: Value,
        kwargs: Value,
        max_response: usize,
    ) -> Result<(Value, usize)> {
        match env.protocol {
            Protocol::Auto => return Err(unresolved_protocol()),
            Protocol::LegacyJsonrpc => {}
            Protocol::Xmlrpc => {
                return xmlrpc::call(
                    self,
                    env,
                    "object",
                    "execute_kw",
                    &[
                        xmlrpc::Param::Str(&env.database),
                        xmlrpc::Param::Int(uid),
                        xmlrpc::Param::Str(env.secret.as_str()),
                        xmlrpc::Param::Str(model),
                        xmlrpc::Param::Str(method),
                        xmlrpc::Param::Json(&args),
                        xmlrpc::Param::Json(&kwargs),
                    ],
                    max_response,
                )
                .await;
            }
            Protocol::Json2 => {
                return json2::execute(self, env, model, method, args, kwargs, max_response).await;
            }
        }
        self.rpc_measured(
            env,
            "object",
            "execute_kw",
            (
                &env.database,
                uid,
                env.secret.as_str(),
                model,
                method,
                args,
                kwargs,
            ),
            max_response,
        )
        .await
    }
}
fn unresolved_protocol() -> OdxError {
    OdxError::Upstream("automatic protocol selection has not been checked".into())
}
/// Classify sent writes: redacted Odoo errors are known failures; preserve pre-send errors. Other network, HTTP or unreadable responses are uncertain because Odoo may have applied the write.
pub(crate) fn sent_error(error: OdxError) -> OdxError {
    match error {
        OdxError::Remote(_)
        | OdxError::Invalid(_)
        | OdxError::Validation { .. }
        | OdxError::Json(_) => error,
        // JSON-2 rejects invalid keys with HTTP 401 before execution.
        OdxError::AuthenticationRejected => error,
        // Proxy connection failure or HTTP 407 means the request never reached Odoo.
        OdxError::Network { kind: "proxy", .. } => error,
        _ => OdxError::Uncertain,
    }
}
/// Bound response bytes using both Content-Length and received data.
pub(super) async fn read_limited(
    mut response: reqwest::Response,
    env: &Environment,
    max: usize,
) -> Result<Zeroizing<Vec<u8>>> {
    let too_big = || OdxError::ResponseTooLarge {
        limit_mib: max.div_ceil(1024 * 1024),
    };
    if response
        .content_length()
        .is_some_and(|len| len > max as u64)
    {
        return Err(too_big());
    }
    let mut bytes = Zeroizing::new(Vec::new());
    while let Some(chunk) = response.chunk().await.map_err(|e| network_error(&e, env))? {
        if bytes.len() + chunk.len() > max {
            return Err(too_big());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
/// Parse JSON; bodies over 1 MiB parse on the blocking pool so tokio workers keep serving.
pub(super) async fn parse_json(bytes: Zeroizing<Vec<u8>>) -> Option<Value> {
    if bytes.len() <= 1024 * 1024 {
        return serde_json::from_slice(&bytes).ok();
    }
    tokio::task::spawn_blocking(move || serde_json::from_slice(&bytes).ok())
        .await
        .ok()
        .flatten()
}
fn relayed(clients: &ClientPool, proxy: &ProxySettings) -> bool {
    clients
        .iter()
        .any(|(p, _, relay)| p.as_ref() == Some(proxy) && relay.is_some())
}
fn build_client(proxy: Option<reqwest::Proxy>) -> Result<Client> {
    let builder = Client::builder()
        .user_agent(concat!("Odx/", env!("CARGO_PKG_VERSION")))
        .redirect(Policy::none())
        .retry(reqwest::retry::never())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(60));
    match proxy {
        None => builder.no_proxy(),
        Some(config) => builder.proxy(config),
    }
    .build()
    .map_err(|_| OdxError::Upstream("could not create HTTP client".into()))
}
/// HTTP 407 from forwarding or from the CONNECT tunnel.
fn auth_rejected(error: &OdxError) -> bool {
    matches!(error, OdxError::Network { kind: "proxy", message }
        if message.contains("HTTP 407") || message.contains("proxy authorization required"))
}
/// HTTP 407 while forwarding http:// is a proxy authentication failure, not an Odoo status.
fn proxy_rejected(response: reqwest::Response, env: &Environment) -> Result<reqwest::Response> {
    if env.proxied() && response.status() == reqwest::StatusCode::PROXY_AUTHENTICATION_REQUIRED {
        return Err(OdxError::Network {
            kind: "proxy",
            message: "proxy authentication required (HTTP 407)".into(),
        });
    }
    Ok(response)
}
/// Network kind: proxy, timeout, dns, tls, refused, unreachable, reset or network. With a proxy, connection failures are proxy errors except Odoo TLS failures inside the tunnel.
fn network_error(e: &reqwest::Error, env: &Environment) -> OdxError {
    // Include the cause chain, such as DNS or certificate errors.
    let mut detail = e.to_string();
    let mut source = std::error::Error::source(e);
    while let Some(cause) = source {
        detail.push_str(": ");
        detail.push_str(&cause.to_string());
        source = cause.source();
    }
    let lower = detail.to_lowercase();
    let in_proxy = lower.contains("tunnel error") || lower.contains("socks error");
    let tls = lower.contains("certificate") || lower.contains("tls") || lower.contains("handshake");
    let kind = if env.proxied() && e.is_connect() && (in_proxy || !tls) {
        "proxy"
    } else if e.is_timeout() || lower.contains("timed out") {
        "timeout"
    } else if lower.contains("dns")
        || lower.contains("failed to lookup")
        || lower.contains("name or service")
    {
        "dns"
    } else if tls {
        "tls"
    } else if lower.contains("refused") {
        "refused"
    } else if lower.contains("unreachable") || lower.contains("no route") {
        "unreachable"
    } else if lower.contains("reset") || lower.contains("closed") || lower.contains("broken pipe") {
        "reset"
    } else {
        "network"
    };
    OdxError::Network {
        kind,
        message: scrub(&detail, env),
    }
}
pub(super) fn scrub(s: &str, env: &Environment) -> String {
    let mut clean = s.to_owned();
    // Redact environment credentials, masked headers and proxy credentials.
    for secret in env.redaction_secrets() {
        clean = clean.replace(secret, "[redacted]");
    }
    clean.chars().take(512).collect()
}

#[cfg(test)]
mod header_tests {
    use super::*;
    #[test]
    fn errors_redact_header_credentials_and_odoo_secret() {
        let env = Environment {
            secret: crate::security::Secret::new("fixture-odoo-secret").unwrap(),
            headers: vec![
                crate::headers::HeaderEntry {
                    name: "Authorization".into(),
                    value: "Bearer fixture-header-secret".into(),
                    enabled: true,
                },
                crate::headers::HeaderEntry {
                    name: "X-Debug".into(),
                    value: "fixture-debug".into(),
                    enabled: true,
                },
                crate::headers::HeaderEntry {
                    name: "Cache-Control".into(),
                    value: "1".into(),
                    enabled: true,
                },
            ],
            ..Default::default()
        };
        let message = scrub(
            "rejected Bearer fixture-header-secret, fixture-debug and fixture-odoo-secret",
            &env,
        );
        assert_eq!(message, "rejected [redacted], [redacted] and [redacted]");
        // Visible header values remain visible in errors.
        assert_eq!(
            scrub("Record 12 does not exist", &env),
            "Record 12 does not exist"
        );
    }
}
