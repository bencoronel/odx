//! Shared result envelope, compact formatting and final size limits.
use crate::error::OdxError;
use serde::Serialize;
use serde_json::{Map, Value, json};

/// Final response size limit; MCP JSON counts both representations.
pub const MAX_OUTPUT: usize = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// Validation or denial before sending to Odoo.
    NotSent,
    /// Known Odoo or network failure.
    Failed,
    /// The write may have applied; never retry automatically.
    Uncertain,
}

#[derive(Debug, Clone)]
pub struct ToolError {
    pub code: &'static str,
    pub message: String,
    pub outcome: Outcome,
    /// Desktop-only localization metadata; deliberately omitted from API/MCP envelopes.
    pub ui_code: Option<&'static str>,
    pub ui_params: Value,
}
impl ToolError {
    pub fn new(code: &'static str, message: impl Into<String>, outcome: Outcome) -> Self {
        Self {
            code,
            message: message.into(),
            outcome,
            ui_code: None,
            ui_params: Value::Object(Map::new()),
        }
    }
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new("invalid_arguments", message, Outcome::NotSent)
    }
    pub fn with_ui_error(mut self, code: &'static str, params: Value) -> Self {
        self.ui_code = Some(code);
        self.ui_params = params;
        self
    }
    pub fn http_status(&self) -> u16 {
        match self.code {
            "invalid_arguments" => 400,
            "unauthenticated" => 401,
            "not_found" => 404,
            "forbidden" | "environment_unavailable" | "approval_rejected" | "approval_revoked" => {
                403
            }
            "approval_expired" | "cancelled" => 408,
            "request_too_large" | "output_too_large" => 413,
            "capacity_exceeded" => 429,
            "incompatible_protocol" | "incompatible_method" | "approval_conflict" | "ambiguous" => {
                409
            }
            "upstream_auth" | "upstream_error" | "upstream_network" | "uncertain_result" => 502,
            "upstream_timeout" => 504,
            "output_storage_error" => 507,
            _ => 503,
        }
    }
    /// API error envelope.
    pub fn envelope(&self, request_id: &str) -> Value {
        json!({"error": {
            "code": self.code,
            "message": self.message,
            "request_id": request_id,
            "outcome": self.outcome,
        }})
    }
    /// Compact MCP error text with isError.
    pub fn compact(&self) -> String {
        let outcome = match self.outcome {
            Outcome::NotSent => "not_sent",
            Outcome::Failed => "failed",
            Outcome::Uncertain => "uncertain",
        };
        format!("error {} ({outcome}): {}", self.code, self.message)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Compact,
    Json,
}

#[derive(Debug, Clone)]
pub struct ToolOutput {
    pub data: Value,
    pub meta: Map<String, Value>,
    pub format: Format,
    pub trunc: usize,
}
impl ToolOutput {
    pub fn envelope(&self) -> Value {
        if self.meta.is_empty() {
            json!({"data": self.data})
        } else {
            json!({"data": self.data, "meta": self.meta})
        }
    }
    pub fn warn(&mut self, code: &str, message: impl Into<String>) {
        let warnings = self
            .meta
            .entry("warnings")
            .or_insert_with(|| Value::Array(vec![]));
        if let Value::Array(list) = warnings {
            list.push(json!({"code": code, "message": message.into()}));
        }
    }
    /// Record page; fetched may include one continuation row.
    pub fn page(&mut self, limit: usize, offset: usize) {
        if let Value::Array(rows) = &mut self.data {
            let page = page_of(rows, limit, offset);
            self.meta.insert("page".into(), page);
        }
    }
}

/// Page metadata for rows already starting at `offset`; drop the extra continuation row.
pub(crate) fn page_of(rows: &mut Vec<Value>, limit: usize, offset: usize) -> Value {
    let has_more = rows.len() > limit;
    rows.truncate(limit);
    let mut page =
        json!({"limit": limit, "offset": offset, "returned": rows.len(), "has_more": has_more});
    if has_more {
        page["next_offset"] = json!(offset + rows.len());
    }
    page
}

/// Transport-ready representation.
#[derive(Debug)]
pub struct Rendered {
    pub text: String,
    pub structured: Option<Value>,
}
impl Rendered {
    /// Bytes as sent inside JSON-RPC: the text is escaped there, and control characters grow to six bytes.
    fn size(&self) -> usize {
        serde_json::to_string(&self.text).map_or(usize::MAX, |t| t.len())
            + self.structured.as_ref().map_or(0, |v| v.to_string().len())
    }
}

