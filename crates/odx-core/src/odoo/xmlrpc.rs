//! Odoo 16–19 XML-RPC: /xmlrpc/2/<service> with integer faultCode. Sources: odoo/addons/base/controllers/rpc.py (16–18) and addons/rpc/controllers/xmlrpc.py (19). Odoo uses allow_none=False and textual dates/binaries; also accept nil, i8, dateTime.iso8601 and base64. Reject DTDs and custom entities; allow predefined and numeric XML references.
use super::jsonrpc::{OdooClient, read_limited, scrub};
use crate::{
    config::Environment,
    error::{OdxError, Result},
};
use quick_xml::{Reader, events::Event};
use serde_json::{Map, Number, Value};
use std::fmt::Write as _;
use zeroize::Zeroizing;

/// Maximum response element nesting.
const MAX_DEPTH: usize = 128;

pub(crate) enum Param<'a> {
    Str(&'a str),
    Int(i64),
    Json(&'a Value),
}

/// Return the XML-RPC value and response bytes within max_response. Faults become redacted Remote errors even with HTTP 200.
pub(crate) async fn call(
    client: &OdooClient,
    env: &Environment,
    service: &str,
    method: &str,
    params: &[Param<'_>],
    max_response: usize,
) -> Result<(Value, usize)> {
    let body = bytes::Bytes::from_owner(crate::security::WipeBody(Zeroizing::new(
        encode_call(method, params)?.into_bytes(),
    )));
    let headers = crate::headers::request_headers(&env.headers)?;
    let response = client
        .send(env, |http| {
            http.post(format!("{}/xmlrpc/2/{service}", env.url))
                .header(reqwest::header::CONTENT_TYPE, "text/xml; charset=utf-8")
                .headers(headers.clone())
                .body(body.clone())
                .timeout(client.request_timeout())
        })
        .await?;
    if !response.status().is_success() {
        return Err(OdxError::HttpStatus {
            status: response.status().as_u16(),
        });
    }
    let bytes = read_limited(response, env, max_response).await?;
    let size = bytes.len();
    match decode_response(&bytes)? {
        Ok(value) => Ok((value, size)),
        Err(fault) => Err(OdxError::Remote(scrub(&fault_message(&fault), env))),
    }
}

/// Use faultString, or its last line for faultCode 1 tracebacks.
fn fault_message(fault: &Value) -> String {
    let text = fault
        .get("faultString")
        .and_then(Value::as_str)
        .unwrap_or("remote error");
    if text.contains("Traceback (most recent call last)")
        && let Some(line) = text.lines().rev().find(|l| !l.trim().is_empty())
    {
        return line.trim().to_owned();
    }
    text.trim().to_owned()
}

fn encode_call(method: &str, params: &[Param<'_>]) -> Result<String> {
    let mut out = String::from("<?xml version=\"1.0\"?><methodCall><methodName>");
    push_text(&mut out, method)?;
    out.push_str("</methodName><params>");
    for p in params {
        out.push_str("<param>");
        match p {
            Param::Str(s) => {
                out.push_str("<value><string>");
                push_text(&mut out, s)?;
                out.push_str("</string></value>");
            }
            Param::Int(i) => encode_int(&mut out, *i),
            Param::Json(v) => encode_value(&mut out, v, 0)?,
        }
        out.push_str("</param>");
    }
    out.push_str("</params></methodCall>");
    Ok(out)
}

fn encode_int(out: &mut String, i: i64) {
    // Python accepts i8; Odoo returns only 32-bit integers.
    let tag = if i32::try_from(i).is_ok() {
        "int"
    } else {
        "i8"
    };
    let _ = write!(out, "<value><{tag}>{i}</{tag}></value>");
}

fn encode_value(out: &mut String, v: &Value, depth: usize) -> Result<()> {
    if depth > MAX_DEPTH {
        return Err(OdxError::validation(
            "validation.console.arguments_too_deep",
            "arguments nested too deeply",
        ));
    }
    match v {
        Value::Null => out.push_str("<value><nil/></value>"),
        Value::Bool(b) => {
            let _ = write!(out, "<value><boolean>{}</boolean></value>", u8::from(*b));
        }
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                encode_int(out, i);
            } else if let Some(f) = n.as_f64().filter(|_| n.is_f64()) {
                let _ = write!(out, "<value><double>{f:?}</double></value>");
            } else {
                return Err(OdxError::validation(
                    "validation.console.xmlrpc_integer_out_of_range",
                    "integer outside the XML-RPC range",
                ));
            }
        }
        Value::String(s) => {
            out.push_str("<value><string>");
            push_text(out, s)?;
            out.push_str("</string></value>");
        }
        Value::Array(items) => {
            out.push_str("<value><array><data>");
            for item in items {
                encode_value(out, item, depth + 1)?;
            }
            out.push_str("</data></array></value>");
        }
        Value::Object(map) => {
            out.push_str("<value><struct>");
            for (k, item) in map {
                out.push_str("<member><name>");
                push_text(out, k)?;
                out.push_str("</name>");
                encode_value(out, item, depth + 1)?;
                out.push_str("</member>");
            }
            out.push_str("</struct></value>");
        }
    }
    Ok(())
}

/// Escape XML 1.0 text, rejecting unsupported control characters. Encode carriage returns as references to prevent normalization.
fn push_text(out: &mut String, s: &str) -> Result<()> {
    for c in s.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '\r' => out.push_str("&#13;"),
            '\t' | '\n' => out.push(c),
            c if (c as u32) < 0x20 || c == '\u{FFFE}' || c == '\u{FFFF}' => {
                return Err(OdxError::validation(
                    "validation.console.xmlrpc_control_characters",
                    "XML-RPC text cannot contain control characters",
                ));
            }
            c => out.push(c),
        }
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq)]
enum Scalar {
    Int,
    Boolean,
    String,
    Double,
    DateTime,
    Base64,
    Nil,
}

