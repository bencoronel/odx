use crate::{
    output::ToolError,
    tools::catalog::{Args, domain, field_list, fields, ids, is_identifier, order, with_id},
};
use serde_json::{Map, Value, json};

/// Adapt known Odoo 16–19 read signatures only. Preserve result shape; fetch an extra row only for pageable lists.
pub(crate) struct ReadRequest {
    pub args: Value,
    pub kwargs: Map<String, Value>,
    pub page: Option<(usize, usize)>,
    pub name_limit: Option<usize>,
}

pub(crate) fn prepare_read(
    method: &str,
    positional: Vec<Value>,
    mut kwargs: Map<String, Value>,
    json2: bool,
) -> Result<ReadRequest, ToolError> {
    let signature: &[&'static str] = match method {
        "search_read" => &["domain", "fields", "offset", "limit", "order"],
        "read" => &["ids", "fields", "load"],
        "search" => &["domain", "offset", "limit", "order"],
        "search_count" => &["domain", "limit"],
        "name_search" => &["name", "domain", "operator", "limit"],
        "fields_get" => &["allfields", "attributes"],
        "read_group" => &[
            "domain", "fields", "groupby", "offset", "limit", "orderby", "lazy",
        ],
        "get_external_id" => &["ids"],
        "get_views" => &["views", "options"],
        "check_access_rights" => &["operation", "raise_exception"],
        _ => return Err(ToolError::invalid("unknown read signature")),
    };
    // Legacy name_search sends domain positionally; it was named args in Odoo 16–18.
    if method == "name_search"
        && !json2
        && let Some(value) = kwargs.remove("args")
        && kwargs.insert("domain".into(), value).is_some()
    {
        return Err(ToolError::invalid("duplicate parameter: domain/args"));
    }
    if positional.len() > signature.len() {
        return Err(ToolError::invalid(format!(
            "too many positional arguments for {method}"
        )));
    }
    for (name, value) in signature.iter().zip(positional) {
        if kwargs.insert((*name).into(), value).is_some() {
            return Err(ToolError::invalid(format!("duplicate parameter: {name}")));
        }
    }
    let mut parsed = Args::new(Value::Object(kwargs), signature)?;
    let mut out = ReadRequest {
        args: json!([]),
        kwargs: parsed.map.clone(),
        page: None,
        name_limit: None,
    };
    if matches!(
        method,
        "search_read" | "search" | "search_count" | "read_group" | "name_search"
    ) {
        out.kwargs.insert("domain".into(), domain(&mut parsed)?);
    }
    if matches!(method, "read" | "search_read") {
        out.kwargs.insert(
            "fields".into(),
            json!(with_id(fields(&mut parsed, &["display_name"])?)),
        );
    }
    if matches!(method, "read" | "get_external_id") {
        let records = ids(&mut parsed)?;
        out.kwargs.remove("ids");
        out.args = json!([records]);
        if let Some(load) = parsed.raw("load")
            && !load.is_string()
            && load != json!(false)
        {
            return Err(ToolError::invalid("load must be text or false"));
        }
    }
    if matches!(method, "search_read" | "search" | "read_group") {
        let limit = parsed.int("limit", 50, 1, 1000)? as usize;
        let offset = parsed.int("offset", 0, 0, i64::MAX)? as usize;
        out.kwargs.insert("limit".into(), json!(limit + 1));
        out.kwargs.insert("offset".into(), json!(offset));
        out.page = Some((limit, offset));
        if method != "read_group" {
            out.kwargs
                .insert("order".into(), json!(order(&mut parsed, "order")?));
        }
    }
    match method {
        "search_count" if parsed.has("limit") => {
            parsed.int("limit", 50, 1, 1000)?;
        }
        "fields_get" => {
            for key in ["allfields", "attributes"] {
                if parsed.has(key) {
                    let values = field_list(&mut parsed, key, &[])?;
                    if values.len() > 1000 {
                        return Err(ToolError::invalid(format!(
                            "{key} accepts at most 1000 items"
                        )));
                    }
                }
            }
        }
        "read_group" => {
            let (groupby, aggregates) = super::composite::group_fields(&mut parsed)?;
            out.kwargs.insert("groupby".into(), json!(groupby));
            out.kwargs.insert("fields".into(), json!(aggregates));
            let lazy = parsed.bool("lazy")?;
            out.kwargs.insert("lazy".into(), json!(lazy));
            if let Some(raw) = parsed.str_opt("orderby")? {
                out.kwargs.insert(
                    "orderby".into(),
                    json!(super::composite::group_order(&raw)?),
                );
            }
        }
        "name_search" => {
            let name = parsed.text("name")?.unwrap_or_default();
            let operator = parsed
                .str_opt("operator")?
                .unwrap_or_else(|| "ilike".into());
            let limit = parsed.int("limit", 50, 1, 1000)? as usize;
            out.name_limit = Some(limit);
            if json2 {
                out.kwargs.insert("name".into(), json!(name));
                out.kwargs.insert("operator".into(), json!(operator));
                out.kwargs.insert("limit".into(), json!(limit + 1));
            } else {
                out.args = json!([name, out.kwargs["domain"], operator, limit + 1]);
                out.kwargs.clear();
            }
        }
        "check_access_rights" => {
            let operation = parsed.str("operation")?;
            if !["read", "write", "create", "unlink"].contains(&operation.as_str()) {
                return Err(ToolError::invalid(
                    "operation must be read, write, create or unlink",
                ));
            }
            if parsed.has("raise_exception") {
                parsed.bool("raise_exception")?;
            }
        }
        "get_views" => {
            let views = parsed
                .array("views")?
                .ok_or_else(|| ToolError::invalid("missing views"))?;
            if views.is_empty()
                || views.len() > 1000
                || views.iter().any(|view| {
                    let Some(pair) = view.as_array().filter(|p| p.len() == 2) else {
                        return true;
                    };
                    let valid_id = pair[0] == json!(false)
                        || pair[0]
                            .as_i64()
                            .is_some_and(|id| (1..=9_007_199_254_740_991).contains(&id));
                    !valid_id || !pair[1].as_str().is_some_and(|s| is_identifier(s, false))
                })
            {
                return Err(ToolError::invalid(
                    "views requires 1–1000 pairs of [id or false, view type]",
                ));
            }
            if parsed.has("options") {
                parsed.object("options")?;
            }
        }
        _ => {}
    }
    Ok(out)
}
pub fn effective_context(global: &Value, environment: &Value, call: &Value) -> Value {
    let mut out = serde_json::Map::new();
    for source in [global, environment, call] {
        if let Some(m) = source.as_object() {
            for (k, v) in m {
                out.insert(k.clone(), v.clone());
            }
        }
    }
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn context_precedence_is_global_then_environment_then_call() {
        assert_eq!(
            effective_context(
                &json!({"lang":"global","tz":"UTC","active_test":true}),
                &json!({"lang":"environment","allowed_company_ids":[1]}),
                &json!({"lang":"call"}),
            ),
            json!({
                "lang":"call",
                "tz":"UTC",
                "active_test":true,
                "allowed_company_ids":[1]
            })
        );
    }
}
