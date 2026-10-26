//! Tool arguments: the JSON object a tool receives, checked against its tools.schema.json entry, and the shared
//! parsers for models, fields, domains, ids, order and paging.
use crate::{
    output::{Format, ToolError},
    tools::{catalog::SCHEMA, mutations},
};
use serde_json::{Map, Value, json};

pub(crate) struct Args {
    pub(crate) map: Map<String, Value>,
}
impl Args {
    /// Arguments of tool `name`: exactly the properties its tools.schema.json entry declares.
    pub(crate) fn for_tool(name: &str, value: Value) -> Result<Self, ToolError> {
        let allowed: Vec<&'static str> = SCHEMA["$defs"][name]["properties"]
            .as_object()
            .map(|properties| properties.keys().map(String::as_str).collect())
            .unwrap_or_default();
        Self::new(value, &allowed)
    }
    pub(crate) fn new(value: Value, allowed: &[&'static str]) -> Result<Self, ToolError> {
        let Value::Object(map) = value else {
            return Err(ToolError::invalid("arguments must be a JSON object"));
        };
        for (key, v) in &map {
            if !allowed.contains(&key.as_str()) {
                return Err(ToolError::invalid(format!("unknown field: {key}")));
            }
            if v.is_null() {
                return Err(ToolError::invalid(format!("{key} cannot be null; omit it")));
            }
        }
        Ok(Self { map })
    }
    pub(crate) fn has(&self, key: &str) -> bool {
        self.map.contains_key(key)
    }
    pub(crate) fn str_opt(&mut self, key: &'static str) -> Result<Option<String>, ToolError> {
        match self.map.get(key) {
            None => Ok(None),
            Some(Value::String(s)) if !s.is_empty() => Ok(Some(s.clone())),
            Some(_) => Err(ToolError::invalid(format!("{key} must be nonempty text"))),
        }
    }
    pub(crate) fn str(&mut self, key: &'static str) -> Result<String, ToolError> {
        self.str_opt(key)?
            .ok_or_else(|| ToolError::invalid(format!("missing {key}")))
    }
    pub(crate) fn int(
        &mut self,
        key: &'static str,
        default: i64,
        min: i64,
        max: i64,
    ) -> Result<i64, ToolError> {
        match self.map.get(key) {
            None => Ok(default),
            Some(v) => v
                .as_i64()
                .filter(|n| (min..=max).contains(n))
                .ok_or_else(|| {
                    ToolError::invalid(format!("{key} must be an integer between {min} and {max}"))
                }),
        }
    }
    pub(crate) fn object(&mut self, key: &'static str) -> Result<Map<String, Value>, ToolError> {
        match self.map.get(key) {
            None => Ok(Map::new()),
            Some(Value::Object(m)) => Ok(m.clone()),
            Some(_) => Err(ToolError::invalid(format!("{key} must be a JSON object"))),
        }
    }
    pub(crate) fn array(&mut self, key: &'static str) -> Result<Option<Vec<Value>>, ToolError> {
        match self.map.get(key) {
            None => Ok(None),
            Some(Value::Array(a)) => Ok(Some(a.clone())),
            Some(_) => Err(ToolError::invalid(format!("{key} must be a list"))),
        }
    }
    pub(crate) fn bool(&mut self, key: &'static str) -> Result<bool, ToolError> {
        match self.map.get(key) {
            None => Ok(false),
            Some(Value::Bool(b)) => Ok(*b),
            Some(_) => Err(ToolError::invalid(format!("{key} must be boolean"))),
        }
    }
    pub(crate) fn raw(&mut self, key: &'static str) -> Option<Value> {
        self.map.get(key).cloned()
    }
    /// Text allowing empty values for fields/models q filters.
    pub(crate) fn text(&mut self, key: &'static str) -> Result<Option<String>, ToolError> {
        match self.map.get(key) {
            None => Ok(None),
            Some(Value::String(s)) => Ok(Some(s.clone())),
            Some(_) => Err(ToolError::invalid(format!("{key} must be text"))),
        }
    }
}

pub(crate) fn is_identifier(s: &str, dotted: bool) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase() || c == '_' || (!dotted && c.is_ascii_uppercase()))
        && s.len() <= 128
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || (dotted && c == '.'))
}
/// Technical model name such as res.partner.
pub(crate) fn valid_model(m: &str) -> bool {
    is_identifier(m, true) && !m.contains("..") && !m.ends_with('.')
}
/// Public method name: private ones (leading underscore) are never exposed.
pub(crate) fn valid_method(m: &str) -> bool {
    is_identifier(m, false) && !m.starts_with('_')
}
pub(crate) fn model(args: &mut Args) -> Result<String, ToolError> {
    let m = args.str("model")?;
    if !valid_model(&m) {
        return Err(ToolError::invalid(format!("invalid model: {m}")));
    }
    Ok(m)
}
pub(crate) fn fields(args: &mut Args, default: &[&str]) -> Result<Vec<String>, ToolError> {
    field_list(args, "fields", default)
}
pub(crate) fn field_list(
    args: &mut Args,
    key: &'static str,
    default: &[&str],
) -> Result<Vec<String>, ToolError> {
    let Some(list) = args.array(key)? else {
        return Ok(default.iter().map(|s| s.to_string()).collect());
    };
    if list.is_empty() {
        return Err(ToolError::invalid(format!("{key} cannot be empty")));
    }
    let mut out: Vec<String> = Vec::new();
    for f in list {
        let Some(name) = f.as_str().filter(|n| is_identifier(n, false)) else {
            return Err(ToolError::invalid(format!("invalid field: {f}")));
        };
        if out.iter().any(|x| x == name) {
            return Err(ToolError::invalid(format!("duplicate field: {name}")));
        }
        out.push(name.into());
    }
    Ok(out)
}
pub(crate) fn with_id(mut fields: Vec<String>) -> Vec<String> {
    if !fields.iter().any(|f| f == "id") {
        fields.insert(0, "id".into());
    }
    fields
}
pub(crate) fn domain(args: &mut Args) -> Result<Value, ToolError> {
    let Some(items) = args.array("domain")? else {
        return Ok(json!([]));
    };
    for item in &items {
        let ok = match item {
            Value::String(op) => matches!(op.as_str(), "&" | "|" | "!"),
            Value::Array(t) => {
                t.len() == 3 && t[0].as_str().is_some_and(|f| !f.is_empty()) && t[1].is_string()
            }
            _ => false,
        };
        if !ok {
            return Err(ToolError::invalid(format!(
                "invalid domain item: {item}; use [field, operator, value] or '&', '|', '!'"
            )));
        }
    }
    Ok(Value::Array(items))
}
/// Append id as a stable tie-breaker if missing.
pub(crate) fn order(args: &mut Args, key: &'static str) -> Result<String, ToolError> {
    let Some(raw) = args.str_opt(key)? else {
        return Ok("id asc".into());
    };
    let mut has_id = false;
    for part in raw.split(',') {
        let tokens: Vec<&str> = part.split_whitespace().collect();
        let valid = match tokens.as_slice() {
            [f] => is_identifier(f, false),
            [f, dir] => {
                is_identifier(f, false)
                    && matches!(dir.to_ascii_lowercase().as_str(), "asc" | "desc")
            }
            _ => false,
        };
        if !valid {
            return Err(ToolError::invalid(format!("invalid order: {raw}")));
        }
        has_id |= tokens[0] == "id";
    }
    Ok(if has_id {
        raw
    } else {
        format!("{raw}, id asc")
    })
}
pub(crate) fn ids(args: &mut Args) -> Result<Vec<i64>, ToolError> {
    let list = args
        .array("ids")?
        .ok_or_else(|| ToolError::invalid("missing ids"))?;
    let ids: Vec<i64> = list
        .iter()
        .map(|v| v.as_i64().filter(|n| *n > 0 && *n <= 9_007_199_254_740_991))
        .collect::<Option<_>>()
        .ok_or_else(|| ToolError::invalid("ids must be positive integers"))?;
    mutations::validate_ids(&ids)?;
    Ok(ids)
}
/// Remove the extra fetched row and populate meta.page.
pub(crate) fn paged(
    mut data: Value,
    limit: usize,
    offset: usize,
    meta: &mut Map<String, Value>,
) -> Value {
    if let Value::Array(rows) = &mut data {
        meta.insert("page".into(), crate::output::page_of(rows, limit, offset));
    }
    data
}
pub(crate) fn common(args: &mut Args) -> Result<(Format, usize), ToolError> {
    let format = match args.map.get("format").and_then(Value::as_str) {
        None if !args.has("format") => Format::Compact,
        Some("compact") => Format::Compact,
        Some("json") => Format::Json,
        _ => return Err(ToolError::invalid("format must be compact or json")),
    };
    let trunc = args.int("trunc", 200, 0, i64::MAX)? as usize;
    Ok((format, trunc))
}

