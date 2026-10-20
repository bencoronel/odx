//! Odoo 19 JSON-2: `POST /json/2/<model>/<method>`. Sources: External JSON-2 API documentation and addons/rpc/controllers/json2.py. Bearer API key, X-Odoo-Database and named body arguments including ids/context. Each call is a separate transaction. Errors use 4xx/5xx with name, message, arguments, context and debug.
use super::jsonrpc::{MAX_RESPONSE, OdooClient, read_limited, scrub};
use crate::{
    config::{Environment, Protocol},
    error::{OdxError, Result},
};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderValue};
use serde_json::{Map, Value};
use zeroize::Zeroizing;

const NOT_AVAILABLE: &str = "server has no JSON-2 endpoint (/json/2); Odoo 19 or later is required";

/// Known Odoo 19 signatures: record IDs and positional parameter names. Translate only these internal calls; all other methods require named arguments.
const SIGNATURES: [(&str, bool, &[&str]); 13] = [
    (
        "check_access_rights",
        false,
        &["operation", "raise_exception"],
    ),
    ("create", false, &["vals_list"]),
    ("fields_get", false, &["allfields", "attributes"]),
    ("get_external_id", true, &[]),
    ("get_views", false, &["views", "options"]),
    (
        "name_search",
        false,
        &["name", "domain", "operator", "limit"],
    ),
    ("read", true, &["fields", "load"]),
    (
        "read_group",
        false,
        &[
            "domain", "fields", "groupby", "offset", "limit", "orderby", "lazy",
        ],
    ),
    ("search", false, &["domain", "offset", "limit", "order"]),
    ("search_count", false, &["domain", "limit"]),
    (
        "search_read",
        false,
        &["domain", "fields", "offset", "limit", "order"],
    ),
    ("unlink", true, &[]),
    ("write", true, &["vals"]),
];

/// JSON-2 `call` requires named arguments; never infer names.
pub(crate) fn reject_positional(env: &Environment, args: &Value) -> Result<()> {
    if env.protocol == Protocol::Json2 && args.as_array().is_some_and(|a| !a.is_empty()) {
        return Err(OdxError::validation(
            "validation.console.json2_positional_arguments",
            "JSON-2 requires named arguments: leave args empty and use Odoo 19 parameter names in kwargs, with records in kwargs.ids",
        ));
    }
    Ok(())
}

/// Build JSON-2 bodies from execute_kw positional and named arguments.
pub(crate) fn body(method: &str, args: Value, kwargs: Value) -> Result<Map<String, Value>> {
    let mut body = match kwargs {
        Value::Object(map) => map,
        Value::Null => Map::new(),
        _ => {
            return Err(OdxError::validation(
                "validation.console.kwargs_object_required",
                "kwargs must be an object",
            ));
        }
    };
    let Value::Array(args) = args else {
        return Err(OdxError::validation(
            "validation.console.args_list_required",
            "args must be a list",
        ));
    };
    if args.is_empty() {
        return Ok(body);
    }
    let Some((_, record, params)) = SIGNATURES.iter().find(|(m, _, _)| *m == method) else {
        return Err(OdxError::validation(
            "validation.console.json2_method_signature_unknown",
            format!("JSON-2 requires named arguments for {method}; use kwargs"),
        ));
    };
    let mut args = args.into_iter();
    let mut named = Vec::new();
    if *record {
        let ids = match args.next() {
            Some(Value::Array(ids)) => Value::Array(ids),
            Some(id @ Value::Number(_)) => Value::Array(vec![id]),
            _ => {
                return Err(OdxError::validation(
                    "validation.console.json2_ids_required",
                    format!("{method} expects an ID list"),
                ));
            }
        };
        named.push(("ids", ids));
    }
    let rest: Vec<Value> = args.collect();
    if rest.len() > params.len() {
        return Err(OdxError::validation(
            "validation.console.json2_too_many_arguments",
            format!("too many positional arguments for {method}"),
        ));
    }
    named.extend(params.iter().copied().zip(rest));
    for (name, value) in named {
        if body.insert(name.to_owned(), value).is_some() {
            return Err(OdxError::validation(
                "validation.console.argument_duplicated",
                format!("{name} supplied both positionally and by name"),
            ));
        }
    }
    Ok(body)
}

pub(crate) async fn execute(
    client: &OdooClient,
    env: &Environment,
    model: &str,
    method: &str,
    args: Value,
    kwargs: Value,
    max_response: usize,
) -> Result<(Value, usize)> {
    let body = body(method, args, kwargs)?;
    post(client, env, model, method, &body, max_response).await
}

/// Resolve the key's uid with res.users/context_get without IDs (Odoo 19 migration documentation).
pub(crate) async fn user_id(client: &OdooClient, env: &Environment) -> Result<(Value, usize)> {
    probe_user_id(client, env).await.map_err(|e| match e {
        OdxError::HttpStatus { status: 404 } => OdxError::Upstream(NOT_AVAILABLE.into()),
        e => e,
    })
}

pub(crate) async fn probe_user_id(
    client: &OdooClient,
    env: &Environment,
) -> Result<(Value, usize)> {
    let (context, size) = post(
        client,
        env,
        "res.users",
        "context_get",
        &Map::new(),
        MAX_RESPONSE,
    )
    .await?;
    Ok((context.get("uid").cloned().unwrap_or(Value::Null), size))
}

/// GET /web/version replaces common.version in Odoo 19.
pub(crate) async fn version(client: &OdooClient, env: &Environment) -> Result<Option<String>> {
    probe_version(client, env).await.map_err(|e| match e {
        OdxError::HttpStatus { status: 404 } => OdxError::Upstream(NOT_AVAILABLE.into()),
        e => e,
    })
}

