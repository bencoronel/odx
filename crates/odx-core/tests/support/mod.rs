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
#[path = "proxy.rs"]
pub mod proxy;

#[derive(Clone, Default)]
pub struct Counts {
    held_databases: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    open_held: Arc<Notify>,
    in_flight: Arc<std::sync::Mutex<InFlight>>,
    response_bytes: Arc<AtomicUsize>,
    headers: Arc<std::sync::Mutex<Vec<HeaderMap>>>,
    writes: Arc<AtomicUsize>,
    logins: Arc<AtomicUsize>,
    queries: Arc<AtomicUsize>,
    request_bytes: Arc<AtomicUsize>,
    lose_response: Arc<AtomicBool>,
    pause_next_write: Arc<AtomicBool>,
    write_started: Arc<Notify>,
    release_write: Arc<Notify>,
    pause_next_login: Arc<AtomicBool>,
    fail_next_login: Arc<AtomicBool>,
    login_started: Arc<Notify>,
    release_login: Arc<Notify>,
    fail_next_execute: Arc<std::sync::Mutex<Option<String>>>,
}
#[allow(dead_code)]
impl Counts {
    pub fn headers(&self) -> Vec<HeaderMap> {
        self.headers.lock().unwrap().clone()
    }
    pub fn writes(&self) -> usize {
        self.writes.load(Ordering::SeqCst)
    }
    pub fn logins(&self) -> usize {
        self.logins.load(Ordering::SeqCst)
    }
    pub fn queries(&self) -> usize {
        self.queries.load(Ordering::SeqCst)
    }
    pub fn request_bytes(&self) -> usize {
        self.request_bytes.load(Ordering::SeqCst)
    }
    pub fn lose_next_response(&self) {
        self.lose_response.store(true, Ordering::SeqCst);
    }
    pub fn pause_next_write(&self) {
        self.pause_next_write.store(true, Ordering::SeqCst);
    }
    pub async fn wait_for_write(&self) {
        self.write_started.notified().await;
    }
    pub fn release_write(&self) {
        self.release_write.notify_one();
    }
    pub fn pause_next_login(&self) {
        self.pause_next_login.store(true, Ordering::SeqCst);
    }
    pub fn fail_next_login(&self) {
        self.fail_next_login.store(true, Ordering::SeqCst);
    }
    pub async fn wait_for_login(&self) {
        self.login_started.notified().await;
    }
    pub fn release_login(&self) {
        self.release_login.notify_one();
    }
    /// Fail the next execute_kw with this logical Odoo error.
    pub fn fail_next_execute(&self, message: &str) {
        *self.fail_next_execute.lock().unwrap() = Some(message.into());
    }
}
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
/// Current and peak execute_kw calls per database.
#[derive(Default)]
struct InFlight {
    now: std::collections::HashMap<String, usize>,
    max: std::collections::HashMap<String, usize>,
    total: usize,
    max_total: usize,
}
struct InFlightGuard {
    counts: Counts,
    db: String,
}
impl Drop for InFlightGuard {
    fn drop(&mut self) {
        let mut f = self.counts.in_flight.lock().unwrap();
        *f.now.get_mut(&self.db).unwrap() -= 1;
        f.total -= 1;
    }
}
#[allow(dead_code)]
impl Counts {
    /// Hold this database's execute_kw calls until release_databases.
    pub fn hold_database(&self, db: &str) {
        self.held_databases.lock().unwrap().insert(db.into());
    }
    pub fn release_databases(&self) {
        self.held_databases.lock().unwrap().clear();
        self.open_held.notify_waiters();
    }
    pub fn in_flight(&self, db: &str) -> usize {
        let f = self.in_flight.lock().unwrap();
        f.now.get(db).copied().unwrap_or(0)
    }
    pub fn in_flight_total(&self) -> usize {
        self.in_flight.lock().unwrap().total
    }
    pub fn max_in_flight(&self, db: &str) -> usize {
        let f = self.in_flight.lock().unwrap();
        f.max.get(db).copied().unwrap_or(0)
    }
    pub fn max_in_flight_total(&self) -> usize {
        self.in_flight.lock().unwrap().max_total
    }
    /// Bytes in simulator JSON-RPC result responses.
    pub fn response_bytes(&self) -> usize {
        self.response_bytes.load(Ordering::SeqCst)
    }
    async fn enter(&self, db: &str) -> InFlightGuard {
        {
            let mut f = self.in_flight.lock().unwrap();
            let now = f.now.entry(db.into()).or_default();
            *now += 1;
            let now = *now;
            let max = f.max.entry(db.into()).or_default();
            *max = (*max).max(now);
            f.total += 1;
            f.max_total = f.max_total.max(f.total);
        }
        let guard = InFlightGuard {
            counts: self.clone(),
            db: db.into(),
        };
        loop {
            // Register before checking to receive later notify_waiters calls.
            let opened = self.open_held.notified();
            if !self.held_databases.lock().unwrap().contains(db) {
                return guard;
            }
            opened.await;
        }
    }
}
#[allow(dead_code)]
pub struct Simulator {
    pub url: String,
    // Shared by all test binaries, including those that do not read counters.
    #[allow(dead_code)]
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
    let result = if service == "common" && method == "login" {
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
#[allow(dead_code)]
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
#[allow(dead_code)]
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

#[allow(dead_code)]
pub async fn protocol_simulator(major: u8) -> ProtocolSimulator {
    protocol_simulator_with_status(major, None, None).await
}

/// Model missing/blocked endpoints separately from invalid credentials.
#[allow(dead_code)]
pub async fn protocol_simulator_with_status(
    major: u8,
    jsonrpc_status: Option<u16>,
    json2_status: Option<u16>,
) -> ProtocolSimulator {
    protocol_simulator_with_version_status(major, jsonrpc_status, json2_status, None).await
}

/// `version_status` answers `GET /web/version` with that status and a login redirect, as some Odoo 16 servers do.
#[allow(dead_code)]
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

// XML-RPC

struct Node {
    name: String,
    children: Vec<Node>,
    text: String,
}

fn parse_xml(input: &str) -> Option<Node> {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_str(input);
    reader.config_mut().expand_empty_elements = true;
    let mut stack = vec![Node {
        name: String::new(),
        children: vec![],
        text: String::new(),
    }];
    loop {
        match reader.read_event().ok()? {
            Event::Start(e) => stack.push(Node {
                name: e.name().as_ref().to_owned(),
                children: vec![],
                text: String::new(),
            }),
            Event::End(_) => {
                let node = stack.pop()?;
                stack.last_mut()?.children.push(node);
            }
            Event::Text(t) => stack.last_mut()?.text.push_str(&t.xml10_content()),
            Event::GeneralRef(r) => {
                let c = match r.resolve_char_ref().ok()? {
                    Some(c) => c.to_string(),
                    None => quick_xml::escape::resolve_predefined_entity(&r)?.to_owned(),
                };
                stack.last_mut()?.text.push_str(&c);
            }
            Event::DocType(_) => return None,
            Event::Eof => break,
            _ => {}
        }
    }
    stack.pop()?.children.pop()
}

/// Like Python xmlrpc.client.loads, accept all types including nil.
fn xml_value(node: &Node) -> Option<Value> {
    let Some(typed) = node.children.first() else {
        return Some(Value::String(node.text.clone()));
    };
    let t = typed.text.trim();
    Some(match typed.name.as_str() {
        "int" | "i4" | "i8" => json!(t.parse::<i64>().ok()?),
        "boolean" => json!(t == "1"),
        "string" => json!(typed.text),
        "double" => json!(t.parse::<f64>().ok()?),
        "nil" => Value::Null,
        "dateTime.iso8601" | "base64" => json!(t),
        "array" => Value::Array(
            typed
                .children
                .first()?
                .children
                .iter()
                .map(xml_value)
                .collect::<Option<_>>()?,
        ),
        "struct" => {
            let mut map = serde_json::Map::new();
            for m in &typed.children {
                map.insert(
                    m.children.first()?.text.clone(),
                    xml_value(m.children.get(1)?)?,
                );
            }
            Value::Object(map)
        }
        _ => return None,
    })
}

/// Like OdooMarshaller with allow_none=False, reject None and integers outside 32 bits.
fn xml_encode(v: &Value, out: &mut String) -> Result<(), &'static str> {
    match v {
        Value::Null => return Err("TypeError: cannot marshal None unless allow_none is enabled"),
        Value::Bool(b) => out.push_str(&format!(
            "<value><boolean>{}</boolean></value>",
            u8::from(*b)
        )),
        Value::Number(n) => match n.as_i64() {
            Some(i) if i32::try_from(i).is_ok() => {
                out.push_str(&format!("<value><int>{i}</int></value>"))
            }
            Some(_) => return Err("OverflowError: int exceeds XML-RPC limits"),
            None if n.is_f64() => out.push_str(&format!(
                "<value><double>{}</double></value>",
                n.as_f64().unwrap()
            )),
            None => return Err("OverflowError: int exceeds XML-RPC limits"),
        },
        Value::String(s) => {
            // Odoo removes control characters before escaping.
            let clean: String = s
                .chars()
                .filter(|c| !c.is_control() || matches!(c, '\t' | '\n' | '\r'))
                .collect();
            out.push_str("<value><string>");
            out.push_str(&quick_xml::escape::escape(clean.as_str()));
            out.push_str("</string></value>");
        }
        Value::Array(items) => {
            out.push_str("<value><array><data>\n");
            for item in items {
                xml_encode(item, out)?;
                out.push('\n');
            }
            out.push_str("</data></array></value>");
        }
        Value::Object(map) => {
            out.push_str("<value><struct>\n");
            for (k, item) in map {
                out.push_str(&format!(
                    "<member>\n<name>{}</name>\n",
                    quick_xml::escape::escape(k.as_str())
                ));
                xml_encode(item, out)?;
                out.push_str("\n</member>\n");
            }
            out.push_str("</struct></value>");
        }
    }
    Ok(())
}

fn xml_response(s: &ProtocolState, body: String) -> Response {
    s.bytes.fetch_add(body.len(), Ordering::SeqCst);
    // Odoo faults also use HTTP 200 and text/xml.
    (
        [(axum::http::header::CONTENT_TYPE, "text/xml; charset=utf-8")],
        body,
    )
        .into_response()
}

fn xml_fault(s: &ProtocolState, code: i64, message: &str) -> Response {
    let mut fault = String::new();
    xml_encode(
        &json!({"faultCode": code, "faultString": message}),
        &mut fault,
    )
    .unwrap();
    xml_response(
        s,
        format!(
            "<?xml version=\"1.0\"?>\n<methodResponse>\n<fault>\n{fault}\n</fault>\n</methodResponse>\n"
        ),
    )
}

async fn xmlrpc_endpoint(
    State(s): State<ProtocolState>,
    axum::extract::Path(service): axum::extract::Path<String>,
    headers: HeaderMap,
    body: String,
) -> Response {
    s.requests.lock().unwrap().push(body.clone());
    let Some(call) = parse_xml(&body).filter(|n| n.name == "methodCall") else {
        return xml_fault(
            &s,
            1,
            "Traceback (most recent call last):\nxml.parsers.expat.ExpatError: syntax error",
        );
    };
    let method = call
        .children
        .iter()
        .find(|n| n.name == "methodName")
        .map(|n| n.text.clone())
        .unwrap_or_default();
    let params: Option<Vec<Value>> = call
        .children
        .iter()
        .find(|n| n.name == "params")
        .map(|p| {
            p.children
                .iter()
                .filter_map(|param| param.children.first().map(xml_value))
                .collect()
        })
        .unwrap_or(Some(vec![]));
    let Some(params) = params else {
        return xml_fault(
            &s,
            1,
            "Traceback (most recent call last):\nValueError: bad value",
        );
    };
    let result = if service == "common" && method == "version" {
        Ok(version_info(s.major))
    } else {
        let (status, result) = legacy(&s.counts, headers, &service, &method, json!(params)).await;
        if !status.is_success() {
            return status.into_response();
        }
        match result {
            // common.login returns False for invalid credentials.
            Ok(Value::Null) if method == "login" => Ok(json!(false)),
            // get_external_id returns integer keys; Python's Marshaller, inherited by OdooMarshaller, requires strings for dump_struct.
            Ok(_) if params.get(4) == Some(&json!("get_external_id")) => {
                return xml_fault(
                    &s,
                    1,
                    "Traceback (most recent call last):\n  File \"/usr/lib/python3/xmlrpc/client.py\", line 1\nTypeError: dictionary key must be string\n",
                );
            }
            other => other,
        }
    };
    match result {
        Ok(value) => {
            let mut out = String::new();
            match xml_encode(&value, &mut out) {
                Ok(()) => xml_response(
                    &s,
                    format!(
                        "<?xml version=\"1.0\"?>\n<methodResponse>\n<params>\n<param>\n{out}\n</param>\n</params>\n</methodResponse>\n"
                    ),
                ),
                Err(e) => xml_fault(
                    &s,
                    1,
                    &format!(
                        "Traceback (most recent call last):\n  File \"/odoo/rpc.py\", line 1\n{e}\n"
                    ),
                ),
            }
        }
        Err(data) => {
            let name = data["name"].as_str().unwrap_or("builtins.Exception");
            let message = data["message"].as_str().unwrap_or("error");
            // xmlrpc_handle_exception_int: AccessError 4, UserError 2, others 1 with traceback.
            if name.ends_with("AccessError") {
                xml_fault(&s, 4, message)
            } else if name.ends_with("UserError") || name.ends_with("ValidationError") {
                xml_fault(&s, 2, message)
            } else {
                let short = name.rsplit('.').next().unwrap_or(name);
                xml_fault(
                    &s,
                    1,
                    &format!(
                        "Traceback (most recent call last):\n  File \"/odoo/service/model.py\", line 1\n{short}: {message}\n"
                    ),
                )
            }
        }
    }
}

// JSON-2

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

async fn json2_endpoint(
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

/// HTTPS server with the test certificate; count TLS connections receiving decrypted bytes.
#[allow(dead_code)]
pub async fn untrusted_tls_server() -> (u16, Arc<AtomicUsize>) {
    use tokio_rustls::rustls::{
        ServerConfig,
        pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer},
    };
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(cert.signing_key.serialize_der()));
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
