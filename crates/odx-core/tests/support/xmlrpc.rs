//! XML-RPC (Odoo 16–19): /xmlrpc/2/<service> envelopes around the shared business logic.
use super::*;

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

pub(super) async fn xmlrpc_endpoint(
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
