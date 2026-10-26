//! API/MCP clients with high-entropy tokens and environment grants. Persist only SHA-256 hashes in the encrypted vault; return plaintext once on creation or replacement.
use crate::error::{OdxError, Result};
use crate::{
    approval::revoke_client,
    execution::{Core, Inner},
};
use chrono::{DateTime, Utc};
use libsodium_rs::{crypto_hash::sha256, crypto_verify, ensure_init, random};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc};
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
/// Find the active grant by comparing all hashes in constant time.
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

// Clients on the core: listing, usage, connections and the token lifecycle.

/// In-memory last use and open MCP connections.
#[derive(Default)]
pub(crate) struct Usage {
    last_used: HashMap<String, chrono::DateTime<Utc>>,
    /// Vault session whose history initialized `last_used`.
    seeded_session: Option<u64>,
    connections: HashMap<u64, OpenConnection>,
    next_connection: u64,
}

struct OpenConnection {
    client_id: String,
    since: chrono::DateTime<Utc>,
    agent: AgentProcess,
    calls: u64,
    close: Arc<tokio::sync::Notify>,
}

/// Process that launched a connector, such as a `claude` session.
#[derive(Debug, Clone, Default, Serialize)]
pub struct AgentProcess {
    pub pid: Option<u32>,
    pub command: Option<String>,
    pub cwd: Option<String>,
}

/// Open MCP connection shown in the app.
#[derive(Debug, Clone, Serialize)]
pub struct McpConnectionInfo {
    pub id: u64,
    pub client_id: String,
    pub client_label: Option<String>,
    pub since: chrono::DateTime<Utc>,
    #[serde(flatten)]
    pub agent: AgentProcess,
    /// Tool calls made on this connection.
    pub calls: u64,
}

/// Count the connection as open while this guard exists.
pub struct ConnectionGuard {
    core: Arc<Core>,
    id: u64,
    close: Arc<tokio::sync::Notify>,
}

impl ConnectionGuard {
    pub fn called(&self) {
        if let Ok(mut usage) = self.core.usage.lock()
            && let Some(connection) = usage.connections.get_mut(&self.id)
        {
            connection.calls += 1;
        }
    }
    /// Resolve when the app asks to close this connection.
    pub async fn closed(&self) {
        self.close.notified().await
    }
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        if let Ok(mut usage) = self.core.usage.lock() {
            usage.connections.remove(&self.id);
        }
    }
}

