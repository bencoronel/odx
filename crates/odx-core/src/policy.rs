use crate::{
    config::{Environment, WriteMode},
    error::{OdxError, Result},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
};

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

/// What a rule does. Missing values in encrypted vaults mean the legacy approval action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RuleAction {
    #[default]
    Approve,
    Block,
}

/// Rule scope. Environment rules retain the old environment_id field for vault compatibility.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RuleScope {
    #[default]
    Environment,
    Global,
}

const MAX_REGEX_BYTES: usize = 256;
const MAX_REGEX_SIZE: usize = 1 << 16;
static COMPILED_REGEXES: OnceLock<Mutex<HashMap<String, Option<Arc<regex::Regex>>>>> =
    OnceLock::new();

fn compiled_regex(pattern: &str) -> Option<Arc<regex::Regex>> {
    let cache = COMPILED_REGEXES.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(regex) = cache.lock().ok()?.get(pattern).cloned() {
        return regex;
    }
    let compiled = regex::RegexBuilder::new(pattern)
        .size_limit(MAX_REGEX_SIZE)
        .dfa_size_limit(MAX_REGEX_SIZE)
        .build()
        .ok()
        .map(Arc::new);
    if let Ok(mut cache) = cache.lock() {
        if cache.len() >= 1024 {
            cache.clear();
        }
        cache.insert(pattern.to_owned(), compiled.clone());
    }
    compiled
}

fn validate_selector(selector: &Selector) -> Result<()> {
    if let Selector::Regex { pattern } = selector
        && (pattern.len() > MAX_REGEX_BYTES || compiled_regex(pattern).is_none())
    {
        return Err(OdxError::validation(
            "validation.rule.regex_invalid",
            "invalid or oversized regular expression",
        ));
    }
    Ok(())
}

fn validate_model_selector(selector: &ModelSelector) -> Result<()> {
    if let ModelSelector::Regex { pattern } = selector {
        validate_selector(&Selector::Regex {
            pattern: pattern.clone(),
        })?;
    }
    Ok(())
}

