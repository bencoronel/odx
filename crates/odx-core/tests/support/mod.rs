//! Odoo simulators for the tests: JSON-RPC here, XML-RPC and JSON-2 in their own files, sharing the call
//! counters and the business logic of `legacy`. Each test binary uses part of it.
#![allow(dead_code)]
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
};
use serde_json::{Value, json};
use std::{
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};
use tokio::{sync::Notify, task::JoinHandle};

#[path = "composite.rs"]
mod composite;
#[path = "counts.rs"]
mod counts;
#[path = "json2.rs"]
mod json2;
#[path = "proxy.rs"]
pub mod proxy;
#[path = "xmlrpc.rs"]
mod xmlrpc;

pub use counts::Counts;
use json2::json2_endpoint;
use xmlrpc::xmlrpc_endpoint;

/// Odoo-shaped JSON-RPC server exception.
fn odoo_error(id: &Value, name: &str, message: &str) -> Response {
    Json(json!({"jsonrpc":"2.0","id":id,"error":{"code":200,"message":"Odoo Server Error","data":{"name":name,"message":message}}})).into_response()
}
/// Simulator ir.model entries sorted by model.
const MODELS: [(i64, &str, &str); 4] = [
    (2, "res.company", "Companies"),
    (1, "res.partner", "Contact"),
    (3, "res.users", "Users"),
    (4, "sale.order", "Sales Order"),
];

/// A core with a new, unlocked vault in `tmp`; keep `tmp` alive as long as the core.
pub async fn unlocked_core(tmp: &tempfile::TempDir) -> std::sync::Arc<odx_core::Core> {
    let core = odx_core::Core::new(tmp.path().join("vault.sqlite3")).unwrap();
    core.create("fictional master password").await.unwrap();
    core
}

/// The usual test environment: JSON-RPC, database base-a, user fixture-user, no context.
pub fn draft(name: &str, url: &str) -> odx_core::EnvironmentDraft {
    odx_core::EnvironmentDraft {
        name: name.into(),
        url: url.into(),
        database: "base-a".into(),
        login: "fixture-user".into(),
        protocol: odx_core::Protocol::LegacyJsonrpc,
        context: json!({}),
        secret: "fixture-secret".into(),
    }
}

