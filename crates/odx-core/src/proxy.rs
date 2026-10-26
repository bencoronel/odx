//! Named outbound proxies; one may be the default. Environments use the default, a named proxy or a direct connection. Admin-only configuration and credentials are hidden from API/MCP clients.
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

/// A named proxy saved in settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct ProxyProfile {
    pub id: String,
    pub name: String,
    #[serde(flatten)]
    pub settings: ProxySettings,
}

/// Stored: Inherit (the default proxy), Profile or Direct. connection_environment resolves it to Custom or Direct before use; stored Custom only exists in old vaults and is migrated on load.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub(crate) enum EnvironmentProxy {
    #[default]
    Inherit,
    Profile {
        id: String,
    },
    Custom(ProxySettings),
    Direct,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyMode {
    Inherit,
    Profile,
    Direct,
}

/// Window input; MASK preserves saved username/password values.
#[derive(Deserialize)]
pub struct ProxyInput {
    #[serde(default)]
    pub name: String,
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
            .field("name", &self.name)
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
    Profile { id: String },
    Direct,
}

/// Window view with MASK for saved credentials, or empty if absent.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProxyView {
    pub id: String,
    pub name: String,
    pub url: String,
    pub username: String,
    pub password: String,
    /// Environments that select this proxy by name; they block deletion.
    pub environments: usize,
}
#[derive(Debug, Clone, Serialize)]
pub struct ProxiesView {
    pub proxies: Vec<ProxyView>,
    /// Id of the default proxy; None means environments that use the default connect directly.
    pub default: Option<String>,
}
#[derive(Debug, Clone, Serialize)]
pub struct EnvironmentProxyView {
    pub mode: ProxyMode,
    /// Profile mode only.
    pub id: Option<String>,
}

fn view(p: &ProxyProfile, environments: usize) -> ProxyView {
    let mask = |s: &Secret| if s.is_empty() { "" } else { MASK }.to_owned();
    ProxyView {
        id: p.id.clone(),
        name: p.name.clone(),
        url: p.settings.url.clone(),
        username: mask(&p.settings.username),
        password: mask(&p.settings.password),
        environments,
    }
}

