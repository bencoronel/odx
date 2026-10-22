use crate::error::{OdxError, Result};
use serde::{
    Deserialize, Serialize,
    de::{self, MapAccess, SeqAccess, Visitor},
};
use serde_json::Value;
use url::Url;
use uuid::Uuid;
use zeroize::Zeroize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    Auto,
    LegacyJsonrpc,
    Xmlrpc,
    Json2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WriteMode {
    ReadOnly,
    Approval,
    Temporary,
    Permanent,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Environment {
    #[serde(skip)]
    pub(crate) disabled_context: serde_json::Map<String, Value>,
    #[serde(skip)]
    pub(crate) headers: Vec<crate::headers::HeaderEntry>,
    pub id: String,
    pub name: String,
    pub url: String,
    pub database: String,
    pub login: String,
    pub protocol: Protocol,
    #[serde(default)]
    pub detected_protocol: Option<Protocol>,
    pub context: Value,
    pub write_mode: WriteMode,
    #[serde(default)]
    pub write_until: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub revision: u64,
    #[serde(skip)]
    pub(crate) secret: crate::security::Secret,
    #[serde(skip)]
    pub(crate) proxy: crate::proxy::EnvironmentProxy,
}
impl std::fmt::Debug for Environment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Environment")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("url", &self.url)
            .field("database", &self.database)
            .field("login", &self.login)
            .field("protocol", &self.protocol)
            .field("context", &self.context)
            .field("write_mode", &self.write_mode)
            .field("write_until", &self.write_until)
            .field("tags", &self.tags)
            .field("revision", &self.revision)
            .field("secret", &"[redacted]")
            .field("proxy", &self.proxy)
            .finish()
    }
}
impl Environment {
    /// Mode effective at `Vault::now`; temporary write access without a future deadline is read-only.
    pub fn effective_write_mode(&self, now: chrono::DateTime<chrono::Utc>) -> WriteMode {
        if self.write_mode == WriteMode::Temporary
            && self.write_until.is_none_or(|until| until <= now)
        {
            WriteMode::ReadOnly
        } else {
            self.write_mode
        }
    }
}

#[derive(Clone, Deserialize)]
pub struct EnvironmentDraft {
    pub name: String,
    pub url: String,
    pub database: String,
    pub login: String,
    pub protocol: Protocol,
    #[serde(default = "empty_object")]
    pub context: Value,
    pub secret: String,
}
impl std::fmt::Debug for EnvironmentDraft {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EnvironmentDraft")
            .field("name", &self.name)
            .field("url", &self.url)
            .field("database", &self.database)
            .field("login", &self.login)
            .field("protocol", &self.protocol)
            .field("context", &self.context)
            .field("secret", &"[redacted]")
            .finish()
    }
}

fn empty_object() -> Value {
    Value::Object(Default::default())
}

impl EnvironmentDraft {
    pub fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty()
            || self.database.trim().is_empty()
            || self.login.trim().is_empty()
            || self.secret.is_empty()
        {
            return Err(OdxError::validation(
                "validation.environment.required_fields",
                "name, database, login and secret are required",
            ));
        }
        if self.context.is_null() || !self.context.is_object() {
            return Err(OdxError::validation(
                "validation.context.object_required",
                "context must be a JSON object",
            ));
        }
        validate_url(&self.url)?;
        if serde_json::to_vec(&self.context)?.len() > 1_048_576 {
            return Err(OdxError::validation(
                "validation.context.too_large",
                "context exceeds 1 MiB",
            ));
        }
        Ok(())
    }
    pub fn into_environment(mut self) -> Result<Environment> {
        self.validate()?;
        let plain = zeroize::Zeroizing::new(std::mem::take(&mut self.secret));
        let secret = crate::security::Secret::new(&plain)?;
        Ok(Environment {
            disabled_context: Default::default(),
            headers: Vec::new(),
            id: Uuid::new_v4().to_string(),
            name: self.name.clone(),
            url: self.url.trim_end_matches('/').into(),
            database: self.database.clone(),
            login: self.login.clone(),
            protocol: self.protocol,
            detected_protocol: None,
            context: self.context.clone(),
            write_mode: WriteMode::ReadOnly,
            write_until: None,
            tags: Vec::new(),
            revision: 1,
            secret,
            proxy: Default::default(),
        })
    }
}

