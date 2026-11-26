//! Saved read-only Odoo views (SQL-view style queries) and optional local scheduled monitors.
use crate::{
    error::{OdxError, Result},
    execution::Core,
    shell::ShellCall,
    tools::args::valid_model,
};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use uuid::Uuid;

pub const MAX_VIEWS: usize = 200;
pub const MAX_VIEW_NAME: usize = 120;
pub const MAX_VIEW_DESCRIPTION: usize = 300;
pub const MAX_VIEW_LIMIT: u32 = 500;
pub const MIN_INTERVAL_MINUTES: u32 = 5;
pub const MAX_INTERVAL_MINUTES: u32 = 10_080; // 7 days

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AlertCondition {
    AnyRows,
    CountAbove { threshold: u64 },
    Changed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewRunSummary {
    pub environment_id: String,
    pub environment_name: String,
    pub at: DateTime<Utc>,
    pub count: u64,
    pub triggered: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewSchedule {
    pub enabled: bool,
    #[serde(default)]
    pub environment_ids: Vec<String>,
    pub interval_minutes: u32,
    pub alert_on: AlertCondition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub last_runs: Vec<ViewRunSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedView {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub model: String,
    #[serde(default = "empty_array")]
    pub domain: Value,
    #[serde(default)]
    pub fields: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule: Option<ViewSchedule>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ViewScheduleDraft {
    pub enabled: bool,
    #[serde(default)]
    pub environment_ids: Vec<String>,
    pub interval_minutes: u32,
    pub alert_on: AlertCondition,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SavedViewDraft {
    #[serde(default)]
    pub id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub model: String,
    #[serde(default = "empty_array")]
    pub domain: Value,
    #[serde(default)]
    pub fields: Vec<String>,
    #[serde(default)]
    pub order: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: u32,
    #[serde(default)]
    pub schedule: Option<ViewScheduleDraft>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ViewRunResult {
    pub view_id: String,
    pub environment_id: String,
    pub environment_name: String,
    pub rows: Vec<Value>,
    pub total_count: u64,
    pub duration_ms: u64,
    pub at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ViewAlert {
    pub view_id: String,
    pub view_name: String,
    pub environment_id: String,
    pub environment_name: String,
    pub count: u64,
    pub error: Option<String>,
}

fn empty_array() -> Value {
    Value::Array(Vec::new())
}

fn default_limit() -> u32 {
    50
}

fn valid_field(field: &str) -> bool {
    !field.is_empty()
        && field.len() <= 128
        && field
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

impl SavedViewDraft {
    pub fn validate(&self) -> Result<()> {
        let name = self.name.trim();
        if name.is_empty() || name.chars().count() > MAX_VIEW_NAME {
            return Err(OdxError::validation(
                "validation.view.name_invalid",
                "view name must be 1 to 120 characters",
            ));
        }
        if self.description.trim().chars().count() > MAX_VIEW_DESCRIPTION {
            return Err(OdxError::validation(
                "validation.view.description_too_long",
                "view description is limited to 300 characters",
            ));
        }
        if !valid_model(self.model.trim()) {
            return Err(OdxError::validation(
                "validation.view.model_invalid",
                "invalid Odoo model name",
            ));
        }
        if !self.domain.is_array() {
            return Err(OdxError::validation(
                "validation.view.domain_invalid",
                "domain must be a JSON array",
            ));
        }
        if self.fields.iter().any(|f| !valid_field(f.trim())) {
            return Err(OdxError::validation(
                "validation.view.fields_invalid",
                "invalid field name",
            ));
        }
        if self.limit < 1 || self.limit > MAX_VIEW_LIMIT {
            return Err(OdxError::validation(
                "validation.view.limit_invalid",
                "limit must be between 1 and 500",
            ));
        }
        if let Some(schedule) = &self.schedule {
            if schedule.interval_minutes < MIN_INTERVAL_MINUTES
                || schedule.interval_minutes > MAX_INTERVAL_MINUTES
            {
                return Err(OdxError::validation(
                    "validation.view.interval_invalid",
                    "schedule interval must be between 5 minutes and 7 days",
                ));
            }
            if schedule.enabled && schedule.environment_ids.is_empty() {
                return Err(OdxError::validation(
                    "validation.view.schedule_environments_required",
                    "select at least one environment for an enabled monitor",
                ));
            }
        }
        Ok(())
    }
}

/// Replace `{{today}}`, `{{now}}`, `{{days_ago:N}}`, and `{{hours_ago:N}}` inside domain strings.
pub fn expand_domain_placeholders(value: &Value, now: DateTime<Utc>) -> Value {
    match value {
        Value::String(s) => Value::String(expand_string(s, now)),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|v| expand_domain_placeholders(v, now))
                .collect(),
        ),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), expand_domain_placeholders(v, now)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn expand_string(input: &str, now: DateTime<Utc>) -> String {
    if !input.contains("{{") {
        return input.to_owned();
    }
    let mut out = input
        .replace("{{today}}", &now.format("%Y-%m-%d").to_string())
        .replace("{{now}}", &now.format("%Y-%m-%d %H:%M:%S").to_string());
    // Expand {{days_ago:N}} and {{hours_ago:N}}
    for (prefix, unit_secs, fmt) in [
        ("{{days_ago:", 86_400_i64, "%Y-%m-%d"),
        ("{{hours_ago:", 3_600_i64, "%Y-%m-%d %H:%M:%S"),
    ] {
        while let Some(start) = out.find(prefix) {
            let rest = &out[start + prefix.len()..];
            let Some(end) = rest.find("}}") else {
                break;
            };
            let raw_n = rest[..end].trim();
            let Ok(n) = raw_n.parse::<i64>() else {
                break;
            };
            let clamped = n.clamp(0, 3650);
            let ts = now - Duration::seconds(clamped.saturating_mul(unit_secs));
            let formatted = ts.format(fmt).to_string();
            out.replace_range(start..start + prefix.len() + end + 2, &formatted);
        }
    }
    out
}

pub fn should_alert(condition: &AlertCondition, count: u64, previous_count: Option<u64>) -> bool {
    match condition {
        AlertCondition::AnyRows => count > 0,
        AlertCondition::CountAbove { threshold } => count > *threshold,
        AlertCondition::Changed => previous_count.is_some_and(|prev| prev != count),
    }
}

impl Core {
    pub async fn views(&self) -> Result<Vec<SavedView>> {
        let s = self.inner.lock().await;
        Ok(s.vault()?.data().views.clone())
    }

    pub async fn save_view(&self, draft: SavedViewDraft) -> Result<SavedView> {
        draft.validate()?;
        let mut s = self.inner.lock().await;
        let v = s.vault_mut()?;
        let now = Utc::now();
        let fields: Vec<String> = draft
            .fields
            .iter()
            .map(|f| f.trim().to_owned())
            .filter(|f| !f.is_empty())
            .collect();
        let order = draft
            .order
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        let mut saved = None;
        v.update(|data| {
            let existing = draft
                .id
                .as_deref()
                .and_then(|id| data.views.iter().position(|item| item.id == id));
            if existing.is_none() && data.views.len() >= MAX_VIEWS {
                return Err(OdxError::validation(
                    "validation.view.limit_reached",
                    "maximum number of saved views reached",
                ));
            }
            let prev_schedule = existing.and_then(|i| data.views[i].schedule.clone());
            let schedule = draft.schedule.as_ref().map(|s| ViewSchedule {
                enabled: s.enabled,
                environment_ids: s.environment_ids.clone(),
                interval_minutes: s.interval_minutes,
                alert_on: s.alert_on.clone(),
                last_run_at: prev_schedule.as_ref().and_then(|p| p.last_run_at),
                last_runs: prev_schedule.map(|p| p.last_runs).unwrap_or_default(),
            });
            let view = SavedView {
                id: draft
                    .id
                    .clone()
                    .unwrap_or_else(|| Uuid::new_v4().to_string()),
                name: draft.name.trim().to_owned(),
                description: draft.description.trim().to_owned(),
                model: draft.model.trim().to_owned(),
                domain: draft.domain.clone(),
                fields: fields.clone(),
                order: order.clone(),
                limit: draft.limit,
                schedule,
                created_at: existing.map(|i| data.views[i].created_at).unwrap_or(now),
                updated_at: now,
            };
            if let Some(i) = existing {
                data.views[i] = view.clone();
            } else {
                data.views.push(view.clone());
            }
            saved = Some(view);
            Ok(())
        })?;
        saved.ok_or(OdxError::Persistence)
    }

    pub async fn delete_view(&self, id: &str) -> Result<()> {
        let mut s = self.inner.lock().await;
        let v = s.vault_mut()?;
        v.update(|data| {
            data.views.retain(|item| item.id != id);
            Ok(())
        })
    }

    /// Execute a saved view in read-only mode against one environment.
    pub async fn run_view(&self, view_id: &str, environment_id: &str) -> Result<ViewRunResult> {
        let (view, env_name) = {
            let s = self.inner.lock().await;
            let data = s.vault()?.data();
            let view = data
                .views
                .iter()
                .find(|v| v.id == view_id)
                .cloned()
                .ok_or_else(|| OdxError::Invalid("saved view not found".into()))?;
            let env = data.environment(environment_id)?;
            (view, env.name.clone())
        };
        let now = Utc::now();
        let domain = expand_domain_placeholders(&view.domain, now);
        let mut kwargs = Map::new();
        if !view.fields.is_empty() {
            kwargs.insert(
                "fields".into(),
                Value::Array(view.fields.iter().cloned().map(Value::String).collect()),
            );
        }
        kwargs.insert("limit".into(), Value::from(view.limit));
        if let Some(order) = &view.order {
            kwargs.insert("order".into(), Value::String(order.clone()));
        }
        let read = self
            .shell_call(ShellCall {
                environment_id: environment_id.to_owned(),
                model: view.model.clone(),
                method: "search_read".into(),
                args: vec![domain.clone()],
                kwargs,
                context: Map::new(),
                confirmed: false,
                client_id: None,
                client_revision: None,
            })
            .await
            .map_err(|e| OdxError::Invalid(e.message))?;
        let rows = read.data.as_array().cloned().unwrap_or_default();
        let mut duration_ms = read.upstream_ms;
        let total_count = if rows.len() as u32 >= view.limit {
            match self
                .shell_call(ShellCall {
                    environment_id: environment_id.to_owned(),
                    model: view.model.clone(),
                    method: "search_count".into(),
                    args: vec![domain],
                    kwargs: Map::new(),
                    context: Map::new(),
                    confirmed: false,
                    client_id: None,
                    client_revision: None,
                })
                .await
            {
                Ok(counted) => {
                    duration_ms = duration_ms.saturating_add(counted.upstream_ms);
                    counted.data.as_u64().unwrap_or(rows.len() as u64)
                }
                Err(_) => rows.len() as u64,
            }
        } else {
            rows.len() as u64
        };
        Ok(ViewRunResult {
            view_id: view.id,
            environment_id: environment_id.to_owned(),
            environment_name: env_name,
            rows,
            total_count,
            duration_ms,
            at: now,
        })
    }

    /// Run all enabled scheduled views that are due, persist summaries, and return triggered alerts.
    pub async fn run_due_views(&self) -> Result<Vec<ViewAlert>> {
        let now = Utc::now();
        let due: Vec<(SavedView, Vec<(String, String)>)> = {
            let s = self.inner.lock().await;
            let Ok(v) = s.vault() else {
                return Ok(Vec::new());
            };
            let data = v.data();
            data.views
                .iter()
                .filter_map(|view| {
                    let sched = view.schedule.as_ref()?;
                    if !sched.enabled || sched.environment_ids.is_empty() {
                        return None;
                    }
                    let is_due = sched.last_run_at.is_none_or(|last| {
                        now - last >= Duration::minutes(i64::from(sched.interval_minutes))
                    });
                    if !is_due {
                        return None;
                    }
                    let targets: Vec<(String, String)> = sched
                        .environment_ids
                        .iter()
                        .filter_map(|id| {
                            data.environments
                                .iter()
                                .find(|e| &e.id == id)
                                .map(|e| (e.id.clone(), e.name.clone()))
                        })
                        .collect();
                    if targets.is_empty() {
                        None
                    } else {
                        Some((view.clone(), targets))
                    }
                })
                .collect()
        };

        let mut alerts = Vec::new();
        for (view, targets) in due {
            let Some(sched) = &view.schedule else {
                continue;
            };
            let mut summaries = Vec::with_capacity(targets.len());
            for (env_id, env_name) in targets {
                let prev = sched
                    .last_runs
                    .iter()
                    .find(|r| r.environment_id == env_id && r.error.is_none())
                    .map(|r| r.count);
                match self.run_view(&view.id, &env_id).await {
                    Ok(res) => {
                        let triggered = should_alert(&sched.alert_on, res.total_count, prev);
                        if triggered {
                            alerts.push(ViewAlert {
                                view_id: view.id.clone(),
                                view_name: view.name.clone(),
                                environment_id: env_id.clone(),
                                environment_name: env_name.clone(),
                                count: res.total_count,
                                error: None,
                            });
                        }
                        summaries.push(ViewRunSummary {
                            environment_id: env_id,
                            environment_name: env_name,
                            at: res.at,
                            count: res.total_count,
                            triggered,
                            error: None,
                            duration_ms: res.duration_ms,
                        });
                    }
                    Err(err) => {
                        let msg = err.to_string();
                        alerts.push(ViewAlert {
                            view_id: view.id.clone(),
                            view_name: view.name.clone(),
                            environment_id: env_id.clone(),
                            environment_name: env_name.clone(),
                            count: 0,
                            error: Some(msg.clone()),
                        });
                        summaries.push(ViewRunSummary {
                            environment_id: env_id,
                            environment_name: env_name,
                            at: Utc::now(),
                            count: 0,
                            triggered: true,
                            error: Some(msg),
                            duration_ms: 0,
                        });
                    }
                }
            }
            let mut s = self.inner.lock().await;
            if let Ok(v) = s.vault_mut() {
                let _ = v.update(|data| {
                    if let Some(target) = data.views.iter_mut().find(|v| v.id == view.id)
                        && let Some(target_sched) = target.schedule.as_mut()
                    {
                        target_sched.last_run_at = Some(now);
                        target_sched.last_runs = summaries.clone();
                    }
                    Ok(())
                });
            }
        }
        Ok(alerts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use serde_json::json;

    #[test]
    fn expands_date_and_time_placeholders_in_domains() {
        let now = Utc.with_ymd_and_hms(2026, 11, 26, 14, 30, 0).unwrap();
        let domain = json!([
            ["create_date", ">=", "{{days_ago:7}}"],
            ["write_date", "<=", "{{hours_ago:2}}"],
            ["date", "=", "{{today}}"],
            ["state", "=", "draft"]
        ]);
        let expanded = expand_domain_placeholders(&domain, now);
        assert_eq!(
            expanded,
            json!([
                ["create_date", ">=", "2026-11-19"],
                ["write_date", "<=", "2026-11-26 12:30:00"],
                ["date", "=", "2026-11-26"],
                ["state", "=", "draft"]
            ])
        );
    }

    #[test]
    fn alert_conditions_evaluate_accurately() {
        assert!(!should_alert(&AlertCondition::AnyRows, 0, None));
        assert!(should_alert(&AlertCondition::AnyRows, 1, None));
        assert!(!should_alert(
            &AlertCondition::CountAbove { threshold: 5 },
            5,
            None
        ));
        assert!(should_alert(
            &AlertCondition::CountAbove { threshold: 5 },
            6,
            None
        ));
        assert!(!should_alert(&AlertCondition::Changed, 3, None));
        assert!(!should_alert(&AlertCondition::Changed, 3, Some(3)));
        assert!(should_alert(&AlertCondition::Changed, 4, Some(3)));
    }

    #[tokio::test]
    async fn saved_views_persist_across_vault_reopen() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("vault.sqlite3");
        let core = Core::new(path.clone()).unwrap();
        core.create("fixture password").await.unwrap();
        let created = core
            .save_view(SavedViewDraft {
                id: None,
                name: "Failed emails".into(),
                description: "Outgoing emails in exception".into(),
                model: "mail.mail".into(),
                domain: json!([["state", "=", "exception"]]),
                fields: vec!["id".into(), "subject".into(), "date".into()],
                order: Some("date desc".into()),
                limit: 50,
                schedule: Some(ViewScheduleDraft {
                    enabled: true,
                    environment_ids: vec!["env-1".into()],
                    interval_minutes: 15,
                    alert_on: AlertCondition::AnyRows,
                }),
            })
            .await
            .unwrap();
        assert_eq!(created.name, "Failed emails");
        core.lock().await;
        drop(core);

        let reopened = Core::new(path).unwrap();
        reopened.unlock("fixture password").await.unwrap();
        let views = reopened.views().await.unwrap();
        assert_eq!(views.len(), 1);
        assert_eq!(views[0].id, created.id);
        assert_eq!(views[0].model, "mail.mail");
        assert!(views[0].schedule.as_ref().unwrap().enabled);
        reopened.delete_view(&created.id).await.unwrap();
        assert!(reopened.views().await.unwrap().is_empty());
    }
}