const MAX_NAME: usize = 100;
fn normalize_name(raw: &str) -> Result<String> {
    let name = raw.trim();
    if name.is_empty() || name.chars().count() > MAX_NAME || name.chars().any(char::is_control) {
        return Err(OdxError::validation(
            "validation.proxy.name_invalid",
            "proxy name is required, up to 100 characters",
        ));
    }
    Ok(name.to_owned())
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
    fn to_settings(&self, stored: Option<&ProxySettings>) -> Result<ProxySettings> {
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
    fn selected_by(&self, id: &str) -> usize {
        self.environments
            .iter()
            .filter(|e| matches!(&e.proxy, EnvironmentProxy::Profile { id: p } if p == id))
            .count()
    }
    /// The stored form of a window choice; unknown proxies are rejected.
    pub(crate) fn chosen_proxy(&self, proxy: EnvironmentProxyInput) -> Result<EnvironmentProxy> {
        Ok(match proxy {
            EnvironmentProxyInput::Inherit => EnvironmentProxy::Inherit,
            EnvironmentProxyInput::Direct => EnvironmentProxy::Direct,
            EnvironmentProxyInput::Profile { id } => {
                self.profile(&id).ok_or_else(not_found)?;
                EnvironmentProxy::Profile { id }
            }
        })
    }
    fn profile(&self, id: &str) -> Option<&ProxyProfile> {
        self.proxies.iter().find(|p| p.id == id)
    }
    /// Resolve the default or named proxy to its settings; a missing proxy never falls back to direct.
    pub(crate) fn effective_proxy(&self, env: &Environment) -> EnvironmentProxy {
        let id = match &env.proxy {
            EnvironmentProxy::Inherit => match &self.default_proxy {
                Some(id) => id,
                None => return EnvironmentProxy::Direct,
            },
            EnvironmentProxy::Profile { id } => id,
            other => return other.clone(),
        };
        self.profile(id).map_or(EnvironmentProxy::Inherit, |p| {
            EnvironmentProxy::Custom(p.settings.clone())
        })
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
    /// Turn the old global proxy and per-environment custom proxies into named proxies.
    pub(crate) fn migrate_legacy_proxies(&mut self, global: Option<ProxySettings>) {
        let add = |proxies: &mut Vec<ProxyProfile>, settings: ProxySettings, name: String| {
            if let Some(p) = proxies.iter().find(|p| p.settings == settings) {
                return p.id.clone();
            }
            let id = uuid::Uuid::new_v4().to_string();
            proxies.push(ProxyProfile {
                id: id.clone(),
                name,
                settings,
            });
            id
        };
        if let Some(settings) = global {
            self.default_proxy = Some(add(&mut self.proxies, settings, "Proxy".into()));
        }
        for env in &mut self.environments {
            if let EnvironmentProxy::Custom(settings) = &env.proxy {
                let id = add(
                    &mut self.proxies,
                    settings.clone(),
                    format!("Proxy {}", env.name),
                );
                env.proxy = EnvironmentProxy::Profile { id };
            }
        }
    }
}

impl Environment {
    /// Environment credentials, masked headers and effective proxy credentials to redact from errors and activity.
    /// Values under MIN_REDACTED characters are left alone: they would redact unrelated text.
    pub(crate) fn redaction_secrets(&self) -> Vec<&str> {
        let proxy = match &self.proxy {
            EnvironmentProxy::Custom(p) => vec![p.username.as_str(), p.password.as_str()],
            _ => vec![],
        };
        let mut secrets: Vec<&str> = self
            .headers
            .iter()
            .filter(|h| crate::headers::is_sensitive(&h.name))
            .map(|h| h.value.as_str())
            .chain(std::iter::once(self.secret.as_str()))
            .chain(proxy)
            .filter(|v| v.chars().count() >= MIN_REDACTED)
            .collect();
        secrets.sort_by_key(|v| std::cmp::Reverse(v.len()));
        secrets
    }
    /// Replace every secret with [redacted]. Short ones only as whole words, so a password
    /// "admin" leaves base.user_admin intact.
    pub(crate) fn redact(&self, text: &str) -> String {
        let mut clean = text.to_owned();
        for secret in self.redaction_secrets() {
            clean = if secret.chars().count() < WHOLE_WORD_BELOW {
                replace_word(&clean, secret)
            } else {
                clean.replace(secret, "[redacted]")
            };
        }
        clean
    }
    pub(crate) fn proxied(&self) -> bool {
        matches!(self.proxy, EnvironmentProxy::Custom(_))
    }
}

/// Minimum proxy username length for literal redaction.
const MIN_REDACTED: usize = 4;
const WHOLE_WORD_BELOW: usize = 8;

fn replace_word(text: &str, word: &str) -> String {
    let word_char = |c: char| c.is_alphanumeric() || c == '_';
    let (mut out, mut last) = (String::with_capacity(text.len()), 0);
    for (start, _) in text.match_indices(word) {
        let end = start + word.len();
        if text[..start].chars().next_back().is_some_and(word_char)
            || text[end..].chars().next().is_some_and(word_char)
        {
            continue;
        }
        out.push_str(&text[last..start]);
        out.push_str("[redacted]");
        last = end;
    }
    out.push_str(&text[last..]);
    out
}

impl Core {
    pub async fn proxies(&self) -> Result<ProxiesView> {
        let s = self.inner.lock().await;
        let data = s.vault()?.data();
        Ok(ProxiesView {
            proxies: data
                .proxies
                .iter()
                .map(|p| view(p, data.selected_by(&p.id)))
                .collect(),
            default: data.default_proxy.clone(),
        })
    }
    pub async fn environment_proxy(&self, id: &str) -> Result<EnvironmentProxyView> {
        let s = self.inner.lock().await;
        let data = s.vault()?.data();
        let env = data.environment(id)?;
        Ok(match &env.proxy {
            EnvironmentProxy::Profile { id } => EnvironmentProxyView {
                mode: ProxyMode::Profile,
                id: Some(id.clone()),
            },
            EnvironmentProxy::Direct => EnvironmentProxyView {
                mode: ProxyMode::Direct,
                id: None,
            },
            _ => EnvironmentProxyView {
                mode: ProxyMode::Inherit,
                id: None,
            },
        })
    }
    /// Create (`id` None) or update a named proxy; returns its id.
    pub async fn save_proxy(&self, id: Option<String>, input: ProxyInput) -> Result<String> {
        let mut out = String::new();
        self.change_proxies(|data| {
            let name = normalize_name(&input.name)?;
            if data
                .proxies
                .iter()
                .any(|p| Some(&p.id) != id.as_ref() && p.name.to_lowercase() == name.to_lowercase())
            {
                return Err(OdxError::validation(
                    "validation.proxy.name_taken",
                    "a proxy with this name already exists",
                ));
            }
            match &id {
                Some(id) => {
                    let profile = data
                        .proxies
                        .iter_mut()
                        .find(|p| &p.id == id)
                        .ok_or_else(not_found)?;
                    profile.settings = input.to_settings(Some(&profile.settings))?;
                    profile.name = name;
                    out = id.clone();
                }
                None => {
                    let settings = input.to_settings(None)?;
                    out = uuid::Uuid::new_v4().to_string();
                    data.proxies.push(ProxyProfile {
                        id: out.clone(),
                        name,
                        settings,
                    });
                }
            }
            Ok(())
        })
        .await?;
        Ok(out)
    }
    /// Delete a named proxy that no environment selects; deleting the default leaves the default unset (direct).
    pub async fn delete_proxy(&self, id: &str) -> Result<()> {
        self.change_proxies(|data| {
            let users = data.selected_by(id);
            if users > 0 {
                return Err(OdxError::validation_with_params(
                    "validation.proxy.in_use",
                    serde_json::json!({ "count": users }),
                    format!("{users} environments use this proxy"),
                ));
            }
            let before = data.proxies.len();
            data.proxies.retain(|p| p.id != id);
            if data.proxies.len() == before {
                return Err(not_found());
            }
            if data.default_proxy.as_deref() == Some(id) {
                data.default_proxy = None;
            }
            Ok(())
        })
        .await
    }
    /// Choose the proxy used by environments that use the default; None connects them directly.
    pub async fn set_default_proxy(&self, id: Option<String>) -> Result<()> {
        self.change_proxies(|data| {
            if let Some(id) = &id
                && data.profile(id).is_none()
            {
                return Err(not_found());
            }
            data.default_proxy = id;
            Ok(())
        })
        .await
    }
    /// Choose an environment's proxy.
    pub async fn set_environment_proxy(
        &self,
        id: &str,
        proxy: EnvironmentProxyInput,
    ) -> Result<()> {
        self.change_proxies(|data| {
            let next = data.chosen_proxy(proxy)?;
            data.environment_mut(id)?.proxy = next;
            Ok(())
        })
        .await
    }
    /// Apply a proxy change. Environments whose effective proxy changes get a new connection revision, lose cached authentication and have pending approvals revoked. No effective change saves nothing.
    async fn change_proxies(
        &self,
        change: impl FnOnce(&mut VaultData) -> Result<()>,
    ) -> Result<()> {
        let mut s = self.inner.lock().await;
        let vault = s.vault_mut()?;
        let old = vault.data();
        let mut next = old.clone();
        change(&mut next)?;
        let same_proxy = |a: &Environment, b: &Environment| a.proxy == b.proxy;
        if next.proxies == old.proxies
            && next.default_proxy == old.default_proxy
            && next
                .environments
                .iter()
                .zip(&old.environments)
                .all(|(a, b)| same_proxy(a, b))
        {
            return Ok(());
        }
        let ids: Vec<String> = next
            .environments
            .iter()
            .filter(|e| {
                old.environments
                    .iter()
                    .find(|o| o.id == e.id)
                    .is_none_or(|o| old.effective_proxy(o) != next.effective_proxy(e))
            })
            .map(|e| e.id.clone())
            .collect();
        vault.update(|data| {
            *data = next;
            for env in data.environments.iter_mut().filter(|e| ids.contains(&e.id)) {
                env.revision += 1;
                env.detected_protocol = None;
            }
            Ok(())
        })?;
        let active = vault.data().active_proxies();
        self.odoo.retain_clients(&active);
        self.environment_changed(s, &ids).await;
        Ok(())
    }
}

/// A proxy test that reached the Odoo server.
#[derive(Debug, Clone, Serialize)]
pub struct ProxyTestReport {
    pub latency_ms: u64,
    /// None when the server answered but did not report its version.
    pub server_version: Option<String>,
}

impl Core {
    /// Reach `url` through an unsaved proxy (`id` resolves masked credentials). Any answer from the server proves the proxy works; only network and proxy failures are errors.
    pub async fn test_proxy(
        &self,
        id: Option<String>,
        input: ProxyInput,
        url: &str,
    ) -> Result<ProxyTestReport> {
        crate::config::validate_url(url)?;
        let (settings, headers, active) = {
            let s = self.inner.lock().await;
            let data = s.vault()?.data();
            let stored = id
                .as_deref()
                .and_then(|id| data.profile(id))
                .map(|p| &p.settings);
            (
                input.to_settings(stored)?,
                crate::headers::merge(&data.global_headers, &[]),
                data.active_proxies(),
            )
        };
        let env = Environment {
            id: uuid::Uuid::new_v4().to_string(),
            url: url.trim_end_matches('/').into(),
            protocol: crate::config::Protocol::LegacyJsonrpc,
            headers,
            proxy: EnvironmentProxy::Custom(settings),
            ..Default::default()
        };
        let permits = self
            .limits
            .acquire(crate::environments::DRAFT_SLOTS)
            .await?;
        let started = std::time::Instant::now();
        let result = self.odoo.version(&env).await;
        let latency_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
        drop(permits);
        // Drop the client built for this unsaved proxy.
        self.odoo.retain_clients(&active);
        self.odoo.invalidate_environment(&env.id).await;
        match result {
            Ok(server_version) => Ok(ProxyTestReport {
                latency_ms,
                server_version,
            }),
            Err(error @ OdxError::Network { .. }) => Err(error),
            Err(_) => Ok(ProxyTestReport {
                latency_ms,
                server_version: None,
            }),
        }
    }
}

fn not_found() -> OdxError {
    OdxError::validation("validation.proxy.not_found", "proxy not found")
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
    fn short_secrets_redact_whole_words_only_and_tiny_ones_nothing() {
        let env = crate::config::Environment {
            secret: Secret::new("admin").unwrap(),
            headers: vec![crate::headers::HeaderEntry {
                name: "Authorization".into(),
                value: "1".into(),
                enabled: true,
            }],
            ..Default::default()
        };
        assert_eq!(
            env.redact(r#"{"login": "admin", "xmlid": "base.user_admin", "ids": [1]}"#),
            r#"{"login": "[redacted]", "xmlid": "base.user_admin", "ids": [1]}"#
        );
        let long = crate::config::Environment {
            secret: Secret::new("fixture-long-secret").unwrap(),
            ..Default::default()
        };
        assert_eq!(long.redact("xfixture-long-secretx"), "x[redacted]x");
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
            name: "Office".into(),
            url: "http://h:1".into(),
            username: "fixture-user".into(),
            password: "fixture-pass".into(),
        };
        assert!(!format!("{input:?}").contains("fixture-"));
        let settings = input.to_settings(None).unwrap();
        assert!(!format!("{settings:?}").contains("fixture-"));
        let profile = ProxyProfile {
            id: "p".into(),
            name: "Office".into(),
            settings,
        };
        let shown = serde_json::to_string(&view(&profile, 0)).unwrap();
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
        assert!(restored.proxies.is_empty() && restored.default_proxy.is_none());
        assert_eq!(restored.environments[0].proxy, EnvironmentProxy::Inherit);
    }
    #[test]
    fn old_global_and_custom_proxies_become_named_proxies() {
        let settings = |url: &str| ProxySettings {
            url: url.into(),
            username: Secret::new("fixture-user").unwrap(),
            password: Secret::new("fixture-pass").unwrap(),
        };
        let env = |id: &str, proxy| Environment {
            id: id.into(),
            name: id.into(),
            proxy,
            ..Default::default()
        };
        let data = VaultData {
            environments: vec![
                env("a", EnvironmentProxy::Custom(settings("http://a:1"))),
                env("b", EnvironmentProxy::Custom(settings("http://g:1"))),
                env("c", EnvironmentProxy::Direct),
                env("d", EnvironmentProxy::Inherit),
            ],
            ..Default::default()
        };
        let mut old = serde_json::to_value(data).unwrap();
        old["proxy"] = serde_json::to_value(settings("http://g:1")).unwrap();
        let restored: VaultData = serde_json::from_value(old).unwrap();
        let names: Vec<_> = restored.proxies.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Proxy", "Proxy a"]);
        let global = &restored.proxies[0];
        assert_eq!(restored.default_proxy.as_ref(), Some(&global.id));
        let modes: Vec<_> = restored
            .environments
            .iter()
            .map(|e| e.proxy.clone())
            .collect();
        assert_eq!(
            modes,
            [
                EnvironmentProxy::Profile {
                    id: restored.proxies[1].id.clone()
                },
                EnvironmentProxy::Profile {
                    id: global.id.clone()
                },
                EnvironmentProxy::Direct,
                EnvironmentProxy::Inherit,
            ]
        );
        // Every environment resolves to the same settings it used before.
        assert_eq!(
            restored.effective_proxy(&restored.environments[1]),
            EnvironmentProxy::Custom(settings("http://g:1"))
        );
        assert_eq!(
            restored.effective_proxy(&restored.environments[3]),
            EnvironmentProxy::Custom(settings("http://g:1"))
        );
        assert!(
            serde_json::to_value(&restored)
                .unwrap()
                .get("proxy")
                .is_none()
        );
    }
}