pub fn render(output: &ToolOutput) -> Rendered {
    match output.format {
        Format::Json => {
            let envelope = output.envelope();
            Rendered {
                text: envelope.to_string(),
                structured: Some(envelope),
            }
        }
        Format::Compact => Rendered {
            text: compact(output),
            structured: None,
        },
    }
}

/// Fit the final limit by dropping whole rows and marking truncation. Return output_too_large if no rows can be dropped.
pub fn fit(output: ToolOutput) -> Result<(ToolOutput, Rendered), ToolError> {
    fit_with(output, Rendered::size)
}
/// `fit` with the transport's own measure of what it sends.
pub fn fit_with(
    mut output: ToolOutput,
    size: impl Fn(&Rendered) -> usize,
) -> Result<(ToolOutput, Rendered), ToolError> {
    loop {
        let rendered = render(&output);
        if size(&rendered) <= MAX_OUTPUT {
            return Ok((output, rendered));
        }
        let Value::Array(rows) = &mut output.data else {
            return Err(ToolError::new(
                "output_too_large",
                format!(
                    "result uses {} bytes, exceeding the {MAX_OUTPUT} limit",
                    size(&rendered)
                ),
                Outcome::Failed,
            ));
        };
        if rows.len() <= 1 {
            return Err(ToolError::new(
                "output_too_large",
                "a single row exceeds the output limit; request fewer fields",
                Outcome::Failed,
            ));
        }
        // Drop rows proportionally to overflow, with a minimum of one.
        let keep = (rows.len() * MAX_OUTPUT / size(&rendered)).clamp(1, rows.len() - 1);
        rows.truncate(keep);
        let returned = rows.len();
        output.meta.insert("truncated".into(), json!(true));
        if let Some(Value::Object(page)) = output.meta.get_mut("page") {
            let offset = page.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize;
            page.insert("returned".into(), json!(returned));
            page.insert("has_more".into(), json!(true));
            page.insert("next_offset".into(), json!(offset + returned));
        }
    }
}

fn cell(value: &Value, trunc: usize) -> String {
    let mut raw = match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    // Truncate before escaping to preserve TSV escape sequences.
    let textual = matches!(value, Value::String(_) | Value::Array(_) | Value::Object(_));
    let cut = textual && trunc > 0 && raw.chars().count() > trunc;
    if cut {
        raw = raw.chars().take(trunc).collect();
    }
    let mut escaped = raw
        .replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
        .replace('\r', "\\r");
    if cut {
        escaped.push('…');
    }
    escaped
}

/// TSV with headers in encounter order; trunc=0 preserves full cells.
fn table(rows: &[Value], trunc: usize) -> String {
    let mut columns: Vec<String> = Vec::new();
    for row in rows {
        for key in row.as_object().into_iter().flat_map(|o| o.keys()) {
            if !columns.contains(key) {
                columns.push(key.clone());
            }
        }
    }
    let mut lines = vec![columns.join("\t")];
    for row in rows {
        lines.push(
            columns
                .iter()
                .map(|c| row.get(c).map_or(String::new(), |v| cell(v, trunc)))
                .collect::<Vec<_>>()
                .join("\t"),
        );
    }
    lines.join("\n")
}

/// File TSV without cell truncation or pagination footer.
pub(crate) fn tsv(rows: &[Value]) -> String {
    let mut text = table(rows, 0);
    text.push('\n');
    text
}