/// Reject encrypted rules written by an older or externally modified vault when a persisted
/// selector cannot be compiled safely. Silently treating an invalid block regex as a miss would
/// turn a malformed policy into an access grant.
pub(crate) fn validate_persisted_rules(rules: &[Rule]) -> Result<()> {
    if rules.len() > MAX_RULES {
        return Err(OdxError::Persistence);
    }
    for rule in rules {
        if let Some(selector) = &rule.model_selector {
            validate_model_selector(selector).map_err(|_| OdxError::Persistence)?;
        }
        for selector in &rule.field_selectors {
            validate_selector(selector).map_err(|_| OdxError::Persistence)?;
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Selector {
    Exact { value: String },
    Regex { pattern: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ModelSelector {
    #[default]
    All,
    Exact {
        value: String,
    },
    Regex {
        pattern: String,
    },
}

/// Persisted approval or block rule. Approval rules never authorize unlink or relational deletes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rule {
    pub id: String,
    #[serde(default)]
    pub action: RuleAction,
    #[serde(default)]
    pub scope: RuleScope,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub model_selector: Option<ModelSelector>,
    #[serde(default)]
    pub operations: Vec<String>,
    #[serde(default)]
    pub field_selectors: Vec<Selector>,
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

/// Admin rule creation (no id) or update.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleDraft {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub action: RuleAction,
    #[serde(default)]
    pub scope: RuleScope,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub model_selector: Option<ModelSelector>,
    #[serde(default)]
    pub operations: Vec<String>,
    #[serde(default)]
    pub field_selectors: Vec<Selector>,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleTrace {
    pub rule_id: String,
    pub rule_revision: u64,
}

/// Rule record limit matches the per-call ID limit.
pub const MAX_RULE_RECORDS: u64 = 1000;

impl RuleDraft {
    /// Validate without broadening rules: reject unrepresentable conditions rather than ignoring fields.
    pub fn into_rule(self, now: DateTime<Utc>) -> Result<Rule> {
        if let Some(selector) = &self.model_selector {
            validate_model_selector(selector)?;
        }
        if let Some(ModelSelector::Exact { value }) = &self.model_selector
            && !crate::tools::args::valid_model(value)
        {
            return Err(OdxError::validation(
                "validation.rule.model_invalid",
                "invalid model selector",
            ));
        }
        for selector in &self.field_selectors {
            validate_selector(selector)?;
            if self.action == RuleAction::Approve && matches!(selector, Selector::Regex { .. }) {
                return Err(OdxError::validation(
                    "validation.rule.regex_invalid",
                    "approval rules do not support regular expression selectors",
                ));
            }
        }
        if self.name.len() > 120 {
            return Err(OdxError::validation(
                "validation.rule.name_invalid",
                "rule name is too long",
            ));
        }
        if self.action == RuleAction::Block
            && (self.operations.is_empty()
                || self.operations.len() > 3
                || self
                    .operations
                    .iter()
                    .any(|op| !["read", "write", "delete"].contains(&op.as_str()))
                || self
                    .operations
                    .iter()
                    .collect::<std::collections::HashSet<_>>()
                    .len()
                    != self.operations.len())
        {
            return Err(OdxError::validation(
                "validation.rule.operations_invalid",
                "block rules require valid read/write/delete operations",
            ));
        }
        if self.field_selectors.len() > 1000 {
            return Err(OdxError::validation(
                "validation.rule.selectors_count",
                "at most 1000 field selectors are allowed",
            ));
        }
        if self.action == RuleAction::Block
            && self.kind == RuleKind::Fields
            && self.field_selectors.is_empty()
            && self
                .fields
                .as_deref()
                .is_none_or(|fields| fields.is_empty())
        {
            return Err(OdxError::validation(
                "validation.rule.selectors_required",
                "field block rules require at least one field selector",
            ));
        }
        for selector in &self.field_selectors {
            if let Selector::Exact { value } = selector
                && !crate::tools::args::is_identifier(value, false)
            {
                return Err(OdxError::validation(
                    "validation.rule.field_invalid",
                    "invalid exact field selector",
                ));
            }
        }
        if !crate::tools::args::valid_model(&self.model) && self.model_selector.is_none() {
            return Err(OdxError::validation(
                "validation.rule.model_invalid",
                "invalid model",
            ));
        }
        if !crate::tools::args::valid_method(&self.method) && self.method != "all" {
            return Err(OdxError::validation(
                "validation.rule.method_invalid",
                "invalid method",
            ));
        }
        if self.action == RuleAction::Block && self.kind == RuleKind::Values {
            return Err(OdxError::validation(
                "validation.rule.block_values_unsupported",
                "block rules do not support value matching; use model or field selectors",
            ));
        }
        if self.action == RuleAction::Approve && self.method == "unlink" {
            return Err(OdxError::validation(
                "validation.rule.unlink_not_allowed",
                "unlink always requires individual approval; rules cannot authorize it",
            ));
        }
        if self.action == RuleAction::Approve && is_credential_call(&self.model, &self.method) {
            return Err(OdxError::validation(
                "validation.rule.credential_method_not_allowed",
                "methods generating or exposing API keys are always denied",
            ));
        }
        if self.action == RuleAction::Approve
            && crate::tools::catalog::READ_METHODS.contains(&self.method.as_str())
        {
            return Err(OdxError::validation(
                "validation.rule.read_method_not_needed",
                "reads do not need approval rules",
            ));
        }
        let payload_method = matches!(self.method.as_str(), "create" | "write");
        let (fields, values) = match (self.kind, self.fields, self.values) {
            (RuleKind::Fields, None, None) if self.action == RuleAction::Block => (None, None),
            (RuleKind::Method, None, None) => (None, None),
            (RuleKind::Fields, Some(fields), None)
                if payload_method || self.action == RuleAction::Block =>
            {
                if fields.is_empty() || fields.len() > 1000 {
                    return Err(OdxError::validation(
                        "validation.rule.fields_count",
                        "field rules require 1–1000 fields",
                    ));
                }
                if fields
                    .iter()
                    .any(|f| !crate::tools::args::is_identifier(f, false))
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
        if self.action == RuleAction::Approve
            && (self.max_records == 0 || self.max_records > MAX_RULE_RECORDS)
        {
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
        let environment_id = if self.scope == RuleScope::Global {
            String::new()
        } else {
            self.environment_id
        };
        Ok(Rule {
            id: self.id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            action: self.action,
            scope: self.scope,
            name: self.name,
            model_selector: self.model_selector,
            operations: self.operations,
            field_selectors: self.field_selectors,
            environment_id,
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
            || self.action != RuleAction::Approve
            || call.method == "unlink"
            || (self.scope != RuleScope::Global && self.environment_id != call.environment_id)
            || !model_matches(self, call.model)
            || (self.method != "all" && self.method != call.method)
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

fn block_selector_matches(selector: &Selector, value: &str) -> bool {
    match selector {
        Selector::Exact { value: expected } => expected == value,
        Selector::Regex { pattern } if pattern.len() > MAX_REGEX_BYTES => true,
        Selector::Regex { pattern } => {
            compiled_regex(pattern).is_none_or(|regex| regex.is_match(value))
        }
    }
}

fn field_selector_matches(selector: &Selector, path: &str) -> bool {
    block_selector_matches(selector, path)
        || path
            .split('.')
            .any(|part| block_selector_matches(selector, part))
}

fn model_matches(rule: &Rule, model: &str) -> bool {
    match rule.model_selector.as_ref() {
        None => rule.model == model,
        Some(ModelSelector::All) => true,
        Some(ModelSelector::Exact { value }) => value == model,
        Some(ModelSelector::Regex { pattern }) => {
            compiled_regex(pattern).is_some_and(|regex| regex.is_match(model))
        }
    }
}

fn block_model_matches(rule: &Rule, model: &str) -> bool {
    match rule.model_selector.as_ref() {
        None => rule.model == model,
        Some(ModelSelector::All) => true,
        Some(ModelSelector::Exact { value }) => value == model,
        Some(ModelSelector::Regex { pattern }) => block_selector_matches(
            &Selector::Regex {
                pattern: pattern.clone(),
            },
            model,
        ),
    }
}

fn operation_matches(rule: &Rule, effect: &Effect) -> bool {
    let operation = match effect {
        Effect::Read => "read",
        Effect::Write => "write",
        Effect::Delete => "delete",
    };
    if !rule.operations.is_empty() {
        if rule.action == RuleAction::Block
            && rule
                .operations
                .iter()
                .any(|op| !["read", "write", "delete"].contains(&op.as_str()))
        {
            return true;
        }
        return rule.operations.iter().any(|op| op == operation);
    }
    if rule.action == RuleAction::Block {
        return true;
    }
    matches!(effect, Effect::Write) // legacy rules were approval rules for writes
}

fn selected_field(field: &str, selectors: &[Selector], legacy: &[String]) -> bool {
    selectors.iter().any(|s| field_selector_matches(s, field))
        || legacy
            .iter()
            .any(|f| f == field || field.split('.').any(|part| f == part))
}

fn contains_write_field(value: &Value, selectors: &[Selector], legacy: &[String]) -> bool {
    match value {
        Value::Array(values) => values
            .iter()
            .any(|v| contains_write_field(v, selectors, legacy)),
        Value::Object(values) => values.iter().any(|(key, value)| {
            selected_field(key, selectors, legacy) || contains_write_field(value, selectors, legacy)
        }),
        _ => false,
    }
}

fn query_field_matches(value: &str, selectors: &[Selector], legacy: &[String]) -> bool {
    let trimmed = value.trim();
    let head = trimmed.split([':', ' ', '\t']).next().unwrap_or("");
    if selected_field(head, selectors, legacy) {
        return true;
    }
    if let Some((_, rest)) = trimmed.split_once('(')
        && let Some(inner) = rest.split(')').next()
    {
        let inner = inner.trim();
        if !inner.is_empty() && selected_field(inner, selectors, legacy) {
            return true;
        }
    }
    false
}

fn domain_contains_field(value: &Value, selectors: &[Selector], legacy: &[String]) -> bool {
    match value {
        Value::Array(values) => {
            if values.len() >= 3
                && values[0]
                    .as_str()
                    .is_some_and(|field| query_field_matches(field, selectors, legacy))
            {
                return true;
            }
            values
                .iter()
                .any(|v| domain_contains_field(v, selectors, legacy))
        }
        _ => false,
    }
}

// Until a relation can be resolved against authoritative metadata, fail closed. A rule on
// another model may apply to any segment or nested command; matching only the root model
// would silently bypass it.
fn contains_relational_path(value: &Value) -> bool {
    match value {
        Value::Array(items) => items.iter().any(contains_relational_path),
        Value::String(s) => s.split([',', ' ', '\t']).any(|part| part.contains('.')),
        _ => false,
    }
}

fn contains_relational_command(value: &Value) -> bool {
    match value {
        Value::Object(values) => values.values().any(contains_relational_command),
        Value::Array(values) => {
            crate::tools::mutations::is_command_list(value)
                || values.iter().any(contains_relational_command)
        }
        _ => false,
    }
}

fn uncertain_relation(call: &EffectiveCall<'_>, effect: &Effect) -> bool {
    if matches!(effect, Effect::Read) {
        let positions: &[(&str, usize)] = match call.method {
            "search_read" => &[("domain", 0), ("fields", 1), ("order", 4)],
            "search" => &[("domain", 0), ("order", 3)],
            "search_count" => &[("domain", 0)],
            "read_group" => &[("domain", 0), ("fields", 1), ("groupby", 2), ("orderby", 5)],
            "read" => &[("fields", 1)],
            "name_search" => &[("domain", 1)],
            _ => &[],
        };
        positions.iter().any(|(name, index)| {
            parameter(call, *index, name).is_some_and(contains_relational_path)
        })
    } else {
        let payload = match call.method {
            "create" => parameter(call, 0, "vals_list"),
            "write" | "web_save" => parameter(call, 1, "vals"),
            _ => None,
        };
        payload.is_some_and(contains_relational_command)
    }
}

/// Fields reached by a domain path, including each intermediate relation.
pub(crate) type ReadRelations = Vec<(String, String)>;

pub(crate) fn has_any_read_block(rules: &[Rule], environment_id: &str, now: DateTime<Utc>) -> bool {
    rules.iter().any(|rule| {
        rule.action == RuleAction::Block
            && !rule.is_expired(now)
            && (rule.scope == RuleScope::Global || rule.environment_id == environment_id)
            && operation_matches(rule, &Effect::Read)
    })
}

pub(crate) fn domain_paths(call: &EffectiveCall<'_>) -> Vec<String> {
    fn collect(value: &Value, paths: &mut Vec<String>) {
        if let Some(items) = value.as_array() {
            if let Some(field) = items
                .first()
                .and_then(Value::as_str)
                .filter(|s| items.len() >= 3 && s.contains('.'))
            {
                paths.push(field.to_owned());
            } else {
                for item in items {
                    collect(item, paths);
                }
            }
        }
    }
    let index = match call.method {
        "search_read" | "search" | "search_count" | "read_group" => 0,
        "name_search" => 1,
        _ => return Vec::new(),
    };
    let mut paths = Vec::new();
    if let Some(domain) = parameter(call, index, "domain") {
        collect(domain, &mut paths);
    }
    paths
}

pub(crate) fn matching_read_relations<'r>(
    rules: &'r [Rule],
    call: &EffectiveCall<'_>,
    relations: &ReadRelations,
    now: DateTime<Utc>,
) -> Option<&'r Rule> {
    relations.iter().find_map(|(model, field)| {
        rules.iter().find(|rule| {
            rule.action == RuleAction::Block
                && !rule.is_expired(now)
                && (rule.scope == RuleScope::Global || rule.environment_id == call.environment_id)
                && block_model_matches(rule, model)
                && (rule.method == "all" || rule.method == call.method)
                && operation_matches(rule, &Effect::Read)
                && (rule.kind != RuleKind::Fields
                    || selected_field(
                        field,
                        &rule.field_selectors,
                        rule.fields.as_deref().unwrap_or_default(),
                    ))
        })
    })
}

fn uncertain_non_domain_read(call: &EffectiveCall<'_>) -> bool {
    let positions: &[(&str, usize)] = match call.method {
        "search_read" => &[("fields", 1), ("order", 4)],
        "search" => &[("order", 3)],
        "read_group" => &[("fields", 1), ("groupby", 2), ("orderby", 5)],
        "read" => &[("fields", 1)],
        _ => &[],
    };
    positions
        .iter()
        .any(|(name, index)| parameter(call, *index, name).is_some_and(contains_relational_path))
}

fn named_or_positional<'a>(
    call: &'a EffectiveCall<'_>,
    index: usize,
    name: &str,
) -> Option<&'a Value> {
    parameter(call, index, name)
}

fn values_have_fields(value: Option<&Value>, selectors: &[Selector], legacy: &[String]) -> bool {
    match value {
        Some(Value::String(text)) => text.split(',').any(|part| {
            let field = part.split_whitespace().next().unwrap_or("");
            query_field_matches(field, selectors, legacy)
        }),
        Some(Value::Array(values)) => values.iter().any(|value| {
            value
                .as_str()
                .is_some_and(|field| query_field_matches(field, selectors, legacy))
        }),
        _ => false,
    }
}

fn read_inference_matches(
    call: &EffectiveCall<'_>,
    selectors: &[Selector],
    legacy: &[String],
    check_selection: bool,
) -> bool {
    if call.method == "fields_get" {
        return false;
    }
    let domain = match call.method {
        "search_read" | "search" | "search_count" | "read_group" => {
            named_or_positional(call, 0, "domain")
        }
        "name_search" => named_or_positional(call, 1, "domain"),
        _ => None,
    };
    if domain.is_some_and(|value| domain_contains_field(value, selectors, legacy)) {
        return true;
    }
    match call.method {
        "read" => {
            check_selection
                && values_have_fields(named_or_positional(call, 1, "fields"), selectors, legacy)
        }
        "search_read" => {
            (check_selection
                && values_have_fields(named_or_positional(call, 1, "fields"), selectors, legacy))
                || values_have_fields(named_or_positional(call, 4, "order"), selectors, legacy)
        }
        "search" => values_have_fields(named_or_positional(call, 3, "order"), selectors, legacy),
        "read_group" => {
            values_have_fields(named_or_positional(call, 1, "fields"), selectors, legacy)
                || values_have_fields(named_or_positional(call, 2, "groupby"), selectors, legacy)
                || values_have_fields(named_or_positional(call, 5, "orderby"), selectors, legacy)
        }
        _ => false,
    }
}

/// Return any active deny rule. Blocks intentionally ignore approval context and record limits.
pub fn matching_uncertain_relation<'r>(
    rules: &'r [Rule],
    call: &EffectiveCall,
    effect: &Effect,
    now: DateTime<Utc>,
) -> Option<&'r Rule> {
    if uncertain_relation(call, effect) {
        return rules.iter().find(|rule| {
            rule.action == RuleAction::Block
                && !rule.is_expired(now)
                && (rule.scope == RuleScope::Global || rule.environment_id == call.environment_id)
                && if matches!(effect, Effect::Read) {
                    operation_matches(rule, &Effect::Read)
                } else {
                    operation_matches(rule, &Effect::Write)
                        || operation_matches(rule, &Effect::Delete)
                }
        });
    }
    None
}

pub fn matching_block<'r>(
    rules: &'r [Rule],
    call: &EffectiveCall,
    effect: &Effect,
    now: DateTime<Utc>,
) -> Option<&'r Rule> {
    matching_block_with_domain(rules, call, effect, now, false)
}

pub(crate) fn matching_block_with_domain<'r>(
    rules: &'r [Rule],
    call: &EffectiveCall,
    effect: &Effect,
    now: DateTime<Utc>,
    resolved: bool,
) -> Option<&'r Rule> {
    if (!resolved || !matches!(effect, Effect::Read) || uncertain_non_domain_read(call))
        && let Some(rule) = matching_uncertain_relation(rules, call, effect, now)
    {
        return Some(rule);
    }
    rules.iter().find(|rule| {
        rule.action == RuleAction::Block
            && !rule.is_expired(now)
            && (rule.scope == RuleScope::Global || rule.environment_id == call.environment_id)
            && block_model_matches(rule, call.model)
            && (rule.method == "all" || rule.method == call.method)
            && (operation_matches(rule, effect)
                || (matches!(effect, Effect::Delete)
                    && matches!(call.method, "write" | "create" | "web_save")
                    && operation_matches(rule, &Effect::Write))
                || (matches!(effect, Effect::Write)
                    && (crate::tools::mutations::has_relational_delete(&Value::Array(
                        call.args.to_vec(),
                    )) || crate::tools::mutations::has_relational_delete(&Value::Object(
                        call.kwargs.clone(),
                    )))
                    && operation_matches(rule, &Effect::Delete)))
            && (rule.kind != RuleKind::Fields || {
                if call.method == "fields_get" {
                    false
                } else {
                    let selectors = &rule.field_selectors;
                    let fields = rule.fields.as_deref().unwrap_or_default();
                    if selectors.is_empty() && fields.is_empty() {
                        true
                    } else if matches!(effect, Effect::Read) {
                        read_inference_matches(call, selectors, fields, true)
                    } else {
                        let payload = match call.method {
                            "create" => parameter(call, 0, "vals_list"),
                            "write" | "web_save" => parameter(call, 1, "vals"),
                            _ => None,
                        };
                        payload.is_some_and(|value| contains_write_field(value, selectors, fields))
                    }
                }
            })
    })
}

pub fn first_field_block<'r>(
    rules: &'r [Rule],
    environment_id: &str,
    model: &str,
    method: &str,
    now: DateTime<Utc>,
) -> Option<&'r Rule> {
    rules.iter().find(|rule| {
        rule.action == RuleAction::Block
            && rule.kind == RuleKind::Fields
            && !rule.is_expired(now)
            && (rule.scope == RuleScope::Global || rule.environment_id == environment_id)
            && block_model_matches(rule, model)
            && (rule.method == "all" || rule.method == method)
            && operation_matches(rule, &Effect::Read)
    })
}