pub(crate) async fn probe_version(
    client: &OdooClient,
    env: &Environment,
) -> Result<Option<String>> {
    let headers = crate::headers::request_headers(&env.headers)?;
    let response = client
        .send(env, |http| {
            http.get(format!("{}/web/version", env.url))
                .headers(headers.clone())
                .timeout(client.request_timeout())
        })
        .await?;
    match response.status().as_u16() {
        200..=299 => {}
        status => return Err(OdxError::HttpStatus { status }),
    }
    let bytes = read_limited(response, env, MAX_RESPONSE).await?;
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|_| OdxError::Upstream("invalid /web/version response".into()))?;
    Ok(value
        .get("version")
        .and_then(Value::as_str)
        .map(|v| v.chars().take(64).collect()))
}

async fn post(
    client: &OdooClient,
    env: &Environment,
    model: &str,
    method: &str,
    body: &Map<String, Value>,
    max_response: usize,
) -> Result<(Value, usize)> {
    let headers = crate::headers::request_headers(&env.headers)?;
    // Reserve Authorization and X-Odoo-Database for the adapter.
    if headers.contains_key(AUTHORIZATION) || headers.contains_key("x-odoo-database") {
        return Err(OdxError::validation(
            "validation.environment.json2_reserved_headers",
            "JSON-2 manages Authorization and X-Odoo-Database; remove them from environment headers",
        ));
    }
    let mut url = url::Url::parse(&env.url)
        .map_err(|_| OdxError::validation("validation.environment.url_invalid", "invalid URL"))?;
    url.path_segments_mut()
        .map_err(|_| OdxError::validation("validation.environment.url_invalid", "invalid URL"))?
        .pop_if_empty()
        .extend(["json", "2", model, method]);
    let bearer = Zeroizing::new(format!("bearer {}", env.secret.as_str()));
    let mut authorization = HeaderValue::from_str(&bearer).map_err(|_| {
        OdxError::validation(
            "validation.environment.api_key_header_invalid",
            "API key contains invalid characters",
        )
    })?;
    authorization.set_sensitive(true);
    let database = HeaderValue::from_str(&env.database).map_err(|_| {
        OdxError::validation(
            "validation.environment.database_header_invalid",
            "invalid database name for JSON-2",
        )
    })?;
    let payload = bytes::Bytes::from_owner(crate::security::WipeBody(Zeroizing::new(
        serde_json::to_vec(body)?,
    )));
    let response = client
        .send(env, |http| {
            http.post(url.clone())
                .header(CONTENT_TYPE, "application/json; charset=utf-8")
                .headers(headers.clone())
                .header(AUTHORIZATION, authorization.clone())
                .header("X-Odoo-Database", database.clone())
                .body(payload.clone())
                .timeout(client.request_timeout())
        })
        .await?;
    let status = response.status().as_u16();
    let bytes = read_limited(response, env, max_response).await?;
    let size = bytes.len();
    if (200..300).contains(&status) {
        let value: Value = serde_json::from_slice(&bytes)
            .map_err(|_| OdxError::Upstream("invalid JSON-2 response".into()))?;
        return Ok((value, size));
    }
    if status == 401 {
        return Err(OdxError::AuthenticationRejected);
    }
    // Only an Odoo error object proves transactional failure; other responses retain their HTTP status.
    let error: Option<Value> = serde_json::from_slice(&bytes).ok();
    match error.as_ref().and_then(odoo_error_message) {
        Some(message) => Err(OdxError::Remote(scrub(
            &format!("{message} (HTTP {status})"),
            env,
        ))),
        None => Err(OdxError::HttpStatus { status }),
    }
}

fn odoo_error_message(error: &Value) -> Option<&str> {
    error.get("name").and_then(Value::as_str)?;
    error.get("arguments")?;
    error.get("message").and_then(Value::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn translates_only_known_signatures() {
        assert_eq!(
            body("read", json!([[1, 2]]), json!({"fields":["name"]})).unwrap(),
            json!({"fields":["name"],"ids":[1,2]})
                .as_object()
                .unwrap()
                .clone()
        );
        assert_eq!(
            body("write", json!([3, {"name":"x"}]), json!({"context":{}})).unwrap(),
            json!({"context":{},"ids":[3],"vals":{"name":"x"}})
                .as_object()
                .unwrap()
                .clone()
        );
        assert_eq!(
            body("name_search", json!(["Al", [], "ilike", 6]), json!({})).unwrap(),
            json!({"name":"Al","domain":[],"operator":"ilike","limit":6})
                .as_object()
                .unwrap()
                .clone()
        );
        assert_eq!(
            body("search", json!([[["a", "=", 1]]]), json!({"limit": 5})).unwrap(),
            json!({"limit":5,"domain":[["a","=",1]]})
                .as_object()
                .unwrap()
                .clone()
        );
        // Without positional arguments, pass kwargs unchanged for any method.
        assert_eq!(
            body("action_confirm", json!([]), json!({"ids":[1]})).unwrap(),
            json!({"ids":[1]}).as_object().unwrap().clone()
        );
        assert!(body("action_confirm", json!([[1]]), json!({})).is_err());
        assert!(body("read", json!([[1]]), json!({"ids":[1]})).is_err());
        assert!(body("search", json!([[], 0, 1, "id", "extra"]), json!({})).is_err());
        assert!(body("read", json!(["x"]), json!({})).is_err());
    }

    #[test]
    fn call_rejects_positional_args_only_on_json2() {
        let mut env = Environment {
            protocol: Protocol::Json2,
            ..Default::default()
        };
        assert!(reject_positional(&env, &json!([[1]])).is_err());
        assert!(reject_positional(&env, &json!([])).is_ok());
        env.protocol = Protocol::Xmlrpc;
        assert!(reject_positional(&env, &json!([[1]])).is_ok());
    }
}
