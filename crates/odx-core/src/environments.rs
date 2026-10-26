//! Environments on the core: views, create/update/clone/delete, write modes, tags, health checks and protocol
//! detection.
use crate::{
    approval::revoke_environment,
    config::{Environment, EnvironmentDraft, WriteMode},
    error::{OdxError, Result},
    execution::{Core, Inner},
    tools::args::valid_model,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// Shared limits key for unsaved drafts; per-call random ids would grow the slot map forever.
pub(crate) const DRAFT_SLOTS: &str = "\0draft";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentView {
    pub id: String,
    pub name: String,
    pub url: String,
    pub database: String,
    pub login: String,
    pub protocol: crate::config::Protocol,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detected_protocol: Option<crate::config::Protocol>,
    pub context: Value,
    pub write_mode: WriteMode,
    pub write_until: Option<chrono::DateTime<Utc>>,
    pub has_secret: bool,
    #[serde(default)]
    pub tags: Vec<String>,
}

impl EnvironmentView {
    /// View with the effective mode at the vault expiry clock's `now`.
    pub(crate) fn new(e: &Environment, now: chrono::DateTime<Utc>) -> Self {
        Self {
            id: e.id.clone(),
            name: e.name.clone(),
            url: e.url.clone(),
            database: e.database.clone(),
            login: e.login.clone(),
            protocol: e.protocol,
            detected_protocol: e.detected_protocol,
            context: e.context.clone(),
            write_mode: e.effective_write_mode(now),
            write_until: e.write_until,
            has_secret: !e.secret.is_empty(),
            tags: e.tags.clone(),
        }
    }
}

/// Environment connectivity without business data or secrets.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthReport {
    pub environment_id: String,
    /// `ok`, `unreachable` or `auth_failed`.
    pub status: String,
    pub server_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<crate::config::Protocol>,
    pub latency_ms: u64,
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_params: Option<Value>,
    /// HTTP error status, if Odoo responded.
    pub http_status: Option<u16>,
    /// Network failure kind: timeout, dns, tls, refused, unreachable, reset or network.
    pub network_error: Option<String>,
}