pub fn first_any_field_block<'r>(
    rules: &'r [Rule],
    environment_id: &str,
    model: &str,
    now: DateTime<Utc>,
) -> Option<&'r Rule> {
    rules.iter().find(|rule| {
        rule.action == RuleAction::Block
            && rule.kind == RuleKind::Fields
            && !rule.is_expired(now)
            && (rule.scope == RuleScope::Global || rule.environment_id == environment_id)
            && block_model_matches(rule, model)
    })
}

pub fn trace_rule(rule: &Rule) -> RuleTrace {
    RuleTrace {
        rule_id: rule.id.clone(),
        rule_revision: rule.revision,
    }
}

pub fn matching_read_model_block(
    rules: &[Rule],
    environment_id: &str,
    model: &str,
    method: &str,
    now: DateTime<Utc>,
) -> bool {
    first_read_model_block(rules, environment_id, model, method, now).is_some()
}

pub fn first_read_model_block<'r>(
    rules: &'r [Rule],
    environment_id: &str,
    model: &str,
    method: &str,
    now: DateTime<Utc>,
) -> Option<&'r Rule> {
    rules.iter().find(|rule| {
        rule.action == RuleAction::Block
            && !rule.is_expired(now)
            && (rule.scope == RuleScope::Global || rule.environment_id == environment_id)
            && block_model_matches(rule, model)
            && (rule.method == "all" || rule.method == method)
            && operation_matches(rule, &Effect::Read)
            && (rule.kind != RuleKind::Fields
                || (method != "fields_get"
                    && rule.field_selectors.is_empty()
                    && rule
                        .fields
                        .as_deref()
                        .is_none_or(|fields| fields.is_empty())))
    })
}

