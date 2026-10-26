//! JSON-2 (Odoo 19): /json/2/<model>/<method> with named arguments and HTTP status errors.
use super::*;

/// Odoo 19 signatures from odoo/orm/models.py: ids support and allowed parameter names.
const JSON2_SIGNATURES: [(&str, bool, &[&str]); 13] = [
    (
        "search_read",
        false,
        &["domain", "fields", "offset", "limit", "order", "load"],
    ),
    ("read", true, &["fields", "load"]),
    ("search", false, &["domain", "offset", "limit", "order"]),
    ("search_count", false, &["domain", "limit"]),
    ("fields_get", false, &["allfields", "attributes"]),
    (
        "name_search",
        false,
        &["name", "domain", "operator", "limit"],
    ),
    ("create", false, &["vals_list"]),
    ("write", true, &["vals"]),
    ("unlink", true, &[]),
    // Composite methods from tests/support/composite.rs.
    (
        "read_group",
        false,
        &[
            "domain", "fields", "groupby", "offset", "limit", "orderby", "lazy",
        ],
    ),
    ("get_views", false, &["views", "options"]),
    (
        "check_access_rights",
        false,
        &["operation", "raise_exception"],
    ),
    ("get_external_id", true, &[]),
];

fn json2_error(s: &ProtocolState, status: StatusCode, name: &str, message: &str) -> Response {
    let body = json!({"name": name, "message": message, "arguments": [message, status.as_u16()],
                      "context": {}, "debug": format!("Traceback (most recent call last):\n{name}: {message}\n")})
        .to_string();
    s.bytes.fetch_add(body.len(), Ordering::SeqCst);
    (
        status,
        [(
            axum::http::header::CONTENT_TYPE,
            "application/json; charset=utf-8",
        )],
        body,
    )
        .into_response()
}

pub(super) async fn json2_endpoint(
    State(s): State<ProtocolState>,
    axum::extract::Path((model, method)): axum::extract::Path<(String, String)>,
    headers: HeaderMap,
    body: String,
) -> Response {
    s.requests.lock().unwrap().push(body.clone());
    if let Some(status) = s.json2_status {
        return StatusCode::from_u16(status).unwrap().into_response();
    }
    let bearer = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let key = bearer
        .get(..7)
        .filter(|p| p.eq_ignore_ascii_case("bearer "))
        .map(|_| &bearer[7..]);
    if key != Some("fixture-secret") {
        return json2_error(
            &s,
            StatusCode::UNAUTHORIZED,
            "werkzeug.exceptions.Unauthorized",
            "Invalid apikey",
        );
    }
    let Some(db) = headers
        .get("x-odoo-database")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
    else {
        // Multiple databases without dbfilter require a database header.
        return (StatusCode::NOT_FOUND, "<html>404</html>").into_response();
    };
    let Ok(Value::Object(mut params)) = serde_json::from_str::<Value>(&body) else {
        return json2_error(
            &s,
            StatusCode::BAD_REQUEST,
            "werkzeug.exceptions.BadRequest",
            "could not parse the body as json",
        );
    };
    let ids = params.remove("ids").unwrap_or(json!([]));
    let context = params.remove("context").unwrap_or(json!({}));
    let uid = if db == "base-a" { 11 } else { 22 };
    if model == "res.users" && method == "context_get" {
        let body = json!({"lang": "en_US", "tz": false, "uid": uid}).to_string();
        s.bytes.fetch_add(body.len(), Ordering::SeqCst);
        return (
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            body,
        )
            .into_response();
    }
    if model == "missing.model" {
        return json2_error(
            &s,
            StatusCode::NOT_FOUND,
            "werkzeug.exceptions.NotFound",
            &format!("the model '{model}' does not exist"),
        );
    }
    let Some((_, record, names)) = JSON2_SIGNATURES
        .iter()
        .find(|(m, _, _)| *m == method)
        .copied()
    else {
        // Simulator business methods receive IDs and remaining arguments by name.
        if method == "action_confirm" {
            let mut kwargs = params;
            kwargs.insert("context".into(), context);
            let (status, result) = legacy(
                &s.counts,
                headers,
                "object",
                "execute_kw",
                json!([db, uid, "fixture-secret", model, method, [ids], kwargs]),
            )
            .await;
            return json2_result(&s, &method, status, result);
        }
        return json2_error(
            &s,
            StatusCode::NOT_FOUND,
            "werkzeug.exceptions.NotFound",
            &format!("The method '{method}' does not exist on the model '{model}'"),
        );
    };
    if !record && ids.as_array().is_some_and(|a| !a.is_empty()) {
        return json2_error(
            &s,
            StatusCode::UNPROCESSABLE_ENTITY,
            "werkzeug.exceptions.UnprocessableEntity",
            &format!("cannot call {model}.{method} with ids"),
        );
    }
    if let Some(bad) = params.keys().find(|k| !names.contains(&k.as_str())) {
        return json2_error(
            &s,
            StatusCode::UNPROCESSABLE_ENTITY,
            "werkzeug.exceptions.UnprocessableEntity",
            &format!("got an unexpected keyword argument '{bad}'"),
        );
    }
    // Reuse fixture data by matching JSON-RPC positional shape.
    let take = |params: &mut serde_json::Map<String, Value>, k: &str| {
        params.remove(k).unwrap_or(Value::Null)
    };
    let args = match method.as_str() {
        "read" | "unlink" | "get_external_id" => json!([ids]),
        "write" => json!([ids, take(&mut params, "vals")]),
        "name_search" => json!([
            take(&mut params, "name"),
            take(&mut params, "domain"),
            take(&mut params, "operator"),
            take(&mut params, "limit")
        ]),
        "create" => json!([take(&mut params, "vals_list")]),
        _ => json!([]),
    };
    params.insert("context".into(), context);
    let (status, result) = legacy(
        &s.counts,
        headers,
        "object",
        "execute_kw",
        json!([db, uid, "fixture-secret", model, method, args, params]),
    )
    .await;
    json2_result(&s, &method, status, result)
}

fn json2_result(
    s: &ProtocolState,
    method: &str,
    status: StatusCode,
    result: Result<Value, Value>,
) -> Response {
    if !status.is_success() {
        return status.into_response();
    }
    match result {
        Ok(value) => {
            // JSON-2 serializes create recordsets as ID lists.
            let value = match value {
                Value::Number(n) if method == "create" => json!([n]),
                v => v,
            };
            let body = value.to_string();
            s.bytes.fetch_add(body.len(), Ordering::SeqCst);
            (
                [(
                    axum::http::header::CONTENT_TYPE,
                    "application/json; charset=utf-8",
                )],
                body,
            )
                .into_response()
        }
        Err(data) => {
            let name = data["name"].as_str().unwrap_or("builtins.Exception");
            let message = data["message"].as_str().unwrap_or("error");
            let status = if name.ends_with("AccessError") {
                StatusCode::FORBIDDEN
            } else if name.ends_with("UserError") || name.ends_with("ValidationError") {
                StatusCode::UNPROCESSABLE_ENTITY
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            json2_error(s, status, name, message)
        }
    }
}
