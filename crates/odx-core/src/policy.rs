use crate::{
    config::{Environment, WriteMode},
    error::{OdxError, Result},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    Read,
    Write,
    Delete,
}

/// Ok(true) requires human/rule approval; Err denies. Use the vault's nondecreasing expiry clock.
pub fn authorize(env: &Environment, effect: Effect, now: DateTime<Utc>) -> Result<bool> {
    match (effect, env.effective_write_mode(now)) {
        (Effect::Read, _) => Ok(false),
        (Effect::Write | Effect::Delete, WriteMode::ReadOnly) => Err(OdxError::Denied),
        (Effect::Delete, _) | (Effect::Write, WriteMode::Approval) => Ok(true),
        (Effect::Write, WriteMode::Temporary | WriteMode::Permanent) => Ok(false),
    }
}

/// Write authorization source stored in requests and audit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Authorization {
    /// Human decision for this specific request.
    Individual,
    /// Automatic approval rule and the revision used to authorize.
    Rule { rule_id: String, rule_revision: u64 },
    /// Environment's temporary or permanent free-write mode.
    Free,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleKind {
    /// Any method arguments with a known record count.
    Method,
    /// create/write values restricted to these fields.
    Fields,
    /// create/write values matching these exact values.
    Values,
}

/// Automatic approval rule in approval mode only. Never authorizes unlink or relational deletes; those always require individual approval.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rule {
    pub id: String,
    pub environment_id: String,
    pub model: String,
    pub method: String,
    pub kind: RuleKind,
    /// Fields rules only: sorted unique allowed fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fields: Option<Vec<String>>,
    /// Values rules only: exact values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub values: Option<Map<String, Value>>,
    /// Exact effective context match: global < environment < call.
    pub context: Map<String, Value>,
    /// Maximum records per call; the count must be known.
    pub max_records: u32,
    /// No deadline means valid until deleted.
    pub expires_at: Option<DateTime<Utc>>,
    /// Increment on edits; requests authorized under older revisions cannot dispatch.
    pub revision: u64,
    pub created_at: DateTime<Utc>,
}

/// Local admin rule creation (no id) or update.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleDraft {
    #[serde(default)]
    pub id: Option<String>,
    pub environment_id: String,
    pub model: String,
    pub method: String,
    pub kind: RuleKind,
    #[serde(default)]
    pub fields: Option<Vec<String>>,
    #[serde(default)]
    pub values: Option<Value>,
    pub context: Value,
    pub max_records: u64,
    #[serde(default)]
    pub expires_at: Option<DateTime<Utc>>,
}

/// Admin rule view with validity based on the vault clock.
#[derive(Debug, Clone, Serialize)]
pub struct RuleView {
    #[serde(flatten)]
    pub rule: Rule,
    pub expired: bool,
}

/// Rule record limit matches the per-call ID limit.
pub const MAX_RULE_RECORDS: u64 = 1000;