pub fn read_inference_blocked(rules: &[Rule], call: &EffectiveCall, now: DateTime<Utc>) -> bool {
    matching_read_inference_block(rules, call, now).is_some()
}

pub fn matching_read_inference_block<'r>(
    rules: &'r [Rule],
    call: &EffectiveCall,
    now: DateTime<Utc>,
) -> Option<&'r Rule> {
    // `fields_get` returns definitions keyed by field name. Its output is filtered below, so
    // selecting a name there does not disclose the field's value or infer it through records.
    if call.method == "fields_get" {
        return None;
    }
    rules.iter().find(|rule| {
        if rule.action != RuleAction::Block
            || rule.kind != RuleKind::Fields
            || rule.is_expired(now)
            || (rule.scope != RuleScope::Global && rule.environment_id != call.environment_id)
            || !block_model_matches(rule, call.model)
            || (rule.method != "all" && rule.method != call.method)
            || !operation_matches(rule, &Effect::Read)
        {
            return false;
        }
        read_inference_matches(
            call,
            &rule.field_selectors,
            rule.fields.as_deref().unwrap_or_default(),
            false,
        )
    })
}

pub fn field_is_blocked(
    rules: &[Rule],
    environment_id: &str,
    model: &str,
    method: &str,
    field: &str,
    now: DateTime<Utc>,
) -> bool {
    rules.iter().any(|rule| {
        rule.action == RuleAction::Block
            && rule.kind == RuleKind::Fields
            && !rule.is_expired(now)
            && (rule.scope == RuleScope::Global || rule.environment_id == environment_id)
            && block_model_matches(rule, model)
            && (rule.method == "all" || rule.method == method)
            && operation_matches(rule, &Effect::Read)
            && (rule.field_selectors.is_empty()
                && rule
                    .fields
                    .as_deref()
                    .is_none_or(|fields| fields.is_empty())
                || rule
                    .field_selectors
                    .iter()
                    .any(|s| field_selector_matches(s, field))
                || rule
                    .fields
                    .as_deref()
                    .unwrap_or_default()
                    .iter()
                    .any(|f| f == field || field.split('.').any(|part| f == part)))
    })
}

