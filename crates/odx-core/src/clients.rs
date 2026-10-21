//! API/MCP clients with high-entropy tokens and environment grants. Persist only SHA-256 hashes in the encrypted vault; return plaintext once on creation or replacement.
use crate::error::{OdxError, Result};
use chrono::{DateTime, Utc};
use libsodium_rs::{crypto_hash::sha256, crypto_verify, ensure_init, random};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use zeroize::Zeroize;

pub(crate) const STDIO_CLIENT_ID: &str = "local-mcp-stdio";
// Persisted profile label; preserve existing grants.
pub(crate) const STDIO_CLIENT_LABEL: &str = "MCP local";

pub(crate) fn new_stdio_grant() -> ClientGrant {
    let mut grant = new_grant(
        STDIO_CLIENT_LABEL,
        ClientScope::All,
        vec![Capability::OdooRead, Capability::OdooWriteRequest],
        String::new(),
    );
    grant.id = STDIO_CLIENT_ID.into();
    grant
}

const TOKEN_PREFIX: &str = "odx_";
const TOKEN_BYTES: usize = 32;
const MAX_LABEL: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Capability {
    #[serde(rename = "odoo:read")]
    OdooRead,
    #[serde(rename = "odoo:write-request")]
    OdooWriteRequest,
}

/// Client environment scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientScope {
    /// All current and future environments.
    All,
    /// Only these IDs.
    Only(Vec<String>),
}
impl From<Vec<String>> for ClientScope {
    fn from(ids: Vec<String>) -> Self {
        ClientScope::Only(ids)
    }
}

/// Persisted grant, private to the core; the UI receives [`ClientView`].
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct ClientGrant {
    pub id: String,
    pub label: String,
    /// Hex-encoded SHA-256 token hash.
    pub token_hash: String,
    pub environment_ids: Vec<String>,
    /// All current and future environments; ignores `environment_ids`.
    #[serde(default)]
    pub all_environments: bool,
    pub capabilities: Vec<Capability>,
    pub revision: u64,
    pub revoked: bool,
    pub created_at: DateTime<Utc>,
    pub token_issued_at: DateTime<Utc>,
}

impl std::fmt::Debug for ClientGrant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientGrant")
            .field("id", &self.id)
            .field("label", &self.label)
            .field("token_hash", &"[redacted]")
            .field("environment_ids", &self.environment_ids)
            .field("all_environments", &self.all_environments)
            .field("capabilities", &self.capabilities)
            .field("revision", &self.revision)
            .field("revoked", &self.revoked)
            .finish()
    }
}

/// Admin grant view without token or hash.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientView {
    pub local_stdio: bool,
    pub id: String,
    pub label: String,
    pub environment_ids: Vec<String>,
    pub all_environments: bool,
    pub capabilities: Vec<Capability>,
    pub revoked: bool,
    pub created_at: DateTime<Utc>,
    pub token_issued_at: DateTime<Utc>,
    /// Last client call from history or this session; `None` if unused.
    pub last_used_at: Option<DateTime<Utc>>,
}
impl From<&ClientGrant> for ClientView {
    fn from(g: &ClientGrant) -> Self {
        Self {
            local_stdio: g.id == STDIO_CLIENT_ID,
            id: g.id.clone(),
            label: g.label.clone(),
            environment_ids: g.environment_ids.clone(),
            all_environments: g.all_environments,
            capabilities: g.capabilities.clone(),
            revoked: g.revoked,
            created_at: g.created_at,
            token_issued_at: g.token_issued_at,
            last_used_at: None,
        }
    }
}

/// Creation/replacement result; the only plaintext token exposure.
#[derive(Serialize)]
pub struct IssuedToken {
    pub client: ClientView,
    pub token: String,
}
impl std::fmt::Debug for IssuedToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IssuedToken")
            .field("client", &self.client)
            .field("token", &"[redacted]")
            .finish()
    }
}
impl Drop for IssuedToken {
    fn drop(&mut self) {
        self.token.zeroize();
    }
}

/// Authenticated identity attached to each transport call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientIdentity {
    pub client_id: String,
    pub revision: u64,
    pub environment_ids: Vec<String>,
    pub capabilities: Vec<Capability>,
}
impl ClientIdentity {
    pub fn allows(&self, environment_id: &str, capability: Capability) -> bool {
        self.capabilities.contains(&capability)
            && self.environment_ids.iter().any(|id| id == environment_id)
    }
}

/// Generate a token and hash without storing the plaintext token.
pub(crate) fn issue_token() -> Result<(String, String)> {
    ensure_init().map_err(|_| OdxError::Persistence)?;
    let mut bytes = [0u8; TOKEN_BYTES];
    random::fill_bytes(&mut bytes);
    let token = format!("{TOKEN_PREFIX}{}", hex::encode(bytes));
    bytes.zeroize();
    let hash = hex::encode(sha256::hash(token.as_bytes()));
    Ok((token, hash))
}