impl RuleDraft {
    /// Validate without broadening rules: reject unrepresentable conditions rather than ignoring fields.
    pub fn into_rule(self, now: DateTime<Utc>) -> Result<Rule> {
        let model_ok = crate::tools::catalog::is_identifier(&self.model, true)
            && !self.model.contains("..")
            && !self.model.ends_with('.');
        if !model_ok {
            return Err(OdxError::validation(
                "validation.rule.model_invalid",
                "invalid model",
            ));
        }
        if !crate::tools::catalog::is_identifier(&self.method, false)
            || self.method.starts_with('_')
        {
            return Err(OdxError::validation(
                "validation.rule.method_invalid",
                "invalid method",
            ));
        }
        if self.method == "unlink" {
            return Err(OdxError::validation(
                "validation.rule.unlink_not_allowed",
                "unlink always requires individual approval; rules cannot authorize it",
            ));
        }
        if is_credential_call(&self.model, &self.method) {
            return Err(OdxError::validation(
                "validation.rule.credential_method_not_allowed",
                "methods generating or exposing API keys are always denied",
            ));
        }
        if crate::tools::catalog::READ_METHODS.contains(&self.method.as_str()) {
            return Err(OdxError::validation(
                "validation.rule.read_method_not_needed",
                "reads do not need approval rules",
            ));
        }
        let payload_method = matches!(self.method.as_str(), "create" | "write");
        let (fields, values) = match (self.kind, self.fields, self.values) {
            (RuleKind::Method, None, None) => (None, None),
            (RuleKind::Fields, Some(fields), None) if payload_method => {
                if fields.is_empty() || fields.len() > 1000 {
                    return Err(OdxError::validation(
                        "validation.rule.fields_count",
                        "field rules require 1–1000 fields",
                    ));
                }
                if fields
                    .iter()
                    .any(|f| !crate::tools::catalog::is_identifier(f, false))
                {
                    return Err(OdxError::validation(
                        "validation.rule.field_invalid",
                        "invalid field name",
                    ));
                }
                let mut sorted = fields.clone();
                sorted.sort();
                sorted.dedup();
                if sorted.len() != fields.len() {
                    return Err(OdxError::validation(
                        "validation.rule.duplicate_fields",
                        "duplicate fields",
                    ));
                }
                (Some(sorted), None)
            }
            (RuleKind::Values, None, Some(Value::Object(values))) if payload_method => {
                if values.is_empty() {
                    return Err(OdxError::validation(
                        "validation.rule.values_required",
                        "value rules require at least one value",
                    ));
                }
                if crate::tools::mutations::has_relational_delete(&Value::Object(values.clone())) {
                    return Err(OdxError::validation(
                        "validation.rule.relational_delete_not_allowed",
                        "relational deletes require individual approval",
                    ));
                }
                (None, Some(values))
            }
            (RuleKind::Fields | RuleKind::Values, _, _) if !payload_method => {
                return Err(OdxError::validation(
                    "validation.rule.kind_requires_write_method",
                    "field/value rules support only create and write",
                ));
            }
            _ => {
                return Err(OdxError::validation(
                    "validation.rule.shape_invalid",
                    "method rules have no fields/values; fields rules need a field list; values rules need an object",
                ));
            }
        };
        let Value::Object(context) = self.context else {
            return Err(OdxError::validation(
                "validation.rule.context_object_required",
                "context must contain the complete effective context object",
            ));
        };
        if self.max_records == 0 || self.max_records > MAX_RULE_RECORDS {
            return Err(OdxError::validation(
                "validation.rule.max_records",
                "max_records must be between 1 and 1000",
            ));
        }
        if self.expires_at.is_some_and(|at| at <= now) {
            return Err(OdxError::validation(
                "validation.rule.expiry_future",
                "expiry must be in the future",
            ));
        }
        Ok(Rule {
            id: self.id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            environment_id: self.environment_id,
            model: self.model,
            method: self.method,
            kind: self.kind,
            fields,
            values,
            context,
            max_records: self.max_records as u32,
            expires_at: self.expires_at,
            revision: 1,
            created_at: now,
        })
    }
}

/// Validated effective call; kwargs.context contains the merged context.
pub struct EffectiveCall<'a> {
    pub environment_id: &'a str,
    pub model: &'a str,
    pub method: &'a str,
    pub args: &'a [Value],
    pub kwargs: &'a Map<String, Value>,
}

impl Rule {
    pub fn is_expired(&self, now: DateTime<Utc>) -> bool {
        self.expires_at.is_some_and(|at| at <= now)
    }
    /// Authorize only when record count, values and context are known; otherwise require individual approval.
    pub fn matches(&self, call: &EffectiveCall, effect: &Effect, now: DateTime<Utc>) -> bool {
        if *effect != Effect::Write
            || call.method == "unlink"
            || self.environment_id != call.environment_id
            || self.model != call.model
            || self.method != call.method
            || self.is_expired(now)
        {
            return false;
        }
        let context_matches = match call.kwargs.get("context") {
            Some(Value::Object(context)) => *context == self.context,
            None => self.context.is_empty(),
            Some(_) => false,
        };
        if !context_matches
            || !record_count(call).is_some_and(|n| n > 0 && n <= self.max_records as usize)
        {
            return false;
        }
        match self.kind {
            RuleKind::Method => true,
            RuleKind::Fields => {
                let Some(fields) = &self.fields else {
                    return false;
                };
                // Command lists write comodel records outside the field list and max_records.
                payloads(call).is_some_and(|list| {
                    list.iter().all(|p| {
                        p.iter().all(|(k, v)| {
                            fields.binary_search(k).is_ok()
                                && !crate::tools::mutations::is_command_list(v)
                        })
                    })
                })
            }
            RuleKind::Values => {
                let Some(values) = &self.values else {
                    return false;
                };
                payloads(call).is_some_and(|list| list.iter().all(|p| *p == values))
            }
        }
    }
}

/// First rule authorizing the call.
pub fn matching_rule<'r>(
    rules: &'r [Rule],
    call: &EffectiveCall,
    effect: &Effect,
    now: DateTime<Utc>,
) -> Option<&'r Rule> {
    rules.iter().find(|r| r.matches(call, effect, now))
}