impl Core {
    /// After client changes, revoke and record its unsent requests.
    async fn client_changed(&self, mut s: tokio::sync::MutexGuard<'_, Inner>, id: &str) {
        let revoked = revoke_client(&mut s.pending, id);
        let ended = s.end(revoked, "revoked");
        drop(s);
        self.record(ended).await;
    }
    pub async fn clients(&self) -> Result<Vec<ClientView>> {
        let (mut views, audit, session) = {
            let s = self.inner.lock().await;
            let v = s.vault()?;
            let views: Vec<ClientView> = v
                .data()
                .clients
                .iter()
                .filter(|c| !c.revoked)
                .map(ClientView::from)
                .collect();
            (views, s.audit.clone(), s.session)
        };
        // Initialize last use from history once per session, then track it in memory.
        let seeded = self
            .usage
            .lock()
            .map(|u| u.seeded_session == Some(session))
            .unwrap_or(true);
        if !seeded && let Some(audit) = audit {
            let history = tokio::task::spawn_blocking(move || audit.last_used())
                .await
                .ok()
                .and_then(|r| r.ok());
            if let (Some(events), Ok(mut usage)) = (history, self.usage.lock()) {
                for (id, at) in events {
                    let last = usage.last_used.entry(id).or_insert(at);
                    if at > *last {
                        *last = at;
                    }
                }
                usage.seeded_session = Some(session);
            }
        }
        if let Ok(usage) = self.usage.lock() {
            for view in &mut views {
                view.last_used_at = usage.last_used.get(&view.id).copied();
            }
        }
        Ok(views)
    }
    pub(crate) fn mark_used(&self, client_id: &str) {
        if let Ok(mut usage) = self.usage.lock() {
            usage.last_used.insert(client_id.into(), Utc::now());
        }
    }
    /// Register an authenticated MCP connection until its guard drops.
    pub fn register_connection(
        self: &Arc<Self>,
        client_id: &str,
        agent: AgentProcess,
    ) -> ConnectionGuard {
        let mut usage = self.usage.lock().expect("usage");
        usage.next_connection += 1;
        let id = usage.next_connection;
        let close = Arc::new(tokio::sync::Notify::new());
        usage.connections.insert(
            id,
            OpenConnection {
                client_id: client_id.into(),
                since: Utc::now(),
                agent,
                calls: 0,
                close: close.clone(),
            },
        );
        ConnectionGuard {
            core: self.clone(),
            id,
            close,
        }
    }
    /// Close an open MCP connection; its connector exits and its unsent requests are cancelled.
    pub fn close_connection(&self, id: u64) -> bool {
        let usage = self.usage.lock().expect("usage");
        let Some(connection) = usage.connections.get(&id) else {
            return false;
        };
        // notify_one keeps the wakeup if the connection is not waiting yet.
        connection.close.notify_one();
        true
    }
    pub async fn mcp_connections(&self) -> Vec<McpConnectionInfo> {
        let labels: HashMap<String, String> = {
            let s = self.inner.lock().await;
            s.vault
                .as_ref()
                .map(|v| {
                    v.data()
                        .clients
                        .iter()
                        .map(|c| (c.id.clone(), c.label.clone()))
                        .collect()
                })
                .unwrap_or_default()
        };
        let usage = self.usage.lock().expect("usage");
        let mut list: Vec<McpConnectionInfo> = usage
            .connections
            .iter()
            .map(|(id, c)| McpConnectionInfo {
                id: *id,
                client_label: labels.get(&c.client_id).cloned(),
                client_id: c.client_id.clone(),
                since: c.since,
                agent: c.agent.clone(),
                calls: c.calls,
            })
            .collect();
        list.sort_by_key(|c| c.id);
        list
    }
    /// Create a client; expose plaintext only in the returned value.
    pub async fn create_client(
        &self,
        label: &str,
        scope: impl Into<ClientScope>,
        capabilities: Vec<Capability>,
    ) -> Result<IssuedToken> {
        let scope = scope.into();
        let mut s = self.inner.lock().await;
        let v = s.vault_mut()?;
        let data = v.data();
        validate_grant(label, &scope, &capabilities, |id| {
            data.environments.iter().any(|e| e.id == id)
        })?;
        if data
            .clients
            .iter()
            .any(|c| !c.revoked && c.label.eq_ignore_ascii_case(label.trim()))
        {
            return Err(OdxError::validation(
                "validation.client.name_exists",
                "a client with this name already exists",
            ));
        }
        let (token, hash) = issue_token()?;
        let grant = new_grant(label, scope, capabilities, hash);
        let client = ClientView::from(&grant);
        v.update(|data| {
            data.clients.push(grant);
            Ok(())
        })?;
        Ok(IssuedToken { client, token })
    }
    /// Update label, scope and capabilities without changing the token; the local MCP label is fixed. Revoke requests made under the old grant.
    pub async fn update_client(
        &self,
        id: &str,
        label: &str,
        scope: impl Into<ClientScope>,
        capabilities: Vec<Capability>,
    ) -> Result<ClientView> {
        if id == STDIO_CLIENT_ID && label != STDIO_CLIENT_LABEL {
            return Err(OdxError::validation(
                "validation.client.local_profile_name_fixed",
                "the local MCP profile name cannot change",
            ));
        }
        let scope = scope.into();
        let mut s = self.inner.lock().await;
        let v = s.vault_mut()?;
        let data = v.data();
        validate_grant(label, &scope, &capabilities, |env| {
            data.environments.iter().any(|e| e.id == env)
        })?;
        if data
            .clients
            .iter()
            .any(|c| c.id != id && !c.revoked && c.label.eq_ignore_ascii_case(label.trim()))
        {
            return Err(OdxError::validation(
                "validation.client.name_exists",
                "a client with this name already exists",
            ));
        }
        let updated = new_grant(label, scope, capabilities, String::new());
        let view = v.update(|data| {
            let grant = data
                .clients
                .iter_mut()
                .find(|c| c.id == id && !c.revoked)
                .ok_or(OdxError::ClientNotFound)?;
            grant.label = updated.label;
            grant.all_environments = updated.all_environments;
            grant.environment_ids = updated.environment_ids;
            grant.capabilities = updated.capabilities;
            Ok(ClientView::from(&*grant))
        })?;
        self.client_changed(s, id).await;
        let _ = self.grant_changes.send(id.to_string());
        Ok(view)
    }
    /// Replace the token, invalidate the old one and revoke pending requests.
    pub async fn replace_client_token(&self, id: &str) -> Result<IssuedToken> {
        if id == STDIO_CLIENT_ID {
            return Err(OdxError::validation(
                "validation.client.local_profile_no_token",
                "the MCP stdio profile does not use a token",
            ));
        }
        let mut s = self.inner.lock().await;
        let v = s.vault_mut()?;
        let (token, hash) = issue_token()?;
        let client = v.update(|data| {
            let grant = data
                .clients
                .iter_mut()
                .find(|c| c.id == id && !c.revoked)
                .ok_or(OdxError::ClientNotFound)?;
            grant.token_hash = hash;
            grant.revision += 1;
            grant.token_issued_at = Utc::now();
            Ok(ClientView::from(&*grant))
        })?;
        self.client_changed(s, id).await;
        Ok(IssuedToken { client, token })
    }
    /// Delete the client permanently, invalidate its token and revoke pending requests. Retain its ID without a label in activity.
    pub async fn revoke_client(&self, id: &str) -> Result<()> {
        if id == STDIO_CLIENT_ID {
            return Err(OdxError::validation(
                "validation.client.local_profile_shared",
                "the MCP stdio profile is shared; edit its scope and capabilities or lock the app",
            ));
        }
        let mut s = self.inner.lock().await;
        let v = s.vault_mut()?;
        // Deleting the client invalidates its token and removes it from the list. Also purge legacy revoked grants.
        v.update(|data| {
            if !data.clients.iter().any(|c| c.id == id && !c.revoked) {
                return Err(OdxError::ClientNotFound);
            }
            data.clients.retain(|c| c.id != id && !c.revoked);
            Ok(())
        })?;
        self.client_changed(s, id).await;
        Ok(())
    }
    /// Shared stdio profile authenticated by socket UID, never HTTP token.
    pub(crate) async fn stdio_identity(&self) -> Result<ClientIdentity> {
        let s = self.inner.lock().await;
        let vault = s.vault()?;
        let grant = vault
            .data()
            .clients
            .iter()
            .find(|g| g.id == STDIO_CLIENT_ID && !g.revoked)
            .ok_or(OdxError::Unauthorized)?;
        Ok(identity(grant, vault.data()))
    }
    /// Authenticate a client token; reject all clients while locked.
    pub async fn authenticate_client(&self, token: &str) -> Result<ClientIdentity> {
        let s = self.inner.lock().await;
        let v = s.vault()?;
        let grant = find_grant(&v.data().clients, token).ok_or(OdxError::Unauthorized)?;
        Ok(identity(grant, v.data()))
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