pub fn has_field_block(
    rules: &[Rule],
    environment_id: &str,
    model: &str,
    method: &str,
    now: DateTime<Utc>,
) -> bool {
    first_field_block(rules, environment_id, model, method, now).is_some()
}

pub fn has_any_field_block(
    rules: &[Rule],
    environment_id: &str,
    model: &str,
    now: DateTime<Utc>,
) -> bool {
    first_any_field_block(rules, environment_id, model, now).is_some()
}

/// Read methods whose responses contain only IDs, counts, booleans or XML IDs, never model field values.
pub(crate) fn is_value_free_read(method: &str) -> bool {
    matches!(
        method,
        "search" | "search_count" | "check_access_rights" | "get_external_id"
    )
}

pub fn has_explicit_read_fields(args: &[Value], kwargs: &Map<String, Value>, method: &str) -> bool {
    if is_value_free_read(method) {
        return true;
    }
    let fields = match method {
        // Metadata results are sanitized by key before audit and return.
        "fields_get" => return true,
        "read" if kwargs.contains_key("fields") => {
            if args.len() > 1 {
                return false;
            }
            kwargs.get("fields").and_then(Value::as_array)
        }
        "read" => {
            if args.len() > 2 {
                return false;
            }
            args.get(1).and_then(Value::as_array)
        }
        "search_read" => {
            if kwargs.contains_key("fields") {
                if args.len() > 1 {
                    return false;
                }
                kwargs.get("fields").and_then(Value::as_array)
            } else {
                if args.len() > 5 {
                    return false;
                }
                args.get(1).and_then(Value::as_array)
            }
        }
        _ => return false,
    };
    fields.is_some_and(|fields| !fields.is_empty() && fields.iter().all(Value::is_string))
}