fn compact(output: &ToolOutput) -> String {
    let mut text = match &output.data {
        Value::Array(rows) if !rows.is_empty() && rows.iter().all(Value::is_object) => {
            table(rows, output.trunc)
        }
        Value::Array(rows) if rows.is_empty() => "(no results)".into(),
        other => other.to_string(),
    };
    if let Some(Value::Object(page)) = output.meta.get("page") {
        let get = |k: &str| page.get(k).cloned().unwrap_or(Value::Null);
        text.push_str(&format!(
            "\n# page offset={} returned={} more={}",
            get("offset"),
            get("returned"),
            if get("has_more") == json!(true) {
                "yes"
            } else {
                "no"
            }
        ));
        if let Some(next) = page.get("next_offset") {
            text.push_str(&format!(" next_offset={next}"));
        }
    }
    if output.meta.get("truncated") == Some(&json!(true)) {
        text.push_str("\n# truncated at output limit");
    }
    if let Some(Value::Array(warnings)) = output.meta.get("warnings") {
        for w in warnings {
            // One line per warning: Odoo access errors span several lines and would read as table rows.
            let message = w["message"].as_str().unwrap_or("");
            text.push_str(&format!(
                "\n# warning {}: {}",
                w["code"].as_str().unwrap_or(""),
                message
                    .replace('\\', "\\\\")
                    .replace('\n', "\\n")
                    .replace('\t', "\\t")
            ));
        }
    }
    text
}

fn from_error(error: OdxError) -> ToolError {
    match error {
        OdxError::Locked => ToolError::new("locked", "Odx vault is locked", Outcome::NotSent),
        OdxError::Unauthorized | OdxError::ClientNotFound => ToolError::new(
            "unauthenticated",
            "invalid, replaced or revoked client token",
            Outcome::NotSent,
        ),
        OdxError::Invalid(_) => ToolError::invalid("invalid input"),
        error @ OdxError::Validation { .. } => {
            let code = error.code();
            let params = error.ui_params();
            ToolError::invalid(error.to_string()).with_ui_error(code, params)
        }
        OdxError::EnvironmentNotFound => ToolError::new(
            "environment_unavailable",
            "environment unavailable to this client",
            Outcome::NotSent,
        ),
        OdxError::Denied => ToolError::new(
            "forbidden",
            "denied by environment policy",
            Outcome::NotSent,
        ),
        OdxError::ApprovalUnavailable => ToolError::new(
            "approval_revoked",
            "request no longer valid: revoked, expired or environment changed",
            Outcome::NotSent,
        ),
        OdxError::Uncertain => ToolError::new(
            "uncertain_result",
            "Odoo may have applied the write; no retry was attempted. Check its state before repeating",
            Outcome::Uncertain,
        ),
        OdxError::Persistence | OdxError::Io(_) | OdxError::Json(_) => ToolError::new(
            "audit_unavailable",
            "could not record audit; nothing was sent to Odoo",
            Outcome::NotSent,
        ),
        OdxError::Capacity => ToolError::new(
            "capacity_exceeded",
            "capacity limit reached; retry later",
            Outcome::NotSent,
        ),
        OdxError::Remote(m) => ToolError::new("upstream_error", m, Outcome::Failed),
        OdxError::AuthenticationRejected => ToolError::new(
            "upstream_auth",
            "Odoo rejected the environment credential",
            Outcome::Failed,
        ),
        OdxError::Network {
            kind: "timeout",
            message,
        } => ToolError::new("upstream_timeout", message, Outcome::Failed),
        OdxError::Network {
            kind: "proxy",
            message,
        } => ToolError::new(
            "upstream_network",
            format!("outbound proxy failure: {message}"),
            Outcome::Failed,
        ),
        OdxError::Network { message, .. } => {
            ToolError::new("upstream_network", message, Outcome::Failed)
        }
        error @ OdxError::HttpStatus { .. } => {
            let (code, params) = (error.code(), error.ui_params());
            ToolError::new("upstream_error", error.to_string(), Outcome::Failed)
                .with_ui_error(code, params)
        }
        OdxError::Upstream(m) => ToolError::new("upstream_network", m, Outcome::Failed),
        e @ OdxError::ResponseTooLarge { .. } => {
            ToolError::new("output_too_large", e.to_string(), Outcome::Failed)
        }
        OdxError::UnlockFailed => ToolError::new(
            "service_unavailable",
            "service unavailable",
            Outcome::NotSent,
        ),
    }
}
impl From<OdxError> for ToolError {
    fn from(error: OdxError) -> Self {
        from_error(error)
    }
}