pub struct Simulator {
    pub url: String,
    // Shared by all test binaries, including those that do not read counters.
    pub counts: Counts,
    _task: JoinHandle<()>,
}
async fn jsonrpc(
    State(counts): State<Counts>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    counts.headers.lock().unwrap().push(headers);
    counts.request_bytes.fetch_add(
        serde_json::to_vec(&body).map_or(0, |bytes| bytes.len()),
        Ordering::SeqCst,
    );
    let p = &body["params"];
    let method = p["method"].as_str().unwrap_or("");
    let service = p["service"].as_str().unwrap_or("");
    let a = p["args"].as_array().cloned().unwrap_or_default();
    let result = if service == "db" && method == "list" {
        json!(["base-a", "base-b"])
    } else if service == "common" && method == "login" {
        counts.logins.fetch_add(1, Ordering::SeqCst);
        if counts.pause_next_login.swap(false, Ordering::SeqCst) {
            counts.login_started.notify_one();
            counts.release_login.notified().await;
        }
        if counts.fail_next_login.swap(false, Ordering::SeqCst) {
            Value::Null
        } else if a.get(2).and_then(Value::as_str) == Some("fixture-secret") {
            json!(if a.first() == Some(&json!("base-a")) {
                11
            } else {
                22
            })
        } else {
            Value::Null
        }
    } else if service == "object" && method == "execute_kw" {
        let db = a.first().and_then(Value::as_str).unwrap_or("");
        let m = a.get(4).and_then(Value::as_str).unwrap_or("");
        let _in_flight = counts.enter(db).await;
        let failure = counts.fail_next_execute.lock().unwrap().take();
        if let Some(message) = failure {
            counts.queries.fetch_add(1, Ordering::SeqCst);
            return odoo_error(&body["id"], "odoo.exceptions.AccessError", &message);
        }
        // Odoo could not serialize a result value, such as computed field bytes.
        if a.get(3) == Some(&json!("x.unserializable")) {
            counts.queries.fetch_add(1, Ordering::SeqCst);
            return odoo_error(
                &body["id"],
                "builtins.TypeError",
                "Object of type bytes is not JSON serializable",
            );
        }
        // Python encodes float('nan') as NaN, which is invalid JSON.
        if a.get(3) == Some(&json!("x.nan")) {
            counts.queries.fetch_add(1, Ordering::SeqCst);
            let raw = format!(
                r#"{{"jsonrpc":"2.0","id":{},"result":[{{"id":1,"ratio":NaN}}]}}"#,
                body["id"]
            );
            return (
                [(axum::http::header::CONTENT_TYPE, "application/json")],
                raw,
            )
                .into_response();
        }
        if a.get(3) == Some(&json!("broken.model")) {
            counts.queries.fetch_add(1, Ordering::SeqCst);
            return Json(json!({"jsonrpc":"2.0","id":body["id"],"error":{"code":200,"message":"Odoo Server Error","data":{"message":"Model broken.model does not exist"}}})).into_response();
        }
        // Composite/file models from tests/support/composite.rs.
        let model = a.get(3).and_then(Value::as_str).unwrap_or("");
        let empty = json!({});
        let kwargs = a.get(6).unwrap_or(&empty);
        let positional = a
            .get(5)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if let Some(answer) = composite::respond(db, model, m, &positional, kwargs) {
            counts.queries.fetch_add(1, Ordering::SeqCst);
            return match answer {
                Ok(result) => {
                    let response = json!({"jsonrpc":"2.0","id":body["id"],"result":result});
                    counts.response_bytes.fetch_add(
                        serde_json::to_vec(&response).map_or(0, |bytes| bytes.len()),
                        Ordering::SeqCst,
                    );
                    Json(response).into_response()
                }
                Err(message) => odoo_error(&body["id"], "odoo.exceptions.AccessError", &message),
            };
        }
        match m {
            // Return limit rows of 1000 characters to exercise output bounds.
            "search_read" if a.get(3) == Some(&json!("limits.big")) => {
                counts.queries.fetch_add(1, Ordering::SeqCst);
                let limit = a.get(6).and_then(|k| k["limit"].as_u64()).unwrap_or(100);
                json!(
                    (1..=limit)
                        .map(|i| json!({"id":i,"display_name":"x".repeat(1000)}))
                        .collect::<Vec<_>>()
                )
            }
            // Exceed the Odoo client's 16 MiB response limit.
            "search_read" if a.get(3) == Some(&json!("limits.huge")) => {
                counts.queries.fetch_add(1, Ordering::SeqCst);
                json!("x".repeat(16 * 1024 * 1024 + 1))
            }
            "search_read" if a.get(3) == Some(&json!("ir.model")) => {
                counts.queries.fetch_add(1, Ordering::SeqCst);
                // Expected domain: ["|", ["model","ilike",q], ["name","ilike",q]].
                let kw = a.get(6).cloned().unwrap_or(json!({}));
                let q = kw["domain"][1][2].as_str().unwrap_or("").to_lowercase();
                let offset = kw["offset"].as_u64().unwrap_or(0) as usize;
                let limit = kw["limit"].as_u64().unwrap_or(100) as usize;
                json!(
                    MODELS
                        .iter()
                        .filter(|(_, model, name)| model.contains(&q)
                            || name.to_lowercase().contains(&q))
                        .skip(offset)
                        .take(limit)
                        .map(|(id, model, name)| json!({"id":id,"model":model,"name":name}))
                        .collect::<Vec<_>>()
                )
            }
            // Mixed types and long multibyte text for compact output.
            "search_read" if a.get(3) == Some(&json!("x.types")) => {
                counts.queries.fetch_add(1, Ordering::SeqCst);
                json!([{"id":1,"name":"ñ€😀".repeat(100),"flag":false,"zero":0,"empty":"",
                        "none":null,"big":u64::MAX,"float":1.5e300,"neg":-0.0,
                        "nested":{"a":[1,{"b":"ç"}]},"m2o":[1,"Main"]}])
            }
            "search_read" => {
                counts.queries.fetch_add(1, Ordering::SeqCst);
                json!([{"id":1,"display_name":format!("contact-{db}"),"context":a.get(6).and_then(|v|v.get("context")).cloned().unwrap_or(json!({}))}])
            }
            "write" => {
                counts.writes.fetch_add(1, Ordering::SeqCst);
                if counts.pause_next_write.swap(false, Ordering::SeqCst) {
                    counts.write_started.notify_one();
                    counts.release_write.notified().await;
                }
                if counts.lose_response.swap(false, Ordering::SeqCst) {
                    return StatusCode::BAD_GATEWAY.into_response();
                }
                json!(true)
            }
            "read" if a.get(3) == Some(&json!("res.users")) => {
                counts.queries.fetch_add(1, Ordering::SeqCst);
                json!([{"id":11,"login":"fixture-user","name":"Fixture User","company_id":[1,"Main"],"company_ids":[1]}])
            }
            "read" => {
                counts.queries.fetch_add(1, Ordering::SeqCst);
                let ids = a
                    .get(5)
                    .and_then(|v| v.get(0))
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                json!(
                    ids.iter()
                        .filter(|i| i.as_i64().is_some_and(|i| i <= 3))
                        .map(|i| json!({"id":i,"display_name":format!("record-{i}")}))
                        .collect::<Vec<_>>()
                )
            }
            "search" => {
                counts.queries.fetch_add(1, Ordering::SeqCst);
                let kw = a.get(6).cloned().unwrap_or(json!({}));
                let offset = kw["offset"].as_u64().unwrap_or(0) as usize;
                let limit = kw["limit"].as_u64().unwrap_or(100) as usize;
                json!((1..=5).skip(offset).take(limit).collect::<Vec<_>>())
            }
            "search_count" => {
                counts.queries.fetch_add(1, Ordering::SeqCst);
                json!(5)
            }
            "fields_get" => {
                counts.queries.fetch_add(1, Ordering::SeqCst);
                let mut fields = json!({"name":{"type":"char","string":"Name","required":true,"readonly":false,"store":true},
                       "email":{"type":"char","string":"Email","required":false,"readonly":false,"store":true},
                       "company_id":{"type":"many2one","relation":"res.company","string":"Company","required":false,"readonly":false,"store":true}});
                // A many2many only when asked for by name, so existing field listings stay unchanged.
                let kw = a.get(6).cloned().unwrap_or(json!({}));
                if kw["allfields"]
                    .as_array()
                    .is_some_and(|names| names.contains(&json!("tag_ids")))
                {
                    fields["tag_ids"] = json!({"type":"many2many","relation":"res.partner.category","string":"Tags"});
                }
                fields
            }
            // Odoo 16–19 name_search(name, args|domain, operator, limit): the second parameter is args through 18 and domain in 19.
            "name_search" => {
                counts.queries.fetch_add(1, Ordering::SeqCst);
                let kw = a.get(6).cloned().unwrap_or(json!({}));
                if let Some(name) = ["args", "domain"].into_iter().find(|k| kw.get(k).is_some()) {
                    return Json(json!({"jsonrpc":"2.0","id":body["id"],"error":{"code":200,"message":"Odoo Server Error","data":{"message":format!("name_search() got an unexpected keyword argument '{name}'")}}})).into_response();
                }
                let limit = a
                    .get(5)
                    .and_then(|v| v.get(3))
                    .and_then(Value::as_u64)
                    .unwrap_or(100) as usize;
                let all = json!([[1, "Alpha"], [2, "Alphabet"], [3, "Alps"]]);
                Value::Array(
                    all.as_array()
                        .unwrap()
                        .iter()
                        .take(limit)
                        .cloned()
                        .collect(),
                )
            }
            "create" => {
                counts.writes.fetch_add(1, Ordering::SeqCst);
                if a.get(5).and_then(|v| v.get(0)).is_some_and(Value::is_array) {
                    json!([7, 8])
                } else {
                    json!(7)
                }
            }
            "unlink" | "action_confirm" => {
                counts.writes.fetch_add(1, Ordering::SeqCst);
                json!(true)
            }
            // Like account.move.action_post: commits and returns None.
            "action_post" => {
                counts.writes.fetch_add(1, Ordering::SeqCst);
                Value::Null
            }
            _ => json!([]),
        }
    } else {
        Value::Null
    };
    let response = json!({"jsonrpc":"2.0","id":body["id"],"result":result});
    counts.response_bytes.fetch_add(
        serde_json::to_vec(&response).map_or(0, |bytes| bytes.len()),
        Ordering::SeqCst,
    );
    Json(response).into_response()
}
pub async fn simulator() -> Simulator {
    let counts = Counts::default();
    let app = Router::new()
        .route("/jsonrpc", post(jsonrpc))
        .with_state(counts.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Simulator {
        url: format!("http://{addr}"),
        counts,
        _task: task,
    }
}

// Versioned XML-RPC (16–19) and JSON-2 (19) simulators reuse JSON-RPC business logic. Envelope sources: odoo/addons/base/controllers/rpc.py (16–18), addons/rpc/controllers/{xmlrpc,json2}.py and odoo/http.py (19).

/// Simulator for one Odoo major version and its available protocols.
pub struct ProtocolSimulator {
    pub url: String,
    pub counts: Counts,
    /// XML-RPC, JSON-2 and /web/version response body bytes.
    pub bytes: Arc<AtomicUsize>,
    /// Captured XML-RPC/JSON-2 request bodies for shape assertions.
    pub requests: Arc<std::sync::Mutex<Vec<String>>>,
    _task: JoinHandle<()>,
}

#[derive(Clone)]
struct ProtocolState {
    counts: Counts,
    major: u8,
    bytes: Arc<AtomicUsize>,
    requests: Arc<std::sync::Mutex<Vec<String>>>,
    jsonrpc_status: Option<u16>,
    json2_status: Option<u16>,
}

pub async fn protocol_simulator(major: u8) -> ProtocolSimulator {
    protocol_simulator_with_status(major, None, None).await
}

/// Model missing/blocked endpoints separately from invalid credentials.
pub async fn protocol_simulator_with_status(
    major: u8,
    jsonrpc_status: Option<u16>,
    json2_status: Option<u16>,
) -> ProtocolSimulator {
    protocol_simulator_with_version_status(major, jsonrpc_status, json2_status, None).await
}

/// `version_status` answers `GET /web/version` with that status and a login redirect, as some Odoo 16 servers do.
pub async fn protocol_simulator_with_version_status(
    major: u8,
    jsonrpc_status: Option<u16>,
    json2_status: Option<u16>,
    version_status: Option<u16>,
) -> ProtocolSimulator {
    let state = ProtocolState {
        counts: Counts::default(),
        major,
        bytes: Arc::default(),
        requests: Arc::default(),
        jsonrpc_status,
        json2_status,
    };
    let mut app = Router::new()
        .route("/jsonrpc", post(versioned_jsonrpc))
        .route("/xmlrpc/2/{service}", post(xmlrpc_endpoint));
    if let Some(status) = version_status {
        app = app.route(
            "/web/version",
            axum::routing::get(move || async move {
                (
                    StatusCode::from_u16(status).unwrap(),
                    [(axum::http::header::LOCATION, "/web/login")],
                )
                    .into_response()
            }),
        );
    } else if major >= 19 {
        app = app
            .route("/json/2/{model}/{method}", post(json2_endpoint))
            .route(
                "/web/version",
                axum::routing::get(|State(s): State<ProtocolState>| async move {
                    let body =
                        json!({"version_info":[19,0,0,"final",0,""],"version":"19.0"}).to_string();
                    s.bytes.fetch_add(body.len(), Ordering::SeqCst);
                    (
                        [(axum::http::header::CONTENT_TYPE, "application/json")],
                        body,
                    )
                        .into_response()
                }),
            );
    }
    let app = app.with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    ProtocolSimulator {
        url: format!("http://{addr}"),
        counts: state.counts,
        bytes: state.bytes,
        requests: state.requests,
        _task: task,
    }
}

fn version_info(major: u8) -> Value {
    json!({"server_version": format!("{major}.0"), "server_version_info": [major, 0, 0, "final", 0, ""],
           "server_serie": format!("{major}.0"), "protocol_version": 1})
}

async fn versioned_jsonrpc(
    State(s): State<ProtocolState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if let Some(status) = s.jsonrpc_status {
        return StatusCode::from_u16(status).unwrap().into_response();
    }
    if body["params"]["service"] == "common" && body["params"]["method"] == "version" {
        return Json(json!({"jsonrpc":"2.0","id":body["id"],"result":version_info(s.major)}))
            .into_response();
    }
    jsonrpc(State(s.counts), headers, Json(body)).await
}

/// Run through the JSON-RPC handler; return (status, result or error).
async fn legacy(
    counts: &Counts,
    headers: HeaderMap,
    service: &str,
    method: &str,
    args: Value,
) -> (StatusCode, Result<Value, Value>) {
    let body = json!({"jsonrpc":"2.0","id":1,"method":"call","params":{"service":service,"method":method,"args":args}});
    let response = jsonrpc(State(counts.clone()), headers, Json(body)).await;
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    match value.get("error") {
        Some(e) => (status, Err(e["data"].clone())),
        None => (status, Ok(value["result"].clone())),
    }
}

/// HTTPS server with the test certificate; count TLS connections receiving decrypted bytes.
pub async fn untrusted_tls_server() -> (u16, Arc<AtomicUsize>) {
    use tokio_rustls::rustls::{
        ServerConfig,
        pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer},
    };
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(cert.key_pair.serialize_der()));
    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert.cert.der().clone()], key)
        .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let decrypted = Arc::new(AtomicUsize::new(0));
    let seen = decrypted.clone();
    tokio::spawn(async move {
        while let Ok((tcp, _)) = listener.accept().await {
            if let Ok(mut tls) = acceptor.accept(tcp).await {
                use tokio::io::AsyncReadExt;
                let mut buf = [0u8; 1];
                if tls.read(&mut buf).await.unwrap_or(0) > 0 {
                    seen.fetch_add(1, Ordering::SeqCst);
                }
            }
        }
    });
    (port, decrypted)
}