/// Odoo base URL: HTTP(S) with a host and no credentials, query or fragment.
pub fn validate_url(url: &str) -> Result<()> {
    let url = Url::parse(url)
        .map_err(|_| OdxError::validation("validation.environment.url_invalid", "invalid URL"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(OdxError::validation(
            "validation.environment.url_restricted",
            "URL must use HTTP(S) without user, password, query or fragment",
        ));
    }
    Ok(())
}

pub const MAX_TAGS: usize = 20;
/// Tag colors besides the default look.
pub const TAG_COLORS: [&str; 5] = ["info", "ok", "warn", "danger", "accent"];
pub const MAX_TAG_CHARS: usize = 32;
/// Trimmed, nonempty tags without case-insensitive duplicates, keeping first spelling and order.
pub fn normalize_tags(tags: &[String]) -> Result<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    for tag in tags.iter().map(|t| t.trim()).filter(|t| !t.is_empty()) {
        if tag.chars().count() > MAX_TAG_CHARS {
            return Err(OdxError::validation(
                "validation.environment.tag_too_long",
                "tags are limited to 32 characters",
            ));
        }
        if !out.iter().any(|t| t.to_lowercase() == tag.to_lowercase()) {
            out.push(tag.to_owned());
        }
    }
    if out.len() > MAX_TAGS {
        return Err(OdxError::validation(
            "validation.environment.too_many_tags",
            "an environment can have at most 20 tags",
        ));
    }
    Ok(out)
}