/// Positional or named parameter; None if missing or supplied both ways.
fn parameter<'a>(call: &EffectiveCall<'a>, index: usize, name: &str) -> Option<&'a Value> {
    match (call.args.get(index), call.kwargs.get(name)) {
        (Some(value), None) | (None, Some(value)) => Some(value),
        _ => None,
    }
}

/// Explicit IDs: nonempty unique positive integers.
fn explicit_ids(value: &Value) -> Option<usize> {
    let ids = value.as_array()?;
    let mut seen = std::collections::HashSet::new();
    let valid = !ids.is_empty()
        && ids
            .iter()
            .all(|id| id.as_u64().is_some_and(|id| id > 0) && seen.insert(id.as_u64()));
    valid.then_some(ids.len())
}

/// Count create values or the first positional/named ids list for other methods. Does not bound internal effects of custom methods.
fn record_count(call: &EffectiveCall) -> Option<usize> {
    if call.method == "create" {
        return match parameter(call, 0, "vals_list")? {
            Value::Object(_) => Some(1),
            Value::Array(list) => Some(list.len()),
            _ => None,
        };
    }
    explicit_ids(parameter(call, 0, "ids")?)
}

/// Nonempty create/write values only for exact known signatures.
fn payloads<'a>(call: &EffectiveCall<'a>) -> Option<Vec<&'a Map<String, Value>>> {
    let (index, name, known): (usize, &str, &[&str]) = match call.method {
        "create" => (0, "vals_list", &["vals_list", "context"]),
        "write" => (1, "vals", &["ids", "vals", "context"]),
        _ => return None,
    };
    if call.args.len() > index + 1 || call.kwargs.keys().any(|k| !known.contains(&k.as_str())) {
        return None;
    }
    let list = match parameter(call, index, name)? {
        Value::Object(values) => vec![values],
        Value::Array(list) if call.method == "create" => list
            .iter()
            .map(Value::as_object)
            .collect::<Option<Vec<_>>>()?,
        _ => return None,
    };
    (!list.is_empty() && list.iter().all(|values| !values.is_empty())).then_some(list)
}

/// Deny calls that generate or expose Odoo API keys in every mode, without approval. Private methods are already denied by their underscore prefix. Source: Odoo 16–19 odoo/addons/base/models/res_users.py.
pub fn is_credential_call(model: &str, method: &str) -> bool {
    matches!(
        model,
        // make_key wizard returns the new key in action context.
        "res.users.apikeys.description"
        // Abstract model whose key field exposes the newly created key.
        | "res.users.apikeys.show"
    ) || matches!(
        (model, method),
        // Odoo 19 generates and returns a new key.
        ("res.users.apikeys", "generate")
        // Opens the key wizard.
        | ("res.users", "api_key_wizard")
    )
}

/// Maximum stored rules.
const MAX_RULES: usize = 1000;

impl crate::execution::Core {
    /// Stored rules for one or all environments with current validity.
    pub async fn rules(&self, environment_id: Option<&str>) -> Result<Vec<RuleView>> {
        let s = self.inner.lock().await;
        let v = s.vault.as_ref().ok_or(OdxError::Locked)?;
        let now = v.now();
        Ok(v.data()
            .rules
            .iter()
            .filter(|r| environment_id.is_none_or(|id| r.environment_id == id))
            .map(|rule| RuleView {
                rule: rule.clone(),
                expired: rule.is_expired(now),
            })
            .collect())
    }
    /// Validate creates/updates; editing increments revision and prevents dispatch under older authorizations.
    pub async fn save_rule(&self, draft: RuleDraft) -> Result<RuleView> {
        let mut s = self.inner.lock().await;
        let v = s.vault.as_mut().ok_or(OdxError::Locked)?;
        let now = v.now();
        if !v
            .data()
            .environments
            .iter()
            .any(|e| e.id == draft.environment_id)
        {
            return Err(OdxError::EnvironmentNotFound);
        }
        let previous = match &draft.id {
            Some(id) => Some(
                v.data()
                    .rules
                    .iter()
                    .find(|r| r.id == *id)
                    .cloned()
                    .ok_or_else(|| {
                        OdxError::validation("validation.rule.not_found", "rule not found")
                    })?,
            ),
            None if v.data().rules.len() >= MAX_RULES => {
                return Err(OdxError::validation_with_params(
                    "validation.rule.limit_reached",
                    serde_json::json!({ "limit": MAX_RULES }),
                    format!("maximum {MAX_RULES} rules"),
                ));
            }
            None => None,
        };
        let mut rule = draft.into_rule(now)?;
        if let Some(previous) = &previous {
            if previous.environment_id != rule.environment_id {
                return Err(OdxError::validation(
                    "validation.rule.environment_immutable",
                    "a rule cannot change environment; create another",
                ));
            }
            rule.revision = previous.revision + 1;
            rule.created_at = previous.created_at;
        }
        let saved = rule.clone();
        v.update(|data| {
            match data.rules.iter_mut().find(|r| r.id == saved.id) {
                Some(slot) => *slot = saved,
                None => data.rules.push(saved),
            }
            Ok(())
        })?;
        if previous.is_some() {
            revoke_rule(&mut s, &rule.id);
        }
        Ok(RuleView {
            expired: rule.is_expired(now),
            rule,
        })
    }
    /// Delete the rule and block its unsent authorizations.
    pub async fn delete_rule(&self, id: &str) -> Result<()> {
        let mut s = self.inner.lock().await;
        s.vault.as_mut().ok_or(OdxError::Locked)?.update(|data| {
            let before = data.rules.len();
            data.rules.retain(|r| r.id != id);
            if before == data.rules.len() {
                return Err(OdxError::validation(
                    "validation.rule.not_found",
                    "rule not found",
                ));
            }
            Ok(())
        })?;
        revoke_rule(&mut s, id);
        Ok(())
    }
}