pub(crate) fn not_found(message: impl Into<String>) -> ToolError {
    ToolError::new("not_found", message, Outcome::Failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation_error_keeps_external_envelope_and_filters_desktop_parameters() {
        let error = OdxError::validation_with_params(
            "validation.rule.limit_reached",
            json!({ "limit": 128, "secret": "do-not-send" }),
            "legacy validation text",
        );
        let tool = from_error(error);
        assert_eq!(tool.code, "invalid_arguments");
        assert_eq!(tool.message, "legacy validation text");
        assert_eq!(tool.ui_code, Some("validation.rule.limit_reached"));
        assert_eq!(tool.ui_params, json!({ "limit": 128 }));
        assert_eq!(
            tool.envelope("request")["error"]["code"],
            "invalid_arguments"
        );
        assert!(tool.envelope("request").get("ui_code").is_none());
        assert!(!tool.envelope("request").to_string().contains("do-not-send"));

        let legacy = from_error(OdxError::Invalid("api_key=must-not-leak".into()));
        assert_eq!(legacy.message, "invalid input");
        assert!(
            !legacy
                .envelope("request")
                .to_string()
                .contains("must-not-leak")
        );
    }

    #[test]
    fn a_multi_line_warning_stays_on_its_footer_line() {
        let mut out = rows(1, 1);
        out.warn("soft", "Access denied\n\nAllowed groups:\n\t- Sales");
        let text = compact(&out);
        let footer = text
            .lines()
            .find(|l| l.starts_with("# warning soft"))
            .unwrap();
        assert!(
            footer.ends_with(r"Access denied\n\nAllowed groups:\n\t- Sales"),
            "{text}"
        );
        assert!(!text.lines().any(|l| l.starts_with("Allowed")), "{text}");
    }

    fn rows(n: usize, width: usize) -> ToolOutput {
        let data = (0..n)
            .map(|i| json!({"id": i, "name": "x".repeat(width), "active": false, "ref": null}))
            .collect();
        ToolOutput {
            data: Value::Array(data),
            meta: Map::new(),
            format: Format::Compact,
            trunc: 0,
        }
    }

    #[test]
    fn compact_tsv_keeps_types_and_escapes_cells() {
        let mut out = ToolOutput {
            data: json!([{"id": 1, "name": "a\tb\nc", "active": false, "n": 0, "x": null}]),
            meta: Map::new(),
            format: Format::Compact,
            trunc: 3,
        };
        out.page(50, 0);
        let text = render(&out).text;
        assert!(text.starts_with("id\tname\tactive\tn\tx\n1\ta\\tb…\tfalse\t0\tnull"));
        assert!(text.contains("more=no"));
    }

    #[test]
    fn page_uses_the_extra_row_only_to_detect_more() {
        let mut out = rows(3, 1);
        out.page(2, 10);
        assert_eq!(out.data.as_array().unwrap().len(), 2);
        assert_eq!(out.meta["page"]["next_offset"], 12);
    }

    #[test]
    fn oversized_output_drops_whole_rows_and_marks_continuation() {
        let mut out = rows(400, 2000);
        out.format = Format::Json;
        out.page(1000, 0);
        let (fitted, rendered) = fit(out).unwrap();
        let kept = fitted.data.as_array().unwrap().len();
        assert!(kept < 400 && kept > 0);
        assert!(rendered.size() <= MAX_OUTPUT);
        assert_eq!(fitted.meta["truncated"], true);
        assert_eq!(fitted.meta["page"]["next_offset"], kept);
        assert!(serde_json::from_str::<Value>(&rendered.text).is_ok());
    }

    fn text(data: Value, format: Format, trunc: usize) -> ToolOutput {
        ToolOutput {
            data,
            meta: Map::new(),
            format,
            trunc,
        }
    }

    #[test]
    fn trunc_counts_characters_and_never_splits_multibyte_or_escapes() {
        // Truncate whole 2-, 3- and 4-byte characters.
        assert_eq!(cell(&json!("ñ€😀ñ€😀"), 4), "ñ€😀ñ…");
        assert_eq!(
            cell(&json!("ñ€😀"), 3),
            "ñ€😀",
            "no marker at the exact limit"
        );
        assert_eq!(cell(&json!("ñ€😀"), 0), "ñ€😀");
        // Cut original characters, never TSV escapes.
        assert_eq!(cell(&json!("a\tb"), 2), "a\\t…");
        assert_eq!(cell(&json!("a\\b"), 2), "a\\\\…");
        assert_eq!(cell(&json!("é\nfin"), 2), "é\\n…");
        // Truncate arrays and objects as text; preserve numbers and booleans.
        assert_eq!(cell(&json!([1, "ç", 3]), 4), "[1,\"…");
        assert_eq!(cell(&json!(u64::MAX), 3), u64::MAX.to_string());
        assert_eq!(cell(&json!(-1.5e300), 3), "-1.5e300");
        assert_eq!(cell(&json!(false), 1), "false");
    }

    #[test]
    fn json_ignores_trunc_and_keeps_rare_types() {
        let data = json!([{"id": 1, "name": "ñ".repeat(500), "f": false, "z": 0, "e": "",
                           "n": null, "big": u64::MAX, "neg": -0.0, "o": {"a": [{"b": "😀"}]}}]);
        let rendered = render(&text(data.clone(), Format::Json, 200));
        let parsed: Value = serde_json::from_str(&rendered.text).unwrap();
        assert_eq!(parsed["data"], data);
        assert_eq!(rendered.structured.unwrap()["data"], data);
        assert!(parsed.get("meta").is_none(), "no truncation to mark");
    }

    #[test]
    fn byte_limit_counts_utf8_bytes_and_keeps_whole_rows() {
        // Character counts fit, byte counts do not: emoji use 4 bytes.
        let data = (0..100)
            .map(|i| json!({"id": i, "name": "😀".repeat(1000)}))
            .collect();
        let mut out = text(Value::Array(data), Format::Compact, 0);
        out.page(1000, 0);
        assert!(compact(&out).chars().count() < MAX_OUTPUT && compact(&out).len() > MAX_OUTPUT);
        let (fitted, rendered) = fit(out).unwrap();
        let kept = fitted.data.as_array().unwrap().len();
        assert!(kept > 0 && kept < 100);
        assert!(rendered.text.len() <= MAX_OUTPUT);
        assert_eq!(fitted.meta["truncated"], true);
        assert_eq!(
            fitted.meta["page"],
            json!({"limit": 1000, "offset": 0, "returned": kept, "has_more": true, "next_offset": kept})
        );
        // Preserve whole rows plus header and pagination/truncation footer.
        let lines: Vec<&str> = rendered.text.lines().collect();
        assert_eq!(lines.len(), 1 + kept + 2);
        assert!(
            lines[1..=kept]
                .iter()
                .all(|l| l.ends_with(&"😀".repeat(1000)))
        );
        assert!(rendered.text.contains(&format!("next_offset={kept}")));
        assert!(rendered.text.ends_with("# truncated at output limit"));
    }

    #[test]
    fn mcp_json_counts_both_representations() {
        // ~150 KiB per representation; one fits, both do not.
        let data = (0..150)
            .map(|i| json!({"id": i, "name": "€".repeat(340)}))
            .collect();
        let mut out = text(Value::Array(data), Format::Json, 200);
        out.page(1000, 0);
        let single = out.envelope().to_string().len();
        assert!(single < MAX_OUTPUT && 2 * single > MAX_OUTPUT);
        let (fitted, rendered) = fit(out).unwrap();
        assert!(rendered.size() <= MAX_OUTPUT);
        assert_eq!(fitted.meta["truncated"], true);
        let kept = fitted.data.as_array().unwrap().len();
        assert_eq!(fitted.meta["page"]["next_offset"], kept);
        assert_eq!(
            serde_json::from_str::<Value>(&rendered.text).unwrap(),
            rendered.structured.unwrap()
        );
    }

    #[test]
    fn oversized_multibyte_scalar_is_an_explicit_error() {
        // Oversized indivisible text returns an error instead of corrupting UTF-8 or hiding data loss.
        let out = text(json!("€".repeat(MAX_OUTPUT / 3 + 1)), Format::Compact, 0);
        assert_eq!(fit(out).unwrap_err().code, "output_too_large");
    }

    #[test]
    fn oversized_scalar_is_an_explicit_error() {
        let out = ToolOutput {
            data: json!("y".repeat(MAX_OUTPUT + 1)),
            meta: Map::new(),
            format: Format::Compact,
            trunc: 0,
        };
        assert_eq!(fit(out).unwrap_err().code, "output_too_large");
    }

    #[test]
    fn size_counts_json_escaping_of_control_characters() {
        let rendered = Rendered {
            text: "\u{1}".repeat(1000),
            structured: None,
        };
        assert_eq!(rendered.size(), 6 * 1000 + 2);
    }
}