pub fn trace_field_rules(
    rules: &[Rule],
    environment_id: &str,
    model: &str,
    method: &str,
    fields: &[String],
    now: DateTime<Utc>,
) -> Vec<RuleTrace> {
    let mut seen = std::collections::HashSet::new();
    rules
        .iter()
        .filter(|rule| {
            rule.action == RuleAction::Block
                && !rule.is_expired(now)
                && (rule.scope == RuleScope::Global || rule.environment_id == environment_id)
                && block_model_matches(rule, model)
                && (rule.method == "all" || rule.method == method)
                && operation_matches(rule, &Effect::Read)
                && fields.iter().any(|field| {
                    field_is_blocked(
                        std::slice::from_ref(*rule),
                        environment_id,
                        model,
                        method,
                        field,
                        now,
                    )
                })
        })
        .filter(|rule| seen.insert((rule.id.clone(), rule.revision)))
        .map(|rule| RuleTrace {
            rule_id: rule.id.clone(),
            rule_revision: rule.revision,
        })
        .collect()
}

/// Recheck and sanitize row shaped read results before audit and return. It deliberately does
/// not recurse into arbitrary metadata or scalar/custom method results.
pub fn redact_read_rows(
    rules: &[Rule],
    environment_id: &str,
    model: &str,
    method: &str,
    value: &mut Value,
    now: DateTime<Utc>,
) -> Option<Vec<String>> {
    if !has_field_block(rules, environment_id, model, method, now) || is_value_free_read(method) {
        return Some(Vec::new());
    }
    if method == "fields_get" {
        let definitions = value.as_object_mut()?;
        let blocked: Vec<String> = definitions
            .keys()
            .filter(|field| field_is_blocked(rules, environment_id, model, method, field, now))
            .cloned()
            .collect();
        for field in &blocked {
            definitions.remove(field);
        }
        return Some(blocked);
    }
    if !matches!(method, "read" | "search_read") {
        return None;
    }
    let rows = value.as_array_mut()?;
    if rows.iter().any(|row| !row.is_object()) {
        return None;
    }
    let mut redacted = std::collections::BTreeSet::new();
    for row in rows.iter_mut().filter_map(Value::as_object_mut) {
        let fields: Vec<_> = row
            .keys()
            .filter(|field| field_is_blocked(rules, environment_id, model, method, field, now))
            .cloned()
            .collect();
        redacted.extend(fields.iter().cloned());
        if fields.is_empty() {
            continue;
        }
        for field in &fields {
            row.insert(field.clone(), Value::String("[REDACTED]".into()));
        }
        row.insert(
            "__odx_redacted_fields".into(),
            Value::Array(fields.into_iter().map(Value::String).collect()),
        );
    }
    Some(redacted.into_iter().collect())
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
    crate::tools::mutations::parameter(call.args, call.kwargs, index, name).ok()
}