impl Core {
    pub async fn environments(&self) -> Result<Vec<EnvironmentView>> {
        let s = self.inner.lock().await;
        let v = s.vault()?;
        let now = v.now();
        Ok(v.data()
            .environments
            .iter()
            .map(|e| EnvironmentView::new(e, now))
            .collect())
    }
    /// Web client URL of an environment, or of one of its records. The legacy `/web#` form URL works in Odoo 16 to
    /// 19 (newer versions redirect it).
    pub async fn web_url(&self, id: &str, record: Option<(&str, u64)>) -> Result<String> {
        let base = self
            .inner
            .lock()
            .await
            .vault()?
            .data()
            .environment(id)?
            .url
            .clone();
        if !(base.starts_with("http://") || base.starts_with("https://")) {
            return Err(OdxError::Invalid(
                "the environment URL is not http(s)".into(),
            ));
        }
        match record {
            None => Ok(base),
            Some((model, id)) if valid_model(model) && id > 0 => Ok(format!(
                "{}/web#id={id}&model={model}&view_type=form",
                base.trim_end_matches('/')
            )),
            Some(_) => Err(OdxError::Invalid("invalid record".into())),
        }
    }
    pub async fn save_environment(&self, draft: EnvironmentDraft) -> Result<EnvironmentView> {
        let mut s = self.inner.lock().await;
        let v = s.vault_mut()?;
        draft.validate()?;
        v.data().check_environment_name(&draft.name, None)?;
        let env = draft.into_environment()?;
        let view = EnvironmentView::new(&env, v.now());
        v.update(|data| {
            data.environments.push(env);
            Ok(())
        })?;
        Ok(view)
    }
    pub async fn update_environment(
        &self,
        id: &str,
        mut draft: EnvironmentDraft,
    ) -> Result<EnvironmentView> {
        let mut s = self.inner.lock().await;
        let v = s.vault_mut()?;
        let old = v.data().environment(id)?.clone();
        let keep_secret = draft.secret.is_empty();
        if keep_secret {
            draft.secret = old.secret.as_str().to_owned();
        }
        draft.validate()?;
        v.data().check_environment_name(&draft.name, Some(id))?;
        let mut updated = draft.into_environment()?;
        // Context, headers and proxy have their own editors; connection updates preserve them.
        updated.context = old.context.clone();
        updated.disabled_context = old.disabled_context.clone();
        updated.headers = old.headers.clone();
        updated.proxy = old.proxy.clone();
        updated.tags = old.tags.clone();
        updated.id = old.id.clone();
        updated.revision = old.revision + 1;
        updated.write_mode = old.write_mode;
        updated.write_until = old.write_until;
        let view = EnvironmentView::new(&updated, v.now());
        // Do not transfer rules to a different server, database or user.
        let retarget = (&old.url, &old.database, &old.login)
            != (&updated.url, &updated.database, &updated.login);
        // Never send the old key to a different server, database or user.
        if retarget && keep_secret {
            return Err(OdxError::validation(
                "validation.environment.secret_required",
                "a new connection target requires its API key",
            ));
        }
        v.update(|data| {
            let env = data.environment_mut(id)?;
            *env = updated;
            if retarget {
                data.rules.retain(|r| r.environment_id != id);
            }
            Ok(())
        })?;
        self.environment_changed(s, &[id.to_owned()]).await;
        Ok(view)
    }
    /// Tags only organize the list; they keep the revision and pending approvals.
    pub async fn set_environment_tags(&self, id: &str, tags: &[String]) -> Result<EnvironmentView> {
        let tags = crate::config::normalize_tags(tags)?;
        let mut s = self.inner.lock().await;
        let v = s.vault_mut()?;
        let now = v.now();
        let mut view = None;
        v.update(|data| {
            let env = data.environment_mut(id)?;
            env.tags = tags;
            view = Some(EnvironmentView::new(env, now));
            Ok(())
        })?;
        view.ok_or(OdxError::EnvironmentNotFound)
    }
    pub async fn tag_colors(&self) -> Result<std::collections::BTreeMap<String, String>> {
        let s = self.inner.lock().await;
        Ok(s.vault()?.data().tag_colors.clone())
    }
    /// Color a tag for every environment that has it; `None` restores the default look. Colors of tags no environment uses are dropped.
    pub async fn set_tag_color(&self, tag: &str, color: Option<&str>) -> Result<()> {
        let tag = crate::config::normalize_tags(&[tag.to_owned()])?
            .pop()
            .ok_or_else(|| {
                OdxError::validation("validation.environment.tag_required", "tag required")
            })?
            .to_lowercase();
        if color.is_some_and(|c| !crate::config::TAG_COLORS.contains(&c)) {
            return Err(OdxError::validation(
                "validation.environment.tag_color_invalid",
                "unknown tag color",
            ));
        }
        let mut s = self.inner.lock().await;
        let v = s.vault_mut()?;
        v.update(|data| {
            let used: std::collections::HashSet<String> = data
                .environments
                .iter()
                .flat_map(|e| e.tags.iter().map(|t| t.to_lowercase()))
                .collect();
            data.tag_colors.retain(|t, _| used.contains(t));
            match color {
                Some(color) => data.tag_colors.insert(tag, color.to_owned()),
                None => data.tag_colors.remove(&tag),
            };
            Ok(())
        })
    }
    pub async fn clone_environment(&self, id: &str, new_name: &str) -> Result<EnvironmentView> {
        if new_name.trim().is_empty() {
            return Err(OdxError::validation(
                "validation.environment.name_required",
                "nonempty name required",
            ));
        }
        let mut s = self.inner.lock().await;
        let v = s.vault_mut()?;
        v.data().check_environment_name(new_name, None)?;
        let mut clone = v.data().environment(id)?.clone();
        clone.id = Uuid::new_v4().to_string();
        clone.name = new_name.into();
        clone.write_mode = WriteMode::ReadOnly;
        clone.write_until = None;
        clone.revision = 1;
        let view = EnvironmentView::new(&clone, v.now());
        v.update(|data| {
            data.environments.push(clone);
            Ok(())
        })?;
        Ok(view)
    }
    pub async fn delete_environment(&self, id: &str, confirmed: bool) -> Result<()> {
        if !confirmed {
            return Err(OdxError::validation(
                "validation.environment.delete_confirmation",
                "confirm environment deletion",
            ));
        }
        let mut s = self.inner.lock().await;
        let v = s.vault_mut()?;
        v.update(|data| {
            let before = data.environments.len();
            data.environments.retain(|e| e.id != id);
            if before == data.environments.len() {
                return Err(OdxError::EnvironmentNotFound);
            }
            // Recreating an environment name inherits neither grants nor rules.
            data.rules.retain(|r| r.environment_id != id);
            for grant in &mut data.clients {
                // Keep the token revision unchanged to preserve other connections.
                grant.environment_ids.retain(|e| e != id);
            }
            Ok(())
        })?;
        self.environment_changed(s, &[id.to_owned()]).await;
        Ok(())
    }
    pub async fn set_mode(&self, id: &str, mode: WriteMode) -> Result<()> {
        if mode == WriteMode::Temporary {
            return Err(OdxError::validation(
                "validation.environment.temporary_deadline_required",
                "temporary writes require a future deadline",
            ));
        }
        self.configure_mode(id, mode, None).await
    }
    pub async fn set_temporary_write(&self, id: &str, minutes: u32) -> Result<()> {
        if minutes == 0 {
            return Err(OdxError::validation(
                "validation.environment.duration_positive",
                "duration must be positive",
            ));
        }
        self.configure_mode(id, WriteMode::Temporary, Some(minutes))
            .await
    }
    /// Measure temporary access with the nondecreasing expiry clock.
    async fn configure_mode(&self, id: &str, mode: WriteMode, minutes: Option<u32>) -> Result<()> {
        let mut s = self.inner.lock().await;
        let v = s.vault_mut()?;
        let until = match minutes {
            Some(minutes) => Some(
                v.now()
                    .checked_add_signed(chrono::Duration::minutes(i64::from(minutes)))
                    .ok_or_else(|| {
                        OdxError::validation(
                            "validation.environment.duration_out_of_range",
                            "duration out of range",
                        )
                    })?,
            ),
            None => None,
        };
        v.update(|data| {
            let env = data.environment_mut(id)?;
            env.write_mode = mode;
            env.write_until = until;
            env.revision += 1;
            Ok(())
        })?;
        self.environment_changed(s, &[id.to_owned()]).await;
        Ok(())
    }
    /// After environment changes, revoke and record unsent requests and invalidate cached authentication.
    pub(crate) async fn environment_changed(
        &self,
        mut s: tokio::sync::MutexGuard<'_, Inner>,
        ids: &[String],
    ) {
        let revoked = ids
            .iter()
            .flat_map(|id| revoke_environment(&mut s.pending, id))
            .collect();
        let ended = s.end(revoked, "revoked");
        drop(s);
        self.record(ended).await;
        for id in ids {
            self.odoo.invalidate_environment(id).await;
        }
    }
    /// Check reachability (`common.version`) and credentials (revision-cached login), without accessing business data.
    pub async fn check_environment(&self, id: &str) -> Result<HealthReport> {
        let (env, session) = {
            let s = self.inner.lock().await;
            let v = s.vault()?;
            let env = v.data().connection_environment(id)?;
            (env, s.session)
        };
        let started = std::time::Instant::now();
        let resolved = self.resolve_protocol(env.clone(), session).await;
        let permits = self.limits.acquire(&env.id).await?;
        self.assert_environment_current(&env.id, env.revision, session)
            .await?;
        let report = self.health(&env, resolved, started).await;
        drop(permits);
        Ok(report)
    }
    /// Check an unsaved draft like a new environment (global headers and proxy) without persisting anything.
    pub async fn check_draft(&self, draft: EnvironmentDraft) -> Result<HealthReport> {
        self.check_draft_with_proxy(draft, crate::proxy::EnvironmentProxyInput::Inherit)
            .await
    }
    /// Check a draft through the proxy the new environment will use.
    pub async fn check_draft_with_proxy(
        &self,
        draft: EnvironmentDraft,
        proxy: crate::proxy::EnvironmentProxyInput,
    ) -> Result<HealthReport> {
        let mut env = draft.into_environment()?;
        {
            let s = self.inner.lock().await;
            env.proxy = s.vault()?.data().chosen_proxy(proxy)?;
        }
        let env = self.draft_connection(env).await?;
        let started = std::time::Instant::now();
        let permits = self.limits.acquire(DRAFT_SLOTS).await?;
        let resolved = match env.protocol {
            crate::config::Protocol::Auto => self
                .odoo
                .detect_protocol(&env, || async { Ok(()) })
                .await
                .map(|protocol| Environment {
                    protocol,
                    ..env.clone()
                }),
            _ => Ok(env.clone()),
        };
        let report = self.health(&env, resolved, started).await;
        drop(permits);
        self.odoo.invalidate_environment(&env.id).await;
        Ok(report)
    }
    /// Databases listed by an Odoo server, unauthenticated; servers with `list_db = False` refuse.
    pub async fn list_databases(&self, url: &str) -> Result<Vec<String>> {
        crate::config::validate_url(url)?;
        let env = self
            .draft_connection(Environment {
                id: Uuid::new_v4().to_string(),
                url: url.trim_end_matches('/').into(),
                ..Default::default()
            })
            .await?;
        let _permits = self.limits.acquire(DRAFT_SLOTS).await?;
        self.odoo.list_databases(&env).await
    }
    /// Apply global headers and the inherited proxy to an environment that is not in the vault.
    async fn draft_connection(&self, mut env: Environment) -> Result<Environment> {
        let s = self.inner.lock().await;
        let data = s.vault()?.data();
        env.headers = crate::headers::merge(&data.global_headers, &[]);
        env.proxy = data.effective_proxy(&env);
        Ok(env)
    }
    async fn health(
        &self,
        env: &Environment,
        resolved: Result<Environment>,
        started: std::time::Instant,
    ) -> HealthReport {
        let version = match &resolved {
            Ok(env) => self.odoo.version(env).await,
            Err(_) => Ok(None),
        };
        let latency_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
        let protocol = resolved.as_ref().ok().map(|e| e.protocol);
        let report = |status: &str, server_version, error: Option<OdxError>| HealthReport {
            environment_id: env.id.clone(),
            status: status.into(),
            server_version,
            protocol,
            latency_ms,
            http_status: match &error {
                Some(OdxError::HttpStatus { status }) => Some(*status),
                _ => None,
            },
            network_error: match &error {
                Some(OdxError::Network { kind, .. }) => Some((*kind).into()),
                _ => None,
            },
            error: error
                .as_ref()
                .and_then(OdxError::safe_detail)
                .map(str::to_owned),
            error_code: error.as_ref().map(|e| e.code().to_owned()),
            error_params: error.as_ref().map(OdxError::ui_params),
        };
        match resolved {
            Err(error @ OdxError::AuthenticationRejected) => {
                report("auth_failed", None, Some(error))
            }
            Err(error) => report("unreachable", None, Some(error)),
            Ok(env) => match version {
                Err(error) => report("unreachable", None, Some(error)),
                Ok(server_version) => match self.odoo.authenticate(&env).await {
                    Ok(_) => report("ok", server_version, None),
                    Err(error) => report("auth_failed", server_version, Some(error)),
                },
            },
        }
    }
    /// Resolve transport before argument validation. Persist configuration only, without changing revision or permissions.
    pub(crate) async fn resolve_protocol(
        &self,
        mut env: Environment,
        session: u64,
    ) -> Result<Environment> {
        if env.protocol != crate::config::Protocol::Auto {
            return Ok(env);
        }
        let _permit = self.limits.acquire(&env.id).await?;
        let protocol = self
            .odoo
            .detect_protocol(&env, || {
                self.assert_environment_current(&env.id, env.revision, session)
            })
            .await?;
        self.assert_environment_current(&env.id, env.revision, session)
            .await?;
        let mut state = self.inner.lock().await;
        if state.session != session {
            return Err(OdxError::ApprovalUnavailable);
        }
        let vault = state.vault_mut()?;
        let current = vault.data().environment(&env.id)?;
        if current.revision != env.revision {
            return Err(OdxError::ApprovalUnavailable);
        }
        if current.detected_protocol != Some(protocol) {
            vault.update(|data| {
                let stored = data.environment_mut(&env.id)?;
                stored.detected_protocol = Some(protocol);
                Ok(())
            })?;
        }
        env.protocol = protocol;
        env.detected_protocol = Some(protocol);
        Ok(env)
    }
}
