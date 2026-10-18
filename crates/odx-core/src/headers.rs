use crate::{
    Core,
    config::{Environment, VaultData},
    error::{OdxError, Result},
};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use zeroize::Zeroize;

/// Admin-only values may contain upstream proxy credentials.
#[derive(Clone, Serialize, Deserialize)]
pub struct HeaderEntry {
    pub name: String,
    pub value: String,
    pub enabled: bool,
}
impl Drop for HeaderEntry {
    fn drop(&mut self) {
        self.value.zeroize();
    }
}

impl std::fmt::Debug for HeaderEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HeaderEntry")
            .field("name", &self.name)
            .field("value", &"[redacted]")
            .field("enabled", &self.enabled)
            .finish()
    }
}
#[derive(Serialize)]
pub struct HeaderSettings {
    pub headers: Vec<HeaderEntry>,
    pub inherited: Vec<HeaderEntry>,
}

/// Non-ASCII placeholder for sensitive values; returning it preserves the stored value. Invalid as an HTTP value.
pub const MASK: &str = "••••••••••••";
/// Show allowlisted negotiation, cache, client and trace headers. Treat all others, including custom headers, as sensitive.
const VISIBLE: [&str; 9] = [
    "accept",
    "accept-encoding",
    "accept-language",
    "cache-control",
    "user-agent",
    "x-forwarded-for",
    "x-forwarded-host",
    "x-forwarded-proto",
    "x-request-id",
];
/// Expose values only for names in `VISIBLE`.
pub fn is_sensitive(name: &str) -> bool {
    !VISIBLE
        .iter()
        .any(|visible| name.eq_ignore_ascii_case(visible))
}
fn masked(headers: &[HeaderEntry]) -> Vec<HeaderEntry> {
    headers
        .iter()
        .map(|h| HeaderEntry {
            name: h.name.clone(),
            value: if is_sensitive(&h.name) && !h.value.is_empty() {
                MASK.into()
            } else {
                h.value.clone()
            },
            enabled: h.enabled,
        })
        .collect()
}
/// Restore saved values for headers returned with placeholders.
fn unmask(headers: &mut [HeaderEntry], stored: &[HeaderEntry]) -> Result<()> {
    for h in headers.iter_mut().filter(|h| h.value == MASK) {
        let previous = stored
            .iter()
            .find(|s| s.name.eq_ignore_ascii_case(&h.name) && is_sensitive(&s.name))
            .ok_or_else(|| {
                OdxError::validation(
                    "validation.headers.secret_reentry_required",
                    "reenter the header value",
                )
            })?;
        h.value = previous.value.clone();
    }
    Ok(())
}