/// Revoked rule-authorized requests close as not sent through dispatch; pre-send revalidation also enforces this.
fn revoke_rule(s: &mut crate::execution::Inner, id: &str) {
    for approval in s.pending.values_mut() {
        if matches!(&approval.authorization, Authorization::Rule { rule_id, .. } if rule_id == id) {
            approval.revoke();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_calls_cover_generation_and_display_only() {
        for (model, method) in [
            ("res.users.apikeys", "generate"),
            ("res.users.apikeys.description", "make_key"),
            ("res.users.apikeys.description", "create"),
            ("res.users.apikeys.show", "read"),
            ("res.users", "api_key_wizard"),
        ] {
            assert!(is_credential_call(model, method), "{model}.{method}");
        }
        for (model, method) in [
            ("res.users.apikeys", "search_read"),
            ("res.users", "read"),
            ("res.partner", "generate"),
        ] {
            assert!(!is_credential_call(model, method), "{model}.{method}");
        }
    }
    use chrono::Duration;

    #[test]
    fn free_writes_keep_individual_deletions_and_expired_grants_deny() {
        let now = Utc::now();
        let mut env = Environment::default();
        for mode in [WriteMode::Permanent, WriteMode::Temporary] {
            env.write_mode = mode;
            env.write_until = Some(now + Duration::minutes(1));
            assert!(!authorize(&env, Effect::Write, now).unwrap());
            assert!(authorize(&env, Effect::Delete, now).unwrap());
        }
        env.write_until = Some(now - Duration::seconds(1));
        assert!(authorize(&env, Effect::Write, now).is_err());
        assert!(authorize(&env, Effect::Delete, now).is_err());
        assert!(!authorize(&env, Effect::Read, now).unwrap());
        env.write_until = None;
        assert!(authorize(&env, Effect::Write, now).is_err());
        env.write_mode = WriteMode::Approval;
        assert!(authorize(&env, Effect::Write, now).unwrap());
        assert!(authorize(&env, Effect::Delete, now).unwrap());
    }

    #[test]
    fn fields_rules_never_match_x2many_commands() {
        let now = Utc::now();
        let rule = Rule {
            id: "r".into(),
            environment_id: "e".into(),
            model: "sale.order".into(),
            method: "write".into(),
            kind: RuleKind::Fields,
            fields: Some(vec!["note".into(), "order_line".into()]),
            values: None,
            context: Map::new(),
            max_records: 1,
            expires_at: None,
            revision: 1,
            created_at: now,
        };
        let kwargs = Map::new();
        let matches = |vals: Value| {
            let args = [serde_json::json!([7]), vals];
            let call = EffectiveCall {
                environment_id: "e",
                model: "sale.order",
                method: "write",
                args: &args,
                kwargs: &kwargs,
            };
            rule.matches(&call, &Effect::Write, now)
        };
        assert!(matches(serde_json::json!({"note": "x"})));
        assert!(!matches(
            serde_json::json!({"order_line": [[1, 777, {"price_unit": 0}]]})
        ));
        assert!(!matches(
            serde_json::json!({"order_line": [[0, 0, {"name": "x"}]]})
        ));
        assert!(!matches(serde_json::json!({"order_line": [[4, 9]]})));
    }
}
