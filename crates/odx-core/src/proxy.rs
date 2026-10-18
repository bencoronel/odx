//! Global and per-environment outbound proxies: inherit, custom or direct. Admin-only configuration and credentials are hidden from API/MCP clients.
use crate::{
    Core,
    config::{Environment, VaultData},
    error::{OdxError, Result},
    headers::MASK,
    security::Secret,
};
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

/// Validated encrypted proxy with normalized scheme://host:port URL and no embedded credentials.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct ProxySettings {
    pub url: String,
    #[serde(default)]
    pub username: Secret,
    #[serde(default)]
    pub password: Secret,
}
impl PartialEq for ProxySettings {
    fn eq(&self, other: &Self) -> bool {
        self.url == other.url
            && self.username.as_str() == other.username.as_str()
            && self.password.as_str() == other.password.as_str()
    }
}

/// Environment proxy; connection_environment resolves Inherit before use.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub(crate) enum EnvironmentProxy {
    #[default]
    Inherit,
    Custom(ProxySettings),
    Direct,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyMode {
    Inherit,
    Custom,
    Direct,
}

/// Window input; MASK preserves saved username/password values.
#[derive(Deserialize)]
pub struct ProxyInput {
    pub url: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
}
impl Drop for ProxyInput {
    fn drop(&mut self) {
        self.username.zeroize();
        self.password.zeroize();
    }
}
impl std::fmt::Debug for ProxyInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProxyInput")
            .field("url", &self.url)
            .field("username", &"[redacted]")
            .field("password", &"[redacted]")
            .finish()
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum EnvironmentProxyInput {
    Inherit,
    Custom(ProxyInput),
    Direct,
}

/// Window view with MASK for saved credentials, or empty if absent.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProxyView {
    pub url: String,
    pub username: String,
    pub password: String,
}
#[derive(Debug, Clone, Serialize)]
pub struct EnvironmentProxyView {
    pub mode: ProxyMode,
    /// Custom proxy only.
    pub proxy: Option<ProxyView>,
    /// Inherited global proxy URL; None means direct.
    pub inherited: Option<String>,
}

fn view(p: &ProxySettings) -> ProxyView {
    let mask = |s: &Secret| if s.is_empty() { "" } else { MASK }.to_owned();
    ProxyView {
        url: p.url.clone(),
        username: mask(&p.username),
        password: mask(&p.password),
    }
}

/// scheme://host:port with http, https or socks5h and an explicit port.
fn normalize_url(raw: &str) -> Result<String> {
    let invalid = || {
        OdxError::validation(
            "validation.proxy.url_invalid",
            "invalid proxy URL: use http://, https:// or socks5h://host:port without credentials, path, query or fragment",
        )
    };
    let url = url::Url::parse(raw.trim()).map_err(|_| invalid())?;
    match url.scheme() {
        "http" | "https" | "socks5h" => {}
        "socks5" => {
            return Err(OdxError::validation(
                "validation.proxy.socks5_unsupported",
                "use socks5h:// for proxy DNS resolution; socks5 resolves names locally",
            ));
        }
        _ => return Err(invalid()),
    }
    // Url drops default ports; check the original text.
    let authority = raw
        .trim()
        .split_once("://")
        .map(|(_, rest)| rest.split(['/', '?', '#']).next().unwrap_or(""))
        .unwrap_or("");
    let explicit_port = authority.rsplit_once(':').is_some_and(|(host, port)| {
        !host.is_empty() && !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit())
    });
    let host = url
        .host_str()
        .filter(|h| !h.is_empty())
        .ok_or_else(invalid)?;
    let port = url.port_or_known_default().filter(|_| explicit_port);
    if port.is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || !matches!(url.path(), "" | "/")
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid());
    }
    Ok(format!(
        "{}://{host}:{}",
        url.scheme(),
        port.unwrap_or_default()
    ))
}

fn secret_or_stored(
    value: &str,
    stored: Option<&Secret>,
    code: &'static str,
    what: &str,
) -> Result<Secret> {
    if value == MASK {
        return stored
            .filter(|s| !s.is_empty())
            .cloned()
            .ok_or_else(|| OdxError::validation(code, format!("reenter the proxy {what}")));
    }
    Secret::new(value)
}