impl Drop for EnvironmentDraft {
    fn drop(&mut self) {
        self.secret.zeroize();
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct VaultData {
    pub global_disabled_context: serde_json::Map<String, Value>,
    pub global_headers: Vec<crate::headers::HeaderEntry>,
    pub environments: Vec<Environment>,
    pub global_context: Value,
    pub clients: Vec<crate::clients::ClientGrant>,
    pub rules: Vec<crate::policy::Rule>,
    pub settings: Settings,
    /// Named outbound proxies.
    pub proxies: Vec<crate::proxy::ProxyProfile>,
    /// Proxy for environments that use the default; `None` means direct.
    pub default_proxy: Option<String>,
    /// Lowercase tag → color name; tags without one use the default look.
    pub tag_colors: std::collections::BTreeMap<String, String>,
    /// Saved expiry clock floor, restored on unlock.
    pub clock_floor: Option<chrono::DateTime<chrono::Utc>>,
}

/// Desktop-only settings: concurrency, upstream timeout, approval lifetime and retention. Defaults are also maximums; settings can only reduce them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    /// Global concurrent Odoo calls.
    pub global_concurrency: u32,
    /// Per-environment concurrency, bounded by the global limit.
    pub environment_concurrency: u32,
    /// Odoo HTTP request timeout in seconds.
    pub upstream_timeout_secs: u32,
    /// New approval lifetime in minutes.
    pub approval_minutes: u32,
    pub activity_days: u32,
    pub activity_mib: u32,
    pub output_hours: u32,
    pub output_mib: u32,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            global_concurrency: 16,
            environment_concurrency: 4,
            upstream_timeout_secs: 60,
            approval_minutes: 5,
            activity_days: 30,
            activity_mib: 256,
            output_hours: 24,
            output_mib: 512,
        }
    }
}
/// Inclusive setting range.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Bound {
    pub min: u32,
    pub max: u32,
}
impl Settings {
    /// Maximums with usable minimums: timeout ≥5 s and output quota ≥32 MiB.
    pub fn bounds() -> std::collections::BTreeMap<&'static str, Bound> {
        let d = Self::default();
        let b = |min, max| Bound { min, max };
        [
            ("global_concurrency", b(1, d.global_concurrency)),
            ("environment_concurrency", b(1, d.environment_concurrency)),
            ("upstream_timeout_secs", b(5, d.upstream_timeout_secs)),
            ("approval_minutes", b(1, d.approval_minutes)),
            ("activity_days", b(1, d.activity_days)),
            ("activity_mib", b(16, d.activity_mib)),
            ("output_hours", b(1, d.output_hours)),
            ("output_mib", b(32, d.output_mib)),
        ]
        .into()
    }
    fn values(&self) -> [(&'static str, u32); 8] {
        [
            ("global_concurrency", self.global_concurrency),
            ("environment_concurrency", self.environment_concurrency),
            ("upstream_timeout_secs", self.upstream_timeout_secs),
            ("approval_minutes", self.approval_minutes),
            ("activity_days", self.activity_days),
            ("activity_mib", self.activity_mib),
            ("output_hours", self.output_hours),
            ("output_mib", self.output_mib),
        ]
    }
    pub fn validate(&self) -> Result<()> {
        let bounds = Self::bounds();
        for (name, value) in self.values() {
            let Bound { min, max } = bounds[name];
            if !(min..=max).contains(&value) {
                return Err(OdxError::validation_with_params(
                    "validation.settings.out_of_range",
                    serde_json::json!({ "setting": name, "min": min, "max": max }),
                    format!("{name} must be between {min} and {max}"),
                ));
            }
        }
        if self.environment_concurrency > self.global_concurrency {
            return Err(OdxError::validation(
                "validation.settings.concurrency_order",
                "environment concurrency cannot exceed global concurrency",
            ));
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
struct StoredVaultData {
    #[serde(default)]
    global_disabled_context: serde_json::Map<String, Value>,
    #[serde(default)]
    global_headers: Vec<crate::headers::HeaderEntry>,
    environments: Vec<StoredEnvironment>,
    global_context: Value,
    #[serde(default)]
    clients: Vec<crate::clients::ClientGrant>,
    #[serde(default)]
    rules: Vec<crate::policy::Rule>,
    #[serde(default)]
    settings: Settings,
    #[serde(default)]
    proxies: Vec<crate::proxy::ProxyProfile>,
    #[serde(default)]
    default_proxy: Option<String>,
    #[serde(default)]
    tag_colors: std::collections::BTreeMap<String, String>,
    /// Old single global proxy, migrated to `proxies` on load.
    #[serde(default, skip_serializing)]
    proxy: Option<crate::proxy::ProxySettings>,
    #[serde(default)]
    clock_floor: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Serialize, Deserialize)]
struct StoredEnvironment {
    #[serde(default)]
    disabled_context: serde_json::Map<String, Value>,
    #[serde(default)]
    headers: Vec<crate::headers::HeaderEntry>,
    id: String,
    name: String,
    url: String,
    database: String,
    login: String,
    protocol: Protocol,
    #[serde(default)]
    detected_protocol: Option<Protocol>,
    context: Value,
    write_mode: WriteMode,
    #[serde(default)]
    write_until: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    tags: Vec<String>,
    revision: u64,
    secret: crate::security::Secret,
    #[serde(default)]
    proxy: crate::proxy::EnvironmentProxy,
}

impl From<&Environment> for StoredEnvironment {
    fn from(value: &Environment) -> Self {
        Self {
            disabled_context: value.disabled_context.clone(),
            headers: value.headers.clone(),
            id: value.id.clone(),
            name: value.name.clone(),
            url: value.url.clone(),
            database: value.database.clone(),
            login: value.login.clone(),
            protocol: value.protocol,
            detected_protocol: value.detected_protocol,
            context: value.context.clone(),
            write_mode: value.write_mode,
            write_until: value.write_until,
            tags: value.tags.clone(),
            revision: value.revision,
            secret: value.secret.clone(),
            proxy: value.proxy.clone(),
        }
    }
}

impl From<StoredEnvironment> for Environment {
    fn from(value: StoredEnvironment) -> Self {
        Self {
            disabled_context: value.disabled_context.clone(),
            headers: value.headers.clone(),
            id: value.id,
            name: value.name,
            url: value.url,
            database: value.database,
            login: value.login,
            protocol: value.protocol,
            detected_protocol: value.detected_protocol,
            context: value.context,
            write_mode: value.write_mode,
            write_until: value.write_until,
            tags: value.tags,
            revision: value.revision,
            secret: value.secret,
            proxy: value.proxy,
        }
    }
}

impl Serialize for VaultData {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        StoredVaultData {
            global_disabled_context: self.global_disabled_context.clone(),
            global_headers: self.global_headers.clone(),
            environments: self
                .environments
                .iter()
                .map(StoredEnvironment::from)
                .collect(),
            global_context: self.global_context.clone(),
            clients: self.clients.clone(),
            rules: self.rules.clone(),
            settings: self.settings,
            proxies: self.proxies.clone(),
            default_proxy: self.default_proxy.clone(),
            tag_colors: self.tag_colors.clone(),
            proxy: None,
            clock_floor: self.clock_floor,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for VaultData {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        let stored = StoredVaultData::deserialize(deserializer)?;
        let mut data = Self {
            global_disabled_context: stored.global_disabled_context,
            global_headers: stored.global_headers,
            environments: stored
                .environments
                .into_iter()
                .map(Environment::from)
                .collect(),
            global_context: stored.global_context,
            clients: stored.clients,
            rules: stored.rules,
            settings: stored.settings,
            proxies: stored.proxies,
            default_proxy: stored.default_proxy,
            tag_colors: stored.tag_colors,
            clock_floor: stored.clock_floor,
        };
        data.migrate_legacy_proxies(stored.proxy);
        Ok(data)
    }
}

impl Default for Environment {
    fn default() -> Self {
        Self {
            disabled_context: Default::default(),
            headers: Vec::new(),
            id: String::new(),
            name: String::new(),
            url: String::new(),
            database: String::new(),
            login: String::new(),
            protocol: Protocol::LegacyJsonrpc,
            detected_protocol: None,
            context: empty_object(),
            write_mode: WriteMode::ReadOnly,
            write_until: None,
            tags: Vec::new(),
            revision: 0,
            secret: Default::default(),
            proxy: Default::default(),
        }
    }
}

/// Nondecreasing expiry clock for temporary modes and rules. Takes the maximum of system time, unlock time plus monotonic elapsed time, previous observations and persisted history. Clock rollback cannot extend grants; forward jumps may expire them early.
pub(crate) struct Clock {
    started: std::time::Instant,
    started_wall: chrono::DateTime<chrono::Utc>,
    /// Greatest returned timestamp in microseconds.
    high: std::sync::atomic::AtomicI64,
}
impl Clock {
    pub(crate) fn new(floor: Option<chrono::DateTime<chrono::Utc>>) -> Self {
        let wall = chrono::Utc::now().max(floor.unwrap_or_default());
        Self {
            started: std::time::Instant::now(),
            started_wall: wall,
            high: std::sync::atomic::AtomicI64::new(wall.timestamp_micros()),
        }
    }
    pub(crate) fn now(&self) -> chrono::DateTime<chrono::Utc> {
        self.now_with(chrono::Utc::now())
    }
    fn now_with(&self, system: chrono::DateTime<chrono::Utc>) -> chrono::DateTime<chrono::Utc> {
        let elapsed = chrono::Duration::from_std(self.started.elapsed()).unwrap_or_default();
        let monotonic = self.started_wall + elapsed;
        let candidate = system.max(monotonic).timestamp_micros();
        let high = self
            .high
            .fetch_max(candidate, std::sync::atomic::Ordering::SeqCst)
            .max(candidate);
        // Overflow expires access using the maximum timestamp.
        chrono::DateTime::from_timestamp_micros(high)
            .unwrap_or(chrono::DateTime::<chrono::Utc>::MAX_UTC)
    }
}

pub fn validate_json(input: &[u8]) -> Result<Value> {
    if input.len() > 1_048_576 {
        return Err(OdxError::validation(
            "validation.json.too_large",
            "input exceeds 1 MiB",
        ));
    }
    struct Unique(Value);
    impl<'de> Deserialize<'de> for Unique {
        fn deserialize<D: serde::Deserializer<'de>>(
            deserializer: D,
        ) -> std::result::Result<Self, D::Error> {
            struct UniqueVisitor;
            impl<'de> Visitor<'de> for UniqueVisitor {
                type Value = Unique;
                fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                    f.write_str("JSON without duplicate keys")
                }
                fn visit_unit<E: de::Error>(self) -> std::result::Result<Unique, E> {
                    Ok(Unique(Value::Null))
                }
                fn visit_none<E: de::Error>(self) -> std::result::Result<Unique, E> {
                    Ok(Unique(Value::Null))
                }
                fn visit_bool<E: de::Error>(self, v: bool) -> std::result::Result<Unique, E> {
                    Ok(Unique(Value::Bool(v)))
                }
                fn visit_i64<E: de::Error>(self, v: i64) -> std::result::Result<Unique, E> {
                    Ok(Unique(v.into()))
                }
                fn visit_u64<E: de::Error>(self, v: u64) -> std::result::Result<Unique, E> {
                    Ok(Unique(v.into()))
                }
                fn visit_f64<E: de::Error>(self, v: f64) -> std::result::Result<Unique, E> {
                    serde_json::Number::from_f64(v)
                        .map(|n| Unique(Value::Number(n)))
                        .ok_or_else(|| E::custom("non-finite number"))
                }
                fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<Unique, E> {
                    Ok(Unique(Value::String(v.to_owned())))
                }
                fn visit_string<E: de::Error>(self, v: String) -> std::result::Result<Unique, E> {
                    Ok(Unique(Value::String(v)))
                }
                fn visit_seq<A: SeqAccess<'de>>(
                    self,
                    mut seq: A,
                ) -> std::result::Result<Unique, A::Error> {
                    let mut values = Vec::new();
                    while let Some(Unique(v)) = seq.next_element()? {
                        values.push(v)
                    }
                    Ok(Unique(Value::Array(values)))
                }
                fn visit_map<A: MapAccess<'de>>(
                    self,
                    mut map: A,
                ) -> std::result::Result<Unique, A::Error> {
                    let mut values = serde_json::Map::new();
                    while let Some((key, Unique(value))) = map.next_entry::<String, Unique>()? {
                        if values.contains_key(&key) {
                            return Err(de::Error::custom(format!("duplicate JSON key: {key}")));
                        }
                        values.insert(key, value);
                    }
                    Ok(Unique(Value::Object(values)))
                }
            }
            deserializer.deserialize_any(UniqueVisitor)
        }
    }
    let mut deserializer = serde_json::Deserializer::from_slice(input);
    let Unique(value) = Unique::deserialize(&mut deserializer)?;
    deserializer.end()?;
    fn depth(v: &Value, d: usize) -> bool {
        if d > 32 {
            return false;
        }
        match v {
            Value::Array(a) => a.iter().all(|x| depth(x, d + 1)),
            Value::Object(o) => o.values().all(|x| depth(x, d + 1)),
            _ => true,
        }
    }
    if !depth(&value, 0) {
        return Err(OdxError::validation(
            "validation.json.too_deep",
            "JSON nesting exceeds 32",
        ));
    }
    fn exact_integer_range(v: &Value) -> bool {
        match v {
            Value::Number(n) => {
                let max = 9_007_199_254_740_991u64;
                if let Some(i) = n.as_i64() {
                    i >= -(max as i64) && i <= max as i64
                } else if let Some(u) = n.as_u64() {
                    u <= max
                } else if let Some(f) = n.as_f64() {
                    f.fract() != 0.0 || f.abs() <= max as f64
                } else {
                    true
                }
            }
            Value::Array(a) => a.iter().all(exact_integer_range),
            Value::Object(o) => o.values().all(exact_integer_range),
            _ => true,
        }
    }
    if !exact_integer_range(&value) {
        return Err(OdxError::validation(
            "validation.json.integer_out_of_range",
            "integer outside the exact JSON range",
        ));
    }
    Ok(value)
}