/// Required positive interoperable record id.
pub(crate) fn record_id(args: &mut Args) -> Result<i64, ToolError> {
    if !args.has("id") {
        return Err(ToolError::invalid("missing id"));
    }
    args.int("id", 0, 1, 9_007_199_254_740_991)
}

pub(crate) fn kw(pairs: &[(&str, Value)]) -> Map<String, Value> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_reject_unknown_fields_nulls_and_bad_identifiers() {
        assert!(Args::for_tool("search_read", json!({"env":"a","extra":1})).is_err());
        assert!(Args::for_tool("search_read", json!({"env":null})).is_err());
        let mut a = Args::for_tool("search_read", json!({"model":"res.partner;drop"})).unwrap();
        assert!(model(&mut a).is_err());
        let mut a = Args::for_tool("search_read", json!({"domain":"[('id','=',1)]"})).unwrap();
        assert!(domain(&mut a).is_err());
        let mut a = Args::for_tool("search_read", json!({"order":"name desc; drop"})).unwrap();
        assert!(order(&mut a, "order").is_err());
        let mut a = Args::for_tool("search_read", json!({"order":"name desc"})).unwrap();
        assert_eq!(order(&mut a, "order").unwrap(), "name desc, id asc");
        let mut a = Args::for_tool("search_read", json!({"fields":["name","name"]})).unwrap();
        assert!(fields(&mut a, &[]).is_err());
    }
}