impl ProxyInput {
    fn into_settings(self, stored: Option<&ProxySettings>) -> Result<ProxySettings> {
        let url = normalize_url(&self.url)?;
        let username = secret_or_stored(
            &self.username,
            stored.map(|s| &s.username),
            "validation.proxy.username_reentry",
            "username",
        )?;
        let password = secret_or_stored(
            &self.password,
            stored.map(|s| &s.password),
            "validation.proxy.password_reentry",
            "password",
        )?;
        if username.is_empty() && !password.is_empty() {
            return Err(OdxError::validation(
                "validation.proxy.password_requires_username",
                "proxy password requires a username",
            ));
        }
        // Basic forbids colons in usernames (RFC 7617); SOCKS5 limits each field to 255 bytes (RFC 1929).
        if username.as_str().contains(':')
            || [&username, &password]
                .iter()
                .any(|s| s.as_str().len() > 255 || s.as_str().chars().any(char::is_control))
        {
            return Err(OdxError::validation(
                "validation.proxy.credentials_invalid",
                "invalid proxy credentials: usernames cannot contain colons; no control characters; maximum 255 bytes",
            ));
        }
        Ok(ProxySettings {
            url,
            username,
            password,
        })
    }
}

impl VaultData {
    /// Resolve custom, direct or inherited global proxy.
    pub(crate) fn effective_proxy(&self, env: &Environment) -> EnvironmentProxy {
        match &env.proxy {
            EnvironmentProxy::Inherit => self
                .proxy
                .clone()
                .map_or(EnvironmentProxy::Direct, EnvironmentProxy::Custom),
            other => other.clone(),
        }
    }
    /// Effective proxies in use; drop HTTP clients for unused proxies.
    fn active_proxies(&self) -> Vec<Option<ProxySettings>> {
        self.environments
            .iter()
            .map(|e| match self.effective_proxy(e) {
                EnvironmentProxy::Custom(p) => Some(p),
                _ => None,
            })
            .collect()
    }
}

impl Environment {
    /// Redact environment credentials, masked headers and effective proxy credentials from errors and activity.
    pub(crate) fn redaction_secrets(&self) -> Vec<&str> {
        let proxy = match &self.proxy {
            // Short usernames could redact unrelated error/activity text.
            EnvironmentProxy::Custom(p) => [p.username.as_str()]
                .into_iter()
                .filter(|u| u.chars().count() >= MIN_REDACTED_USERNAME)
                .chain([p.password.as_str()])
                .collect(),
            _ => vec![],
        };
        let mut secrets: Vec<&str> = self
            .headers
            .iter()
            .filter(|h| crate::headers::is_sensitive(&h.name))
            .map(|h| h.value.as_str())
            .chain(std::iter::once(self.secret.as_str()))
            .chain(proxy)
            .filter(|v| !v.is_empty())
            .collect();
        secrets.sort_by_key(|v| std::cmp::Reverse(v.len()));
        secrets
    }
    pub(crate) fn proxied(&self) -> bool {
        matches!(self.proxy, EnvironmentProxy::Custom(_))
    }
}

/// Minimum proxy username length for literal redaction.
const MIN_REDACTED_USERNAME: usize = 4;