pub fn positive_exact_id(value: &Value) -> Result<u64> {
    value
        .as_u64()
        .filter(|id| *id > 0 && *id <= 9_007_199_254_740_991)
        .ok_or_else(|| {
            OdxError::validation(
                "validation.json.id_invalid",
                "ID must be an exact positive integer",
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn draft() -> EnvironmentDraft {
        EnvironmentDraft {
            name: "test".into(),
            url: "https://example.test".into(),
            database: "db".into(),
            login: "fake".into(),
            protocol: Protocol::LegacyJsonrpc,
            context: empty_object(),
            secret: "fixture-key".into(),
        }
    }
    #[test]
    fn rejects_url_credentials_and_context_arrays() {
        let mut d = draft();
        d.url = "https://user:pass@example.test".into();
        assert!(d.validate().is_err());
        let mut d = draft();
        d.context = Value::Array(vec![]);
        assert!(d.validate().is_err());
    }
    #[test]
    fn new_environment_is_read_only_and_keeps_secret_private() {
        let e = draft().into_environment().unwrap();
        assert_eq!(e.write_mode, WriteMode::ReadOnly);
        assert_eq!(e.secret, "fixture-key");
        assert!(!format!("{e:?}").contains("fixture-key"));
        assert!(!serde_json::to_string(&e).unwrap().contains("fixture-key"));
        assert!(!format!("{:?}", draft()).contains("fixture-key"));
    }
    #[test]
    fn clock_never_goes_back_and_starts_from_the_saved_floor() {
        use chrono::{DateTime, Duration, Utc};
        // The clock stores microseconds.
        let micros =
            |at: DateTime<Utc>| DateTime::from_timestamp_micros(at.timestamp_micros()).unwrap();
        // After restart, saved time overrides a rolled-back system clock.
        let floor = micros(Utc::now() + Duration::hours(2));
        let clock = Clock::new(Some(floor));
        assert!(clock.now() >= floor);
        let mut env = Environment {
            write_mode: WriteMode::Temporary,
            write_until: Some(Utc::now() + Duration::hours(1)),
            ..Default::default()
        };
        assert_eq!(env.effective_write_mode(clock.now()), WriteMode::ReadOnly);
        // Forward and backward jumps cannot move the clock backward.
        let clock = Clock::new(None);
        let ahead = micros(Utc::now() + Duration::hours(1));
        assert!(clock.now_with(ahead) >= ahead);
        assert!(clock.now_with(Utc::now() - Duration::hours(1)) >= ahead);
        env.write_until = Some(Utc::now() + Duration::minutes(30));
        assert_eq!(env.effective_write_mode(clock.now()), WriteMode::ReadOnly);
        // Missing deadlines fall back to read-only.
        env.write_until = None;
        assert_eq!(env.effective_write_mode(Utc::now()), WriteMode::ReadOnly);
        // Without a floor or clock jumps, future access remains active.
        env.write_until = Some(Utc::now() + Duration::minutes(5));
        assert_eq!(
            env.effective_write_mode(Clock::new(None).now()),
            WriteMode::Temporary
        );
    }
    #[test]
    fn json_rejects_duplicate_keys_depth_and_inexact_integers() {
        assert!(validate_json(br#"{"a":1,"a":2}"#).is_err());
        assert!(validate_json(br#"9007199254740992"#).is_err());
        let mut too_deep = serde_json::json!(0);
        for _ in 0..33 {
            too_deep = Value::Array(vec![too_deep]);
        }
        assert!(validate_json(&serde_json::to_vec(&too_deep).unwrap()).is_err());
        assert!(validate_json(&vec![b' '; 1_048_577]).is_err());
        assert!(positive_exact_id(&serde_json::json!(0)).is_err());
        assert!(positive_exact_id(&serde_json::json!(-1)).is_err());
        assert_eq!(
            positive_exact_id(&serde_json::json!(9007199254740991u64)).unwrap(),
            9007199254740991
        );
    }
}
