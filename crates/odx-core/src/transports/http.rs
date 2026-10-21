//! Loopback HTTP API: POST http://127.0.0.1:8770/api/v1/<tool> with Bearer tokens. No cookies, CORS or admin routes. Reject every Origin; no browser clients are supported.
use crate::{
    config::validate_json,
    error::OdxError,
    execution::Core,
    output::{self, Format, Outcome, ToolError},
    tools::catalog::TOOLS,
};
use axum::{
    Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Path, State, rejection::BytesRejection},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::post,
};
use serde_json::Value;
use std::{future::Future, net::SocketAddr, sync::Arc};
use tokio::net::TcpListener;
use uuid::Uuid;

pub const DEFAULT_PORT: u16 = 8770;
const MAX_BODY: usize = 1024 * 1024;

#[derive(Clone)]
struct Api {
    core: Arc<Core>,
    hosts: Arc<[String; 2]>,
}

/// Bind loopback; report occupied ports without choosing another.
pub async fn bind(port: u16) -> Result<TcpListener, String> {
    TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], port)))
        .await
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::AddrInUse => format!("port {port} is already in use"),
            _ => format!("could not bind 127.0.0.1:{port}: {e}"),
        })
}

pub fn router(core: Arc<Core>, port: u16) -> Router {
    let api = Api {
        core,
        hosts: Arc::new([format!("127.0.0.1:{port}"), format!("localhost:{port}")]),
    };
    Router::new()
        .route("/api/v1/{tool}", post(call))
        .fallback(|| async {
            failure(
                &ToolError::invalid("unknown route; use POST /api/v1/<tool>"),
                StatusCode::NOT_FOUND,
            )
        })
        .layer(DefaultBodyLimit::max(MAX_BODY))
        .with_state(api)
}

/// Serve until shutdown completes on vault lock or exit.
pub async fn serve(
    core: Arc<Core>,
    listener: TcpListener,
    shutdown: impl Future<Output = ()> + Send + 'static,
) {
    let port = listener
        .local_addr()
        .map(|a| a.port())
        .unwrap_or(DEFAULT_PORT);
    let _ = axum::serve(listener, router(core, port))
        .with_graceful_shutdown(shutdown)
        .await;
}

fn failure(err: &ToolError, status: StatusCode) -> Response {
    let request_id = Uuid::new_v4().to_string();
    let mut response = (status, axum::Json(err.envelope(&request_id))).into_response();
    if status == StatusCode::UNAUTHORIZED {
        response
            .headers_mut()
            .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
    }
    response
}
fn reject(err: ToolError) -> Response {
    let status =
        StatusCode::from_u16(err.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    failure(&err, status)
}
fn forbidden(message: &str) -> Response {
    reject(ToolError::new("forbidden", message, Outcome::NotSent))
}

async fn call(
    State(api): State<Api>,
    Path(tool): Path<String>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Response {
    // Bound body reads before authentication.
    let body = match body {
        Ok(body) => body,
        Err(rejection) if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE => {
            return reject(ToolError::new(
                "request_too_large",
                "body exceeds 1 MiB",
                Outcome::NotSent,
            ));
        }
        Err(rejection) => return reject(ToolError::invalid(rejection.body_text())),
    };
    // Require one exact Host header to resist DNS rebinding.
    let hosts: Vec<_> = headers.get_all(header::HOST).iter().collect();
    if hosts.len() != 1
        || !hosts[0]
            .to_str()
            .is_ok_and(|h| api.hosts.iter().any(|a| a == h))
    {
        return forbidden("Host not allowed");
    }
    if headers.contains_key(header::ORIGIN) {
        return forbidden("Origin requests are forbidden; this API does not support browsers");
    }
    let auth: Vec<_> = headers.get_all(header::AUTHORIZATION).iter().collect();
    let token = match auth.as_slice() {
        [value] => value
            .to_str()
            .ok()
            .and_then(|v| v.strip_prefix("Bearer "))
            .unwrap_or_default(),
        _ => "",
    };
    if token.is_empty() {
        return reject(ToolError::new(
            "unauthenticated",
            "missing Authorization: Bearer <token>",
            Outcome::NotSent,
        ));
    }
    let identity = match api.core.authenticate_client(token).await {
        Ok(identity) => identity,
        Err(OdxError::Locked) => return reject(OdxError::Locked.into()),
        Err(_) => return reject(OdxError::Unauthorized.into()),
    };
    let json = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.split(';')
                .next()
                .is_some_and(|m| m.trim().eq_ignore_ascii_case("application/json"))
        });
    if !json {
        return reject(ToolError::invalid("Content-Type must be application/json"));
    }
    if !TOOLS.contains(&tool.as_str()) {
        return failure(
            &ToolError::invalid(format!("unknown tool: {tool}")),
            StatusCode::NOT_FOUND,
        );
    }
    let arguments = match validate_json(&body) {
        Ok(v @ Value::Object(_)) => v,
        Ok(_) => return reject(ToolError::invalid("body must be a JSON object")),
        Err(e) => return reject(ToolError::invalid(e.to_string())),
    };
    match api.core.run_tool(&identity, &tool, arguments).await {
        Ok(mut out) => {
            out.format = Format::Json; // The API always returns JSON.
            match output::fit(out) {
                Ok((out, _)) => (StatusCode::OK, axum::Json(out.envelope())).into_response(),
                Err(err) => reject(err),
            }
        }
        Err(err) => reject(err),
    }
}