enum Frame {
    /// methodResponse, params, param, fault and data contain no text.
    Wrapper(&'static str),
    Value(Option<Value>, String),
    Scalar(Scalar, String),
    Array(Vec<Value>),
    Struct(Map<String, Value>),
    Member(Option<String>, Option<Value>),
    Name(String),
}

fn invalid() -> OdxError {
    OdxError::Upstream("invalid XML-RPC response".into())
}

/// Return Ok(Ok(value)) for params and Ok(Err(fault)) for faults. The iterative bounded parser rejects DOCTYPE, custom entities and unknown elements.
fn decode_response(bytes: &[u8]) -> Result<Result<Value, Value>> {
    let text = std::str::from_utf8(bytes).map_err(|_| invalid())?;
    let mut reader = Reader::from_str(text);
    reader.config_mut().expand_empty_elements = true;
    let mut stack: Vec<Frame> = Vec::new();
    let mut result: Option<Value> = None;
    let mut fault: Option<Value> = None;
    let mut root_seen = false;
    loop {
        match reader.read_event().map_err(|_| invalid())? {
            Event::Start(e) => {
                if stack.len() >= MAX_DEPTH {
                    return Err(OdxError::Upstream(
                        "XML-RPC response nesting exceeds limit".into(),
                    ));
                }
                let name = e.name();
                let frame = match name.as_ref() {
                    "methodResponse" if stack.is_empty() && !root_seen => {
                        root_seen = true;
                        Frame::Wrapper("methodResponse")
                    }
                    "params" => Frame::Wrapper("params"),
                    "param" => Frame::Wrapper("param"),
                    "fault" => Frame::Wrapper("fault"),
                    "data" => Frame::Wrapper("data"),
                    "value" => Frame::Value(None, String::new()),
                    "array" => Frame::Array(Vec::new()),
                    "struct" => Frame::Struct(Map::new()),
                    "member" => Frame::Member(None, None),
                    "name" => Frame::Name(String::new()),
                    "int" | "i4" | "i8" => Frame::Scalar(Scalar::Int, String::new()),
                    "boolean" => Frame::Scalar(Scalar::Boolean, String::new()),
                    "string" => Frame::Scalar(Scalar::String, String::new()),
                    "double" => Frame::Scalar(Scalar::Double, String::new()),
                    "dateTime.iso8601" => Frame::Scalar(Scalar::DateTime, String::new()),
                    "base64" => Frame::Scalar(Scalar::Base64, String::new()),
                    "nil" => Frame::Scalar(Scalar::Nil, String::new()),
                    _ => return Err(invalid()),
                };
                if stack.is_empty() && !matches!(frame, Frame::Wrapper("methodResponse")) {
                    return Err(invalid());
                }
                // Enforce wrapper parents: params/fault under root, param under params, data under array.
                let placed = match (&frame, stack.last()) {
                    (Frame::Wrapper("params" | "fault"), Some(Frame::Wrapper(p))) => {
                        *p == "methodResponse"
                    }
                    (Frame::Wrapper("param"), Some(Frame::Wrapper(p))) => *p == "params",
                    (Frame::Wrapper("data"), Some(Frame::Array(_))) => true,
                    (Frame::Wrapper("methodResponse"), None) => true,
                    (Frame::Wrapper(_), _) => false,
                    _ => true,
                };
                if !placed {
                    return Err(invalid());
                }
                stack.push(frame);
            }
            Event::End(_) => {
                let frame = stack.pop().ok_or_else(invalid)?;
                let value = match frame {
                    Frame::Wrapper(_) => continue,
                    Frame::Name(text) => {
                        match stack.last_mut() {
                            Some(Frame::Member(name @ None, _)) => *name = Some(text),
                            _ => return Err(invalid()),
                        }
                        continue;
                    }
                    Frame::Member(name, value) => {
                        let (Some(name), Some(value)) = (name, value) else {
                            return Err(invalid());
                        };
                        match stack.last_mut() {
                            Some(Frame::Struct(map)) => {
                                if map.insert(name, value).is_some() {
                                    return Err(OdxError::Upstream(
                                        "duplicate XML-RPC response members".into(),
                                    ));
                                }
                            }
                            _ => return Err(invalid()),
                        }
                        continue;
                    }
                    Frame::Scalar(kind, text) => scalar(kind, &text)?,
                    Frame::Array(items) => Value::Array(items),
                    Frame::Struct(map) => Value::Object(map),
                    Frame::Value(typed, text) => {
                        let value = match typed {
                            Some(v) if text.trim().is_empty() => v,
                            Some(_) => return Err(invalid()),
                            // Untyped value content is a string.
                            None => Value::String(text),
                        };
                        let n = stack.len();
                        if n >= 2 && matches!(stack[n - 1], Frame::Wrapper("data")) {
                            let Frame::Array(items) = &mut stack[n - 2] else {
                                return Err(invalid());
                            };
                            items.push(value);
                            continue;
                        }
                        match stack.last_mut() {
                            Some(Frame::Member(_, slot @ None)) => *slot = Some(value),
                            Some(Frame::Wrapper("param")) if result.is_none() => {
                                result = Some(value)
                            }
                            Some(Frame::Wrapper("fault")) if fault.is_none() => fault = Some(value),
                            _ => return Err(invalid()),
                        }
                        continue;
                    }
                };
                // Scalars, arrays and structs define their parent value's type.
                match stack.last_mut() {
                    Some(Frame::Value(typed @ None, _)) => *typed = Some(value),
                    _ => return Err(invalid()),
                }
            }
            Event::Text(t) => push_content(&mut stack, &t.xml10_content())?,
            Event::CData(c) => push_content(&mut stack, &c.xml10_content())?,
            Event::GeneralRef(r) => {
                let c = match r.resolve_char_ref().map_err(|_| invalid())? {
                    Some(c) => c.to_string(),
                    None => quick_xml::escape::resolve_predefined_entity(&r)
                        .ok_or_else(|| {
                            OdxError::Upstream("disallowed XML-RPC response entities".into())
                        })?
                        .to_owned(),
                };
                push_content(&mut stack, &c)?
            }
            Event::DocType(_) => {
                return Err(OdxError::Upstream(
                    "DOCTYPE is forbidden in XML-RPC responses".into(),
                ));
            }
            Event::Decl(_) | Event::Comment(_) | Event::PI(_) => {}
            Event::Empty(_) => return Err(invalid()),
            Event::Eof => break,
        }
    }
    if !stack.is_empty() || !root_seen {
        return Err(invalid());
    }
    match (result, fault) {
        (Some(v), None) => Ok(Ok(v)),
        (None, Some(f)) => Ok(Err(f)),
        _ => Err(invalid()),
    }
}

fn push_content(stack: &mut [Frame], text: &str) -> Result<()> {
    match stack.last_mut() {
        Some(Frame::Scalar(_, buf) | Frame::Name(buf) | Frame::Value(_, buf)) => {
            buf.push_str(text);
            Ok(())
        }
        // Allow indentation between elements.
        _ if text.chars().all(char::is_whitespace) => Ok(()),
        _ => Err(invalid()),
    }
}

fn scalar(kind: Scalar, text: &str) -> Result<Value> {
    let t = text.trim();
    Ok(match kind {
        Scalar::String => Value::String(text.to_owned()),
        Scalar::Int => Value::Number(t.parse::<i64>().map_err(|_| invalid())?.into()),
        Scalar::Boolean => match t {
            "1" => Value::Bool(true),
            "0" => Value::Bool(false),
            _ => return Err(invalid()),
        },
        Scalar::Double => {
            let f = t.parse::<f64>().map_err(|_| invalid())?;
            Value::Number(
                Number::from_f64(f)
                    .ok_or_else(|| OdxError::Upstream("non-finite XML-RPC double".into()))?,
            )
        }
        // Preserve as text, matching Odoo JSON-RPC output.
        Scalar::DateTime => Value::String(t.to_owned()),
        Scalar::Base64 => Value::String(t.split_whitespace().collect()),
        Scalar::Nil if t.is_empty() => Value::Null,
        Scalar::Nil => return Err(invalid()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ok(xml: &str) -> Value {
        decode_response(xml.as_bytes()).unwrap().unwrap()
    }

    #[test]
    fn decodes_every_type_odoo_and_the_spec_use() {
        let xml = r#"<?xml version='1.0'?>
<methodResponse>
<params>
<param>
<value><struct>
<member><name>int</name><value><int>-7</int></value></member>
<member><name>i4</name><value><i4>2147483647</i4></value></member>
<member><name>i8</name><value><i8>9007199254740991</i8></value></member>
<member><name>double</name><value><double>1.5</double></value></member>
<member><name>t</name><value><boolean>1</boolean></value></member>
<member><name>f</name><value><boolean>0</boolean></value></member>
<member><name>s</name><value><string>a &amp; b &lt;c&gt; &#241;&#x20AC;</string></value></member>
<member><name>raw</name><value>untyped</value></member>
<member><name>empty</name><value><string></string></value></member>
<member><name>nil</name><value><nil/></value></member>
<member><name>dt</name><value><dateTime.iso8601>20261016T10:00:00</dateTime.iso8601></value></member>
<member><name>b64</name><value><base64>aG9s
YQ==</base64></value></member>
<member><name>list</name><value><array><data>
<value><int>1</int></value><value><array><data></data></array></value>
</data></array></value></member>
</struct></value>
</param>
</params>
</methodResponse>"#;
        assert_eq!(
            ok(xml),
            json!({"int":-7,"i4":2147483647,"i8":9007199254740991i64,"double":1.5,"t":true,
                   "f":false,"s":"a & b <c> ñ€","raw":"untyped","empty":"","nil":null,
                   "dt":"20261016T10:00:00","b64":"aG9sYQ==","list":[1,[]]})
        );
    }

    #[test]
    fn faults_and_malformed_responses() {
        let fault = r#"<?xml version="1.0"?><methodResponse><fault><value><struct>
<member><name>faultCode</name><value><int>1</int></value></member>
<member><name>faultString</name><value><string>Traceback (most recent call last):
  File "x.py", line 1
ValueError: Invalid field 'nope'
</string></value></member></struct></value></fault></methodResponse>"#;
        let f = decode_response(fault.as_bytes()).unwrap().unwrap_err();
        assert_eq!(fault_message(&f), "ValueError: Invalid field 'nope'");
        for bad in [
            "<?xml version=\"1.0\"?><!DOCTYPE x [<!ENTITY e SYSTEM \"file:///etc/passwd\">]><methodResponse><params><param><value>&e;</value></param></params></methodResponse>",
            "<methodResponse><params><param><value>&e;</value></param></params></methodResponse>",
            "<methodResponse><params><param><value><int>x</int></value></param></params></methodResponse>",
            "<methodResponse><params><param><value><boolean>2</boolean></value></param></params></methodResponse>",
            "<methodResponse><params><param><value><double>inf</double></value></param></params></methodResponse>",
            "<methodResponse><params><param><value><foo/></value></param></params></methodResponse>",
            "<methodResponse><params><param><value><struct><member><name>a</name><value>1</value></member><member><name>a</name><value>2</value></member></struct></value></param></params></methodResponse>",
            "<methodResponse><params></params></methodResponse>",
            "<methodResponse><params><param><value>1</value></param>",
            "<html>no</html>",
            // Reject misplaced wrappers: param in fault, data outside array, nested param.
            "<methodResponse><fault><param><value>1</value></param></fault></methodResponse>",
            "<methodResponse><params><data><param><value>1</value></param></data></params></methodResponse>",
            "<methodResponse><params><param><param><value>1</value></param></param></params></methodResponse>",
            "<methodResponse><param><value>1</value></param></methodResponse>",
            "<methodResponse><params><param><value><data><value>1</value></data></value></param></params></methodResponse>",
            "<methodResponse><params><param><value><struct><params></params></struct></value></param></params></methodResponse>",
        ] {
            assert!(decode_response(bad.as_bytes()).is_err(), "{bad}");
        }
        let deep = format!(
            "<methodResponse><params><param>{}1{}</param></params></methodResponse>",
            "<value><array><data>".repeat(60),
            "</data></array></value>".repeat(60)
        );
        assert!(decode_response(deep.as_bytes()).is_err());
    }

    #[test]
    fn encodes_json_values_as_xml_rpc() {
        let v = json!({"a":[1, 5_000_000_000i64, 1.5, true, null, "x<&\r"], "b":{}});
        let xml = encode_call("execute_kw", &[Param::Str("db"), Param::Json(&v)]).unwrap();
        assert!(xml.contains("<methodName>execute_kw</methodName>"));
        assert!(xml.contains("<value><int>1</int></value><value><i8>5000000000</i8></value><value><double>1.5</double></value><value><boolean>1</boolean></value><value><nil/></value><value><string>x&lt;&amp;&#13;</string></value>"));
        assert!(xml.contains("<member><name>b</name><value><struct></struct></value></member>"));
        // Encoded values round-trip inside a response wrapper.
        let body = xml
            .split("<param>")
            .nth(2)
            .unwrap()
            .trim_end_matches("</params></methodCall>");
        let response = format!("<methodResponse><params><param>{body}</params></methodResponse>");
        assert_eq!(
            ok(&response),
            json!({"a":[1, 5_000_000_000i64, 1.5, true, null, "x<&\r"], "b":{}})
        );
        assert!(encode_call("m", &[Param::Str("\u{1}")]).is_err());
    }
}