impl Core {
    pub async fn proxy(&self) -> Result<Option<ProxyView>> {
        let s = self.inner.lock().await;
        let data = s.vault.as_ref().ok_or(OdxError::Locked)?.data();
        Ok(data.proxy.as_ref().map(view))
    }
    pub async fn environment_proxy(&self, id: &str) -> Result<EnvironmentProxyView> {
        let s = self.inner.lock().await;
        let data = s.vault.as_ref().ok_or(OdxError::Locked)?.data();
        let env = data
            .environments
            .iter()
            .find(|e| e.id == id)
            .ok_or(OdxError::EnvironmentNotFound)?;
        let (mode, proxy) = match &env.proxy {
            EnvironmentProxy::Inherit => (ProxyMode::Inherit, None),
            EnvironmentProxy::Custom(p) => (ProxyMode::Custom, Some(view(p))),
            EnvironmentProxy::Direct => (ProxyMode::Direct, None),
        };
        Ok(EnvironmentProxyView {
            mode,
            proxy,
            inherited: data.proxy.as_ref().map(|p| p.url.clone()),
        })
    }
    /// Update the global proxy; increment inheriting environment revisions, invalidate authentication and revoke pending approvals. Unchanged settings do nothing.
    pub async fn set_proxy(&self, proxy: Option<ProxyInput>) -> Result<()> {
        let mut s = self.inner.lock().await;
        let vault = s.vault.as_mut().ok_or(OdxError::Locked)?;
        let next = proxy
            .map(|p| p.into_settings(vault.data().proxy.as_ref()))
            .transpose()?;
        if next == vault.data().proxy {
            return Ok(());
        }
        let ids = vault.update(|data| {
            data.proxy = next;
            Ok(data
                .environments
                .iter_mut()
                .filter(|e| e.proxy == EnvironmentProxy::Inherit)
                .map(|e| {
                    e.revision += 1;
                    e.detected_protocol = None;
                    e.id.clone()
                })
                .collect::<Vec<_>>())
        })?;
        let active = vault.data().active_proxies();
        self.odoo.retain_clients(&active);
        self.environment_changed(s, &ids).await;
        Ok(())
    }
    /// Update an environment proxy and increment its connection revision.
    pub async fn set_environment_proxy(
        &self,
        id: &str,
        proxy: EnvironmentProxyInput,
    ) -> Result<()> {
        let mut s = self.inner.lock().await;
        let vault = s.vault.as_mut().ok_or(OdxError::Locked)?;
        let current = vault
            .data()
            .environments
            .iter()
            .find(|e| e.id == id)
            .ok_or(OdxError::EnvironmentNotFound)?
            .proxy
            .clone();
        let next = match proxy {
            EnvironmentProxyInput::Inherit => EnvironmentProxy::Inherit,
            EnvironmentProxyInput::Direct => EnvironmentProxy::Direct,
            EnvironmentProxyInput::Custom(input) => {
                let stored = match &current {
                    EnvironmentProxy::Custom(p) => Some(p),
                    _ => None,
                };
                EnvironmentProxy::Custom(input.into_settings(stored)?)
            }
        };
        if next == current {
            return Ok(());
        }
        vault.update(|data| {
            let env = data
                .environments
                .iter_mut()
                .find(|e| e.id == id)
                .ok_or(OdxError::EnvironmentNotFound)?;
            env.proxy = next;
            env.revision += 1;
            env.detected_protocol = None;
            Ok(())
        })?;
        let active = vault.data().active_proxies();
        self.odoo.retain_clients(&active);
        self.environment_changed(s, &[id.to_owned()]).await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn short_proxy_usernames_are_not_redacted_as_text() {
        let env = |user: &str| crate::config::Environment {
            proxy: EnvironmentProxy::Custom(ProxySettings {
                url: "http://proxy.test:3128".into(),
                username: Secret::new(user).unwrap(),
                password: Secret::new("fixture-proxy-pass").unwrap(),
            }),
            ..Default::default()
        };
        let short = env("ana");
        let secrets = short.redaction_secrets();
        assert!(secrets.contains(&"fixture-proxy-pass"));
        assert!(!secrets.contains(&"ana"));
        let long = env("proxyuser");
        assert!(long.redaction_secrets().contains(&"proxyuser"));
    }
    #[test]
    fn urls_are_normalized_and_restricted() {
        assert_eq!(
            normalize_url("HTTP://Proxy.Test:80/").unwrap(),
            "http://proxy.test:80"
        );
        assert_eq!(
            normalize_url("socks5h://[::1]:1080").unwrap(),
            "socks5h://[::1]:1080"
        );
        for bad in [
            "socks5://h:1",
            "http://h",
            "http://u:p@h:1",
            "http://h:1/x",
            "http://h:1?q",
            "http://h:1#f",
            "h:1",
        ] {
            assert!(normalize_url(bad).is_err(), "{bad}");
        }
    }
    #[test]
    fn debug_and_views_never_show_credentials() {
        let input = ProxyInput {
            url: "http://h:1".into(),
            username: "fixture-user".into(),
            password: "fixture-pass".into(),
        };
        assert!(!format!("{input:?}").contains("fixture-"));
        let settings = input.into_settings(None).unwrap();
        assert!(!format!("{settings:?}").contains("fixture-"));
        let shown = serde_json::to_string(&view(&settings)).unwrap();
        assert!(!shown.contains("fixture-"));
    }
    #[test]
    fn old_vaults_load_without_proxy() {
        let data = VaultData {
            environments: vec![Environment::default()],
            ..Default::default()
        };
        let mut old = serde_json::to_value(data).unwrap();
        old.as_object_mut().unwrap().remove("proxy");
        old["environments"][0]
            .as_object_mut()
            .unwrap()
            .remove("proxy");
        let restored: VaultData = serde_json::from_value(old).unwrap();
        assert!(restored.proxy.is_none());
        assert_eq!(restored.environments[0].proxy, EnvironmentProxy::Inherit);
    }
}