/// Find the active grant by comparing all hashes in constant time.
/// Identity for a grant; `all_environments` expands to the current environments.
pub(crate) fn identity(grant: &ClientGrant, data: &crate::config::VaultData) -> ClientIdentity {
    ClientIdentity {
        client_id: grant.id.clone(),
        revision: grant.revision,
        environment_ids: if grant.all_environments {
            data.environments.iter().map(|e| e.id.clone()).collect()
        } else {
            grant.environment_ids.clone()
        },
        capabilities: grant.capabilities.clone(),
    }
}
pub(crate) fn find_grant<'a>(grants: &'a [ClientGrant], token: &str) -> Option<&'a ClientGrant> {
    if !token.starts_with(TOKEN_PREFIX) || token.len() != TOKEN_PREFIX.len() + TOKEN_BYTES * 2 {
        return None;
    }
    let presented = sha256::hash(token.as_bytes());
    let mut found = None;
    for grant in grants {
        if grant.id == STDIO_CLIENT_ID {
            continue;
        }
        let Ok(stored) = hex::decode(&grant.token_hash) else {
            continue;
        };
        if crypto_verify::verify_32(&presented, &stored) && !grant.revoked && found.is_none() {
            found = Some(grant);
        }
    }
    found
}

pub(crate) fn validate_grant(
    label: &str,
    scope: &ClientScope,
    capabilities: &[Capability],
    known_environment: impl Fn(&str) -> bool,
) -> Result<()> {
    let label = label.trim();
    if label.is_empty() || label.chars().count() > MAX_LABEL {
        return Err(OdxError::validation(
            "validation.client.label_invalid",
            "client name is required (max 200 characters)",
        ));
    }
    if let ClientScope::Only(environment_ids) = scope {
        if environment_ids.is_empty() {
            return Err(OdxError::validation(
                "validation.client.environment_required",
                "select at least one environment",
            ));
        }
        if let Some(unknown) = environment_ids.iter().find(|id| !known_environment(id)) {
            return Err(OdxError::validation(
                "validation.client.environment_unknown",
                format!("unknown environment: {unknown}"),
            ));
        }
    }
    if capabilities.is_empty() {
        return Err(OdxError::validation(
            "validation.client.capability_required",
            "select at least one capability",
        ));
    }
    Ok(())
}

pub(crate) fn new_grant(
    label: &str,
    scope: ClientScope,
    capabilities: Vec<Capability>,
    token_hash: String,
) -> ClientGrant {
    let now = Utc::now();
    let (all_environments, mut environment_ids) = match scope {
        ClientScope::All => (true, vec![]),
        ClientScope::Only(ids) => (false, ids),
    };
    environment_ids.sort();
    environment_ids.dedup();
    let mut capabilities = capabilities;
    capabilities.sort_by_key(|c| *c as u8);
    capabilities.dedup();
    ClientGrant {
        id: Uuid::new_v4().to_string(),
        label: label.trim().into(),
        token_hash,
        environment_ids,
        all_environments,
        capabilities,
        revision: 1,
        revoked: false,
        created_at: now,
        token_issued_at: now,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stdio_profile_cannot_authenticate_http_even_with_a_hash() {
        let (token, hash) = issue_token().unwrap();
        let mut grant = new_grant(
            "MCP local",
            ClientScope::All,
            vec![Capability::OdooRead],
            hash,
        );
        grant.id = STDIO_CLIENT_ID.into();
        assert!(find_grant(&[grant], &token).is_none());
    }

    #[test]
    fn tokens_are_unique_prefixed_and_only_match_their_own_hash() {
        let (a, hash_a) = issue_token().unwrap();
        let (b, hash_b) = issue_token().unwrap();
        assert_ne!(a, b);
        assert!(a.starts_with(TOKEN_PREFIX) && a.len() == 68);
        assert!(!hash_a.contains(&a[TOKEN_PREFIX.len()..]));
        let grants = vec![
            new_grant(
                "a",
                vec!["e".to_string()].into(),
                vec![Capability::OdooRead],
                hash_a,
            ),
            new_grant(
                "b",
                vec!["e".to_string()].into(),
                vec![Capability::OdooRead],
                hash_b,
            ),
        ];
        assert_eq!(find_grant(&grants, &a).unwrap().label, "a");
        assert_eq!(find_grant(&grants, &b).unwrap().label, "b");
        assert!(find_grant(&grants, "odx_corto").is_none());
        let mut flipped = a.clone();
        flipped.replace_range(10..11, if &a[10..11] == "0" { "1" } else { "0" });
        assert!(find_grant(&grants, &flipped).is_none());
    }
}