/// Explicit IDs: nonempty unique positive integers, as call validation requires.
fn explicit_ids(value: &Value) -> Option<usize> {
    let ids = crate::tools::mutations::integer_ids(value).ok()?;
    crate::tools::mutations::validate_ids(&ids).ok()?;
    Some(ids.len())
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
    pub(crate) async fn policy_snapshot(
        &self,
        environment_id: &str,
    ) -> Result<(Vec<Rule>, DateTime<Utc>)> {
        let s = self.inner.lock().await;
        let v = s.vault()?;
        let now = v.now();
        Ok((
            v.data()
                .rules
                .iter()
                .filter(|r| r.scope == RuleScope::Global || r.environment_id == environment_id)
                .cloned()
                .collect(),
            now,
        ))
    }

    /// Stored rules for one or all environments with current validity.
    pub async fn rules(&self, environment_id: Option<&str>) -> Result<Vec<RuleView>> {
        let s = self.inner.lock().await;
        let v = s.vault()?;
        let now = v.now();
        Ok(v.data()
            .rules
            .iter()
            .filter(|r| {
                r.scope == RuleScope::Global
                    || environment_id.is_none_or(|id| r.environment_id == id)
            })
            .map(|rule| RuleView {
                rule: rule.clone(),
                expired: rule.is_expired(now),
            })
            .collect())
    }
    /// Validate creates/updates; editing increments revision and prevents dispatch under older authorizations.
    pub async fn save_rule(&self, draft: RuleDraft) -> Result<RuleView> {
        let mut s = self.inner.lock().await;
        let v = s.vault_mut()?;
        let now = v.now();
        if draft.scope == RuleScope::Environment
            && !v
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
            if previous.environment_id != rule.environment_id || previous.scope != rule.scope {
                return Err(OdxError::validation(
                    "validation.rule.environment_immutable",
                    "a rule cannot change environment; create another",
                ));
            }
            if previous.action != rule.action {
                return Err(OdxError::validation(
                    "validation.rule.action_immutable",
                    "a rule cannot change action; create another",
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
        s.vault_mut()?.update(|data| {
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

    #[test]
    fn legacy_rules_deserialize_as_local_approvals_and_regex_is_bounded() {
        let legacy = serde_json::json!({
            "id":"legacy", "environment_id":"e", "model":"res.partner", "method":"write",
            "kind":"method", "context":{}, "max_records":2, "expires_at":null,
            "revision":1, "created_at":Utc::now()
        });
        let rule: Rule = serde_json::from_value(legacy).unwrap();
        assert_eq!(rule.action, RuleAction::Approve);
        assert_eq!(rule.scope, RuleScope::Environment);
        assert!(rule.field_selectors.is_empty());

        let draft: RuleDraft = serde_json::from_value(serde_json::json!({
            "environment_id":"e", "model":"", "method":"all", "kind":"method",
            "action":"block", "scope":{"kind":"global"}, "operations":["read"],
            "model_selector":{"kind":"all"}, "field_selectors":[{"kind":"regex","pattern":"(?i)(token|password)"}],
            "context":{}, "max_records":0
        })).unwrap();
        assert!(draft.into_rule(Utc::now()).is_ok());

        let too_large: RuleDraft = serde_json::from_value(serde_json::json!({
            "environment_id":"e", "model":"", "method":"all", "kind":"method",
            "action":"block", "model_selector":{"kind":"all"},
            "field_selectors":[{"kind":"regex","pattern":"x".repeat(257)}],
            "context":{}, "max_records":0
        }))
        .unwrap();
        assert_eq!(
            too_large.into_rule(Utc::now()).unwrap_err().code(),
            "validation.rule.regex_invalid"
        );

        let invalid_persisted: Rule = serde_json::from_value(serde_json::json!({
            "id":"bad", "environment_id":"", "model":"", "method":"all", "kind":"fields",
            "action":"block", "scope":{"kind":"global"}, "name":"bad persisted selector",
            "model_selector":{"kind":"all"}, "operations":["read"],
            "field_selectors":[{"kind":"regex","pattern":"("}], "fields":null,
            "values":null, "context":{}, "max_records":0, "expires_at":null,
            "revision":1, "created_at":Utc::now()
        }))
        .unwrap();
        assert!(validate_persisted_rules(&[invalid_persisted]).is_err());

        let glob = Selector::Regex {
            pattern: "*token*".into(),
        };
        assert_eq!(
            validate_selector(&glob).unwrap_err().code(),
            "validation.rule.regex_invalid"
        );
    }

    #[test]
    fn model_regex_selectors_are_typed_and_match_as_regex_not_glob() {
        let draft: RuleDraft = serde_json::from_value(serde_json::json!({
            "environment_id":"", "model":"", "method":"all", "kind":"method",
            "action":"block", "scope":{"kind":"global"}, "operations":["read"],
            "model_selector":{"kind":"regex","pattern":"^res\\.partner$"},
            "context":{}, "max_records":0
        }))
        .unwrap();
        let rule = draft.into_rule(Utc::now()).unwrap();
        let args = [];
        let kwargs = Map::new();
        let matches = EffectiveCall {
            environment_id: "future-environment",
            model: "res.partner",
            method: "search_read",
            args: &args,
            kwargs: &kwargs,
        };
        assert!(
            matching_block(
                std::slice::from_ref(&rule),
                &matches,
                &Effect::Read,
                Utc::now()
            )
            .is_some()
        );
        let other = EffectiveCall {
            model: "res.users",
            ..matches
        };
        assert!(matching_block(&[rule], &other, &Effect::Read, Utc::now()).is_none());
    }

    #[test]
    fn global_blocks_ignore_context_and_match_any_selected_field() {
        let draft: RuleDraft = serde_json::from_value(serde_json::json!({
            "environment_id":"", "model":"", "method":"all", "kind":"fields",
            "action":"block", "scope":{"kind":"global"}, "operations":["read"],
            "model_selector":{"kind":"all"}, "field_selectors":[{"kind":"regex","pattern":"(?i)(token|password)"}],
            "context":{}, "max_records":0
        })).unwrap();
        let rule = draft.into_rule(Utc::now()).unwrap();
        let args = [serde_json::json!([1])];
        let kwargs = Map::from_iter([
            ("fields".into(), serde_json::json!(["id", "api_token"])),
            ("lang".into(), serde_json::json!("fr_FR")),
        ]);
        let call = EffectiveCall {
            environment_id: "future-env",
            model: "res.partner",
            method: "read",
            args: &args,
            kwargs: &kwargs,
        };
        assert!(matching_block(&[rule], &call, &Effect::Read, Utc::now()).is_some());
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
            action: RuleAction::Approve,
            scope: RuleScope::Environment,
            name: String::new(),
            model_selector: None,
            operations: vec![],
            field_selectors: vec![],
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