pub fn validate(headers: &[HeaderEntry]) -> Result<()> {
    if headers.len() > 64
        || headers
            .iter()
            .map(|h| h.name.len() + h.value.len())
            .sum::<usize>()
            > 32_768
    {
        return Err(OdxError::validation(
            "validation.headers.limit_exceeded",
            "maximum 64 headers and 32 KiB per configuration",
        ));
    }
    validate_entries(headers)
}
fn validate_entries(headers: &[HeaderEntry]) -> Result<()> {
    let mut names = HashSet::new();
    for h in headers {
        let name = HeaderName::from_bytes(h.name.as_bytes()).map_err(|_| {
            OdxError::validation(
                "validation.headers.name_invalid",
                "invalid HTTP header name",
            )
        })?;
        HeaderValue::from_str(&h.value).map_err(|_| {
            OdxError::validation(
                "validation.headers.value_invalid",
                "invalid HTTP header value",
            )
        })?;
        if matches!(
            name.as_str(),
            "host"
                | "content-length"
                | "transfer-encoding"
                | "connection"
                | "trailer"
                | "upgrade"
                | "proxy-authorization"
                | "proxy-connection"
                | "te"
        ) {
            return Err(OdxError::validation(
                "validation.headers.reserved_name",
                "reserved transport header managed by the HTTP client",
            ));
        }
        if !names.insert(name) {
            return Err(OdxError::validation(
                "validation.headers.duplicate_name",
                "duplicate case-insensitive header name",
            ));
        }
    }
    Ok(())
}
pub(crate) fn merge(global: &[HeaderEntry], local: &[HeaderEntry]) -> Vec<HeaderEntry> {
    let mut headers: Vec<_> = global.iter().filter(|h| h.enabled).cloned().collect();
    for h in local.iter().filter(|h| h.enabled) {
        headers.retain(|g| !g.name.eq_ignore_ascii_case(&h.name));
        headers.push(h.clone());
    }
    headers
}
pub(crate) fn request_headers(headers: &[HeaderEntry]) -> Result<HeaderMap> {
    validate_entries(headers)?;
    let mut result = HeaderMap::new();
    for h in headers.iter().filter(|h| h.enabled) {
        let mut value = HeaderValue::from_str(&h.value).map_err(|_| {
            OdxError::validation(
                "validation.headers.value_invalid",
                "invalid HTTP header value",
            )
        })?;
        value.set_sensitive(true);
        result.insert(
            HeaderName::from_bytes(h.name.as_bytes()).map_err(|_| {
                OdxError::validation(
                    "validation.headers.name_invalid",
                    "invalid HTTP header name",
                )
            })?,
            value,
        );
    }
    Ok(result)
}
impl VaultData {
    pub(crate) fn connection_environment(&self, id: &str) -> Result<Environment> {
        let mut env = self
            .environments
            .iter()
            .find(|e| e.id == id)
            .cloned()
            .ok_or(OdxError::EnvironmentNotFound)?;
        env.headers = merge(&self.global_headers, &env.headers);
        env.proxy = self.effective_proxy(&env);
        if env.protocol == crate::config::Protocol::Auto
            && let Some(protocol) = env
                .detected_protocol
                .filter(|p| *p != crate::config::Protocol::Auto)
        {
            env.protocol = protocol;
        }
        Ok(env)
    }
}
impl Core {
    pub async fn header_settings(&self, environment_id: Option<&str>) -> Result<HeaderSettings> {
        let s = self.inner.lock().await;
        let data = s.vault.as_ref().ok_or(OdxError::Locked)?.data();
        Ok(match environment_id {
            Some(id) => HeaderSettings {
                headers: masked(
                    &data
                        .environments
                        .iter()
                        .find(|e| e.id == id)
                        .ok_or(OdxError::EnvironmentNotFound)?
                        .headers,
                ),
                inherited: masked(&merge(&data.global_headers, &[])),
            },
            None => HeaderSettings {
                headers: masked(&data.global_headers),
                inherited: vec![],
            },
        })
    }
    pub async fn set_headers(
        &self,
        environment_id: Option<&str>,
        mut headers: Vec<HeaderEntry>,
    ) -> Result<()> {
        let mut s = self.inner.lock().await;
        let vault = s.vault.as_mut().ok_or(OdxError::Locked)?;
        let stored = match environment_id {
            Some(id) => {
                &vault
                    .data()
                    .environments
                    .iter()
                    .find(|e| e.id == id)
                    .ok_or(OdxError::EnvironmentNotFound)?
                    .headers
            }
            None => &vault.data().global_headers,
        };
        unmask(&mut headers, stored)?;
        validate(&headers)?;
        let ids = vault.update(|data| {
            Ok(if let Some(id) = environment_id {
                let env = data
                    .environments
                    .iter_mut()
                    .find(|e| e.id == id)
                    .ok_or(OdxError::EnvironmentNotFound)?;
                env.headers = headers;
                env.revision += 1;
                env.detected_protocol = None;
                vec![id.to_owned()]
            } else {
                data.global_headers = headers;
                data.environments
                    .iter_mut()
                    .map(|e| {
                        e.revision += 1;
                        e.detected_protocol = None;
                        e.id.clone()
                    })
                    .collect()
            })
        })?;
        self.environment_changed(s, &ids).await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn header(name: &str, value: &str) -> HeaderEntry {
        HeaderEntry {
            name: name.into(),
            value: value.into(),
            enabled: true,
        }
    }
    #[test]
    fn validation_rejects_injection_duplicates_reserved_names_and_oversize() {
        for headers in [
            vec![header("X-Test", "a\r\nInjected: yes")],
            vec![header("invalid name", "value")],
            vec![header("X-A", "one"), header("x-a", "two")],
            vec![header("HOST", "elsewhere")],
            vec![header("Content-Length", "1")],
            vec![header("Transfer-Encoding", "chunked")],
            vec![header("X-Big", &"x".repeat(32_769))],
            vec![header("X-Many", "v"); 65],
        ] {
            assert!(validate(&headers).is_err());
        }
        assert!(
            validate(&[
                header("Authorization", "Bearer fixture"),
                header("X-Empty", "")
            ])
            .is_ok()
        );
    }
    #[test]
    fn only_the_closed_list_is_visible() {
        for name in ["Accept", "ACCEPT-LANGUAGE", "user-agent", "X-Request-Id"] {
            assert!(!is_sensitive(name), "{name}");
        }
        for name in [
            "Authorization",
            "X-Api-Key",
            "X-Custom",
            "X-Tenant",
            "Cookie",
            "Accept-Language-Token",
            "X-Accept",
            "",
        ] {
            assert!(is_sensitive(name), "{name}");
        }
    }
    #[test]
    fn merge_is_case_insensitive_and_disabled_rows_inherit() {
        let global = vec![header("X-Test", "global"), header("Accept", "*/*")];
        let local = vec![
            header("x-test", "local"),
            HeaderEntry {
                name: "Accept".into(),
                value: "disabled".into(),
                enabled: false,
            },
        ];
        let effective = request_headers(&merge(&global, &local)).unwrap();
        assert_eq!(effective["x-test"], "local");
        assert_eq!(effective["accept"], "*/*");
        assert!(effective["x-test"].is_sensitive());
        assert!(!format!("{:?}", header("X-Key", "fixture-secret")).contains("fixture-secret"));
        assert_eq!(
            request_headers(&merge(&global, &[header("X-Test", "")])).unwrap()["x-test"],
            ""
        );
    }
    #[test]
    fn old_vaults_load_with_empty_header_settings() {
        let data = VaultData {
            environments: vec![Environment::default()],
            ..Default::default()
        };
        let mut old = serde_json::to_value(data).unwrap();
        old.as_object_mut().unwrap().remove("global_headers");
        old["environments"][0]
            .as_object_mut()
            .unwrap()
            .remove("headers");
        let restored: VaultData = serde_json::from_value(old).unwrap();
        assert!(restored.global_headers.is_empty());
        assert!(restored.environments[0].headers.is_empty());
    }
}
