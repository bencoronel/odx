//! Composite tools: wait, read_group, ref, history, view, access and compare. Each subcall uses read_call for grants, capabilities, limits, audit and pre-send revision checks. Use named arguments except record IDs. Resolve Odoo 16–19 signature differences through fields_get, never by retrying failed calls with another shape.
use crate::{
    clients::{Capability, ClientIdentity},
    execution::{Core, Upstream},
    output::{Format, MAX_OUTPUT, Outcome, ToolError, ToolOutput, page_of},
    tools::{
        catalog::{
            Args, READ_METHODS, Target, allowed, common, domain, fields, is_identifier, kw, model,
            paged,
        },
        files::{OutputKind, not_found, record_id},
    },
};
use serde_json::{Map, Value, json};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::Arc,
    time::{Duration, Instant},
};

/// Maximum view inheritance depth in either direction.
const MAX_VIEW_DEPTH: usize = 16;
/// Maximum views in a chain.
const MAX_VIEWS: usize = 500;
/// Maximum inline view architecture; use q or to_file for more.
const ARCH_BUDGET: usize = 192 * 1024;
/// Maximum view q matches.
const MAX_MATCHES: usize = 500;
/// Maximum tracking values per history page, read in one call.
const MAX_TRACKING: usize = 5000;
/// Maximum related models whose XML IDs compare resolves per environment.
const MAX_RELATION_MODELS: usize = 8;
/// Maximum related IDs per model for compare.
const MAX_RELATION_IDS: usize = 5000;

type Warnings = Vec<(&'static str, String)>;

/// Logical Odoo errors leave that section unavailable with a warning. Propagate vault, network and audit errors.
fn soft(
    result: Result<Value, ToolError>,
    warnings: &mut Warnings,
    what: &str,
) -> Result<Option<Value>, ToolError> {
    match result {
        Ok(v) => Ok(Some(v)),
        Err(e) if e.code == "upstream_error" => {
            warnings.push(("partial_permissions", format!("{what}: {}", e.message)));
            Ok(None)
        }
        Err(e) => Err(e),
    }
}

fn unexpected(what: &str) -> ToolError {
    ToolError::new(
        "upstream_error",
        format!("unexpected Odoo response in {what}"),
        Outcome::Failed,
    )
}

fn finish(
    data: Value,
    mut meta: Map<String, Value>,
    warnings: Warnings,
    up: &Upstream,
    format: Format,
    trunc: usize,
) -> ToolOutput {
    up.write_meta(&mut meta);
    let mut out = ToolOutput {
        data,
        meta,
        format,
        trunc,
    };
    for (code, message) in warnings {
        out.warn(code, message);
    }
    out
}

fn m2o_id(v: &Value) -> Option<i64> {
    v.get(0).and_then(Value::as_i64)
}

fn int_list(v: &Value) -> Vec<i64> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_i64)
        .collect()
}

/// read_group groupby: field or field:granularity.
fn valid_groupby(spec: &str) -> bool {
    match spec.split_once(':') {
        None => is_identifier(spec, false),
        Some((f, g)) => is_identifier(f, false) && is_identifier(g, false),
    }
}
/// read_group fields: field, field:aggregate or name:aggregate(field).
fn valid_aggregate(spec: &str) -> bool {
    match spec.split_once(':') {
        None => is_identifier(spec, false),
        Some((name, agg)) => {
            is_identifier(name, false)
                && match agg.split_once('(') {
                    None => is_identifier(agg, false),
                    Some((func, rest)) => {
                        is_identifier(func, false)
                            && rest
                                .strip_suffix(')')
                                .is_some_and(|f| is_identifier(f, false))
                    }
                }
        }
    }
}
fn string_list(
    args: &mut Args,
    key: &'static str,
    valid: fn(&str) -> bool,
) -> Result<Option<Vec<String>>, ToolError> {
    let Some(list) = args.array(key)? else {
        return Ok(None);
    };
    let mut out: Vec<String> = Vec::new();
    for item in list {
        let Some(s) = item.as_str().filter(|s| valid(s)) else {
            return Err(ToolError::invalid(format!("{key}: invalid item {item}")));
        };
        if out.iter().any(|x| x == s) {
            return Err(ToolError::invalid(format!("{key}: duplicate item {s}")));
        }
        out.push(s.into());
    }
    Ok(Some(out))
}

/// The grouped tool and call(read_group) share validation and defaults.
pub(crate) fn group_fields(args: &mut Args) -> Result<(Vec<String>, Vec<String>), ToolError> {
    let groupby = string_list(args, "groupby", valid_groupby)?
        .ok_or_else(|| ToolError::invalid("groupby is required; use [] for totals"))?;
    let mut aggregates = string_list(args, "fields", valid_aggregate)?.unwrap_or_default();
    if groupby.is_empty() && (aggregates.is_empty() || aggregates.iter().any(|f| !f.contains(':')))
    {
        return Err(ToolError::invalid(
            "groupby=[] requires explicit field aggregates, such as amount_total:sum",
        ));
    }
    if aggregates.is_empty() {
        // Prevent Odoo 16 from aggregating every field by default.
        aggregates = groupby
            .iter()
            .map(|g| g.split(':').next().unwrap_or(g).to_string())
            .collect();
        aggregates.dedup();
    }
    Ok((groupby, aggregates))
}
/// Group ordering uses asc/desc without adding id, which is not a group key.
pub(crate) fn group_order(raw: &str) -> Result<String, ToolError> {
    for part in raw.split(',') {
        let tokens: Vec<&str> = part.split_whitespace().collect();
        let valid = match tokens.as_slice() {
            [f] => valid_aggregate(f) || valid_groupby(f),
            [f, dir] => {
                (valid_aggregate(f) || valid_groupby(f))
                    && matches!(dir.to_ascii_lowercase().as_str(), "asc" | "desc")
            }
            _ => false,
        };
        if !valid {
            return Err(ToolError::invalid(format!("invalid orderby: {raw}")));
        }
    }
    Ok(raw.into())
}

impl Core {
    /// Composite subcalls share the read execution path.
    #[allow(clippy::too_many_arguments)]
    async fn sub(
        &self,
        client: &ClientIdentity,
        target: &Target,
        model: &str,
        method: &'static str,
        args: Value,
        kwargs: Map<String, Value>,
        context: &Value,
        up: &mut Upstream,
    ) -> Result<Value, ToolError> {
        debug_assert!(READ_METHODS.contains(&method), "{method}");
        self.read_call(client, target, model, method, args, kwargs, context, up)
            .await
    }

    // wait

    pub(crate) async fn tool_wait(
        self: &Arc<Self>,
        client: &ClientIdentity,
        arguments: Value,
    ) -> Result<ToolOutput, ToolError> {
        let mut args = Args::new(
            arguments,
            &allowed(&["model", "domain", "absent", "timeout", "interval"]),
        )?;
        let (format, trunc) = common(&mut args)?;
        let env_name = args.str("env")?;
        let context = Value::Object(args.object("context")?);
        let model = model(&mut args)?;
        if !args.has("domain") {
            return Err(ToolError::invalid("missing domain"));
        }
        let domain = domain(&mut args)?;
        let absent = args.bool("absent")?;
        let timeout = args.int("timeout", 60, 1, 600)? as u64;
        let interval = args.int("interval", 3, 3, 600)? as u64;
        let started = Instant::now();
        let deadline = started + Duration::from_secs(timeout);
        let mut up = Upstream::start();
        let mut warnings: Warnings = Vec::new();
        let (mut polls, mut count): (u64, Option<u64>) = (0, None);
        let matched = loop {
            let poll_started = Instant::now();
            // Revalidate grants, capabilities and environment before each poll.
            let (environment_ids, capabilities) = self.current_grant(client).await?;
            let client = ClientIdentity {
                environment_ids,
                capabilities,
                ..client.clone()
            };
            let target = self
                .target(&client, &env_name, Capability::OdooRead)
                .await?;
            // Run polls in separate tasks so timeout/cancellation still records their outcome. Hold no upstream permits between polls.
            let poll = {
                let (core, model, domain, context) =
                    (self.clone(), model.clone(), domain.clone(), context.clone());
                tokio::spawn(async move {
                    let mut up = Upstream::start();
                    let result = core
                        .sub(
                            &client,
                            &target,
                            &model,
                            "search_count",
                            json!([]),
                            kw(&[("domain", domain)]),
                            &context,
                            &mut up,
                        )
                        .await;
                    (result, up)
                })
            };
            polls += 1;
            let remaining = deadline.saturating_duration_since(Instant::now());
            let (result, sub) = match tokio::time::timeout(remaining, poll).await {
                Ok(Ok(done)) => done,
                Ok(Err(_)) => {
                    return Err(ToolError::new(
                        "service_unavailable",
                        "poll interrupted",
                        Outcome::Failed,
                    ));
                }
                Err(_) => {
                    warnings.push((
                        "poll_in_flight",
                        "deadline elapsed during a poll; its result remains in Activity".into(),
                    ));
                    break false;
                }
            };
            up.calls += sub.calls;
            up.bytes += sub.bytes;
            let n = result?.as_u64().ok_or_else(|| unexpected("search_count"))?;
            count = Some(n);
            if (n > 0) != absent {
                break true;
            }
            let next = poll_started + Duration::from_secs(interval);
            if next >= deadline {
                tokio::time::sleep_until(deadline.into()).await;
                break false;
            }
            tokio::time::sleep_until(next.into()).await;
        };
        let elapsed = (started.elapsed().as_millis() / 100) as f64 / 10.0;
        let mut meta = Map::new();
        meta.insert("polls".into(), json!(polls));
        Ok(finish(
            json!({"matched": matched, "count": count, "elapsed_seconds": elapsed}),
            meta,
            warnings,
            &up,
            format,
            trunc,
        ))
    }

    // read_group

    pub(crate) async fn tool_read_group(
        &self,
        client: &ClientIdentity,
        arguments: Value,
    ) -> Result<ToolOutput, ToolError> {
        let mut args = Args::new(
            arguments,
            &allowed(&[
                "model", "groupby", "domain", "fields", "limit", "offset", "orderby", "to_file",
            ]),
        )?;
        let (format, trunc) = common(&mut args)?;
        let env_name = args.str("env")?;
        let context = Value::Object(args.object("context")?);
        let model = model(&mut args)?;
        let (groupby, aggregates) = group_fields(&mut args)?;
        let domain = domain(&mut args)?;
        let limit = args.int("limit", 50, 1, 1000)? as usize;
        let offset = args.int("offset", 0, 0, i64::MAX)? as usize;
        let orderby = args
            .str_opt("orderby")?
            .map(|o| group_order(&o))
            .transpose()?;
        let to_file = args.bool("to_file")?;
        let target = self.target(client, &env_name, Capability::OdooRead).await?;
        let mut up = Upstream::start();
        let mut kwargs = kw(&[
            ("domain", domain),
            ("fields", json!(aggregates)),
            ("groupby", json!(groupby)),
            ("offset", json!(offset)),
            ("limit", json!(limit + 1)),
            ("lazy", json!(false)),
        ]);
        if let Some(o) = orderby {
            kwargs.insert("orderby".into(), json!(o));
        }
        let groups = self
            .sub(
                client,
                &target,
                &model,
                "read_group",
                json!([]),
                kwargs,
                &context,
                &mut up,
            )
            .await?;
        let rows: Vec<Value> = groups
            .as_array()
            .ok_or_else(|| unexpected("read_group"))?
            .iter()
            .map(|g| {
                let clean: Map<String, Value> = g
                    .as_object()
                    .into_iter()
                    .flatten()
                    .filter(|(k, _)| !k.starts_with("__") || *k == "__count")
                    .map(|(k, v)| {
                        let k = if k == "__count" { "count" } else { k.as_str() };
                        (k.to_string(), v.clone())
                    })
                    .collect();
                Value::Object(clean)
            })
            .collect();
        let mut meta = Map::new();
        let data = paged(Value::Array(rows), limit, offset, &mut meta);
        let out = finish(data, meta, Vec::new(), &up, format, trunc);
        if to_file {
            return self
                .output_to_file(out, OutputKind::Tsv, &format!("{model}-read_group"))
                .await;
        }
        Ok(out)
    }

    // ref

    pub(crate) async fn tool_ref(
        &self,
        client: &ClientIdentity,
        arguments: Value,
    ) -> Result<ToolOutput, ToolError> {
        let mut args = Args::new(arguments, &allowed(&["xmlid"]))?;
        let (format, trunc) = common(&mut args)?;
        let env_name = args.str("env")?;
        let call_context = args.object("context")?;
        let xmlid = args.str("xmlid")?;
        let (module, name) = xmlid
            .split_once('.')
            .filter(|(m, n)| {
                !m.is_empty()
                    && !n.is_empty()
                    && !n.contains('.')
                    && xmlid.len() <= 256
                    && !xmlid.chars().any(char::is_whitespace)
            })
            .ok_or_else(|| ToolError::invalid("xmlid must use module.name format"))?;
        let target = self.target(client, &env_name, Capability::OdooRead).await?;
        let mut up = Upstream::start();
        let context = Value::Object(call_context.clone());
        let rows = self
            .sub(
                client,
                &target,
                "ir.model.data",
                "search_read",
                json!([]),
                kw(&[
                    (
                        "domain",
                        json!([["module", "=", module], ["name", "=", name]]),
                    ),
                    ("fields", json!(["model", "res_id"])),
                    ("limit", json!(2)),
                    ("order", json!("id asc")),
                ]),
                &context,
                &mut up,
            )
            .await?;
        let rows = rows.as_array().ok_or_else(|| unexpected("ir.model.data"))?;
        match rows.len() {
            0 => {
                return Err(not_found(format!(
                    "XML ID {xmlid} not found; no name lookup attempted"
                )));
            }
            1 => {}
            _ => {
                return Err(ToolError::new(
                    "ambiguous",
                    format!("XML ID {xmlid} has multiple ir.model.data entries"),
                    Outcome::Failed,
                ));
            }
        }
        let res_model = rows[0]["model"]
            .as_str()
            .filter(|m| is_identifier(m, true))
            .ok_or_else(|| unexpected("ir.model.data.model"))?
            .to_string();
        let res_id = rows[0]["res_id"]
            .as_i64()
            .ok_or_else(|| unexpected("ir.model.data.res_id"))?;
        // XML IDs may reference deleted records; include archived records when checking.
        let mut warnings: Warnings = Vec::new();
        let mut exists_context = call_context;
        exists_context.insert("active_test".into(), json!(false));
        let exists = self
            .sub(
                client,
                &target,
                &res_model,
                "search_count",
                json!([]),
                kw(&[("domain", json!([["id", "=", res_id]]))]),
                &Value::Object(exists_context),
                &mut up,
            )
            .await;
        match exists {
            Ok(n) if n.as_u64() == Some(0) => {
                return Err(not_found(format!(
                    "XML ID {xmlid} points to missing or inaccessible {res_model}({res_id})"
                )));
            }
            Ok(_) => {}
            Err(e) if e.code == "upstream_error" => warnings.push((
                "existence_unverified",
                format!(
                    "could not verify {res_model}({res_id}) exists: {}",
                    e.message
                ),
            )),
            Err(e) => return Err(e),
        }
        Ok(finish(
            json!({"model": res_model, "id": res_id}),
            Map::new(),
            warnings,
            &up,
            format,
            trunc,
        ))
    }

    // history

    pub(crate) async fn tool_history(
        &self,
        client: &ClientIdentity,
        arguments: Value,
    ) -> Result<ToolOutput, ToolError> {
        let mut args = Args::new(
            arguments,
            &allowed(&["model", "id", "limit", "offset", "to_file"]),
        )?;
        let (format, trunc) = common(&mut args)?;
        let env_name = args.str("env")?;
        let context = Value::Object(args.object("context")?);
        let model = model(&mut args)?;
        let id = record_id(&mut args)?;
        let limit = args.int("limit", 50, 1, 1000)? as usize;
        let offset = args.int("offset", 0, 0, i64::MAX)? as usize;
        let to_file = args.bool("to_file")?;
        let target = self.target(client, &env_name, Capability::OdooRead).await?;
        let mut up = Upstream::start();
        let mut warnings: Warnings = Vec::new();
        let messages = self
            .sub(
                client,
                &target,
                "mail.message",
                "search_read",
                json!([]),
                kw(&[
                    (
                        "domain",
                        json!([["model", "=", model], ["res_id", "=", id]]),
                    ),
                    // Omit tracking_value_ids: it requires base.group_system in Odoo 16–19 or the entire read fails with AccessError.
                    (
                        "fields",
                        json!(["date", "author_id", "message_type", "subtype_id", "body"]),
                    ),
                    ("offset", json!(offset)),
                    ("limit", json!(limit + 1)),
                    ("order", json!("id desc")),
                ]),
                &context,
                &mut up,
            )
            .await?;
        let mut meta = Map::new();
        let messages = paged(messages, limit, offset, &mut meta);
        let messages = messages.as_array().cloned().unwrap_or_default();
        if messages.is_empty() && offset == 0 {
            // Odoo filters inaccessible records' messages; empty results do not prove existence or absence of history.
            warnings.push((
                "no_visible_messages",
                format!(
                    "no visible messages for {model}({id}); the record may be missing or inaccessible"
                ),
            ));
        }
        let message_ids: Vec<i64> = messages.iter().filter_map(|m| m["id"].as_i64()).collect();
        let changes = if message_ids.is_empty() {
            Some(HashMap::new())
        } else {
            self.tracking(
                client,
                &target,
                &message_ids,
                &context,
                &mut up,
                &mut warnings,
            )
            .await?
        };
        let rows: Vec<Value> = messages
            .iter()
            .map(|m| {
                let mid = m["id"].as_i64().unwrap_or_default();
                // None means tracking values are unreadable and changes are unknown.
                let changes = changes.as_ref().map_or(Value::Null, |map| {
                    json!(map.get(&mid).cloned().unwrap_or_default())
                });
                json!({
                    "id": m["id"], "date": m["date"], "author": m["author_id"],
                    "type": m["message_type"], "subtype": m["subtype_id"],
                    "body": m["body"], "changes": changes,
                })
            })
            .collect();
        if to_file {
            // One row per change; show the message body only in its first row.
            let mut flat = Vec::new();
            for r in &rows {
                let list = r["changes"].as_array().cloned().unwrap_or_default();
                let base = |c: &Value, first: bool| {
                    json!({"id": r["id"], "date": r["date"], "author": r["author"],
                           "type": r["type"], "subtype": r["subtype"],
                           "field": c["field"], "label": c["label"], "old": c["old"], "new": c["new"],
                           "changes_readable": !r["changes"].is_null(),
                           "body": if first { r["body"].clone() } else { json!("") }})
                };
                if list.is_empty() {
                    flat.push(base(&Value::Null, true));
                }
                for (i, c) in list.iter().enumerate() {
                    flat.push(base(c, i == 0));
                }
            }
            let out = finish(Value::Array(flat), meta, warnings, &up, format, trunc);
            return self
                .output_to_file(out, OutputKind::Tsv, &format!("{model}-{id}-history"))
                .await;
        }
        Ok(finish(
            Value::Array(rows),
            meta,
            warnings,
            &up,
            format,
            trunc,
        ))
    }

    /// Per-message changes; None if tracking values are unreadable under system ACLs or exceed the page budget.
    async fn tracking(
        &self,
        client: &ClientIdentity,
        target: &Target,
        message_ids: &[i64],
        context: &Value,
        up: &mut Upstream,
        warnings: &mut Warnings,
    ) -> Result<Option<HashMap<i64, Vec<Value>>>, ToolError> {
        // Odoo 16 uses field/field_desc/field_type and *_monetary; 17+ uses field_id/field_info and *_float. Request only existing columns.
        const CANDIDATES: [&str; 18] = [
            "field",
            "field_id",
            "field_desc",
            "field_type",
            "field_info",
            "mail_message_id",
            "old_value_integer",
            "old_value_float",
            "old_value_monetary",
            "old_value_char",
            "old_value_text",
            "old_value_datetime",
            "new_value_integer",
            "new_value_float",
            "new_value_monetary",
            "new_value_char",
            "new_value_text",
            "new_value_datetime",
        ];
        let described = self
            .sub(
                client,
                target,
                "mail.tracking.value",
                "fields_get",
                json!([]),
                kw(&[
                    ("allfields", json!(CANDIDATES)),
                    ("attributes", json!(["type"])),
                ]),
                context,
                up,
            )
            .await;
        let Some(described) = soft(described, warnings, "mail.tracking.value")? else {
            return Ok(None);
        };
        let available: Vec<&str> = CANDIDATES
            .into_iter()
            .filter(|f| described.get(*f).is_some())
            .collect();
        let field_key = if available.contains(&"field_id") {
            "field_id"
        } else {
            "field"
        };
        let tracks = self
            .sub(
                client,
                target,
                "mail.tracking.value",
                "search_read",
                json!([]),
                kw(&[
                    ("domain", json!([["mail_message_id", "in", message_ids]])),
                    ("fields", json!(available)),
                    ("limit", json!(MAX_TRACKING + 1)),
                    ("order", json!("id asc")),
                ]),
                context,
                up,
            )
            .await;
        let Some(tracks) = soft(tracks, warnings, "mail.tracking.value")? else {
            return Ok(None);
        };
        if tracks.as_array().is_some_and(|t| t.len() > MAX_TRACKING) {
            warnings.push((
                "changes_omitted",
                format!("page exceeds {MAX_TRACKING} tracking values; reduce limit"),
            ));
            return Ok(None);
        }
        let tracks = tracks.as_array().cloned().unwrap_or_default();
        let mut field_ids: Vec<i64> = tracks
            .iter()
            .filter_map(|t| m2o_id(&t[field_key]))
            .collect();
        field_ids.sort_unstable();
        field_ids.dedup();
        let mut info: HashMap<i64, Value> = HashMap::new();
        if !field_ids.is_empty() {
            let fields = self
                .sub(
                    client,
                    target,
                    "ir.model.fields",
                    "read",
                    json!([field_ids]),
                    kw(&[("fields", json!(["name", "field_description", "ttype"]))]),
                    context,
                    up,
                )
                .await;
            if let Some(fields) = soft(fields, warnings, "ir.model.fields")? {
                for f in fields.as_array().into_iter().flatten() {
                    if let Some(id) = f["id"].as_i64() {
                        info.insert(id, f.clone());
                    }
                }
            }
        }
        let mut out: HashMap<i64, Vec<Value>> = HashMap::new();
        for t in tracks {
            let Some(message) = m2o_id(&t["mail_message_id"]) else {
                continue;
            };
            let f = m2o_id(&t[field_key])
                .and_then(|id| info.get(&id))
                .cloned()
                .unwrap_or(Value::Null);
            let removed = &t["field_info"];
            let ttype = f["ttype"]
                .as_str()
                .or_else(|| t["field_type"].as_str())
                .or_else(|| removed["type"].as_str())
                .unwrap_or("char");
            let name = f["name"]
                .as_str()
                .or_else(|| removed["name"].as_str())
                .map_or(Value::Null, |s| json!(s));
            let label = f["field_description"]
                .as_str()
                .or_else(|| t["field_desc"].as_str())
                .or_else(|| removed["desc"].as_str())
                .or_else(|| t[field_key].get(1).and_then(Value::as_str))
                .map_or(Value::Null, |s| json!(s));
            let side = |side: &str| -> Value {
                let col = |c: &str| t[format!("{side}_value_{c}")].clone();
                match ttype {
                    "integer" => col("integer"),
                    "boolean" => col("integer")
                        .as_i64()
                        .map_or(Value::Null, |v| json!(v != 0)),
                    "float" => col("float"),
                    "monetary" if t.get(format!("{side}_value_monetary")).is_some() => {
                        col("monetary")
                    }
                    "monetary" => col("float"),
                    "date" | "datetime" => col("datetime"),
                    "text" | "html" => col("text"),
                    "many2one" => match col("integer").as_i64() {
                        Some(id) if id > 0 => json!([id, col("char")]),
                        _ => json!(false),
                    },
                    _ => col("char"),
                }
            };
            out.entry(message).or_default().push(json!({
                "field": name, "label": label, "old": side("old"), "new": side("new"),
            }));
        }
        Ok(Some(out))
    }

    // view

    pub(crate) async fn tool_view(
        &self,
        client: &ClientIdentity,
        arguments: Value,
    ) -> Result<ToolOutput, ToolError> {
        let mut args = Args::new(
            arguments,
            &allowed(&[
                "model",
                "view_type",
                "view_id",
                "q",
                "limit",
                "offset",
                "to_file",
            ]),
        )?;
        let (format, trunc) = common(&mut args)?;
        let env_name = args.str("env")?;
        let context = Value::Object(args.object("context")?);
        let model = model(&mut args)?;
        let view_type = args.str_opt("view_type")?.unwrap_or_else(|| "form".into());
        if !is_identifier(&view_type, false) {
            return Err(ToolError::invalid(format!(
                "invalid view_type: {view_type}"
            )));
        }
        let view_id = if args.has("view_id") {
            Some(args.int("view_id", 0, 1, 9_007_199_254_740_991)?)
        } else {
            None
        };
        let q = args.text("q")?.filter(|q| !q.is_empty());
        let limit = args.int("limit", 50, 1, 1000)? as usize;
        let offset = args.int("offset", 0, 0, i64::MAX)? as usize;
        let to_file = args.bool("to_file")?;
        // JSON sends the envelope twice (text and structuredContent) with quotes escaped; leave room for both.
        let arch_budget = if format == Format::Json {
            MAX_OUTPUT / 2 - 24 * 1024
        } else {
            ARCH_BUDGET
        };
        let target = self.target(client, &env_name, Capability::OdooRead).await?;
        let mut up = Upstream::start();
        let mut warnings: Warnings = Vec::new();
        let result = self
            .sub(
                client,
                &target,
                &model,
                "get_views",
                json!([]),
                kw(&[
                    (
                        "views",
                        json!([[view_id.map_or(json!(false), |v| json!(v)), view_type]]),
                    ),
                    ("options", json!({})),
                ]),
                &context,
                &mut up,
            )
            .await?;
        let view = &result["views"][&view_type];
        let arch = view["arch"]
            .as_str()
            .ok_or_else(|| unexpected("get_views"))?
            .to_string();
        let root = view["id"].as_i64().filter(|id| *id > 0);
        let mut views = match root {
            Some(root) => {
                self.view_chain(
                    client,
                    &target,
                    root,
                    q.is_some(),
                    &context,
                    &mut up,
                    &mut warnings,
                )
                .await?
            }
            None => {
                warnings.push((
                    "no_view_record",
                    "Odoo generated the default view; no ir.ui.view records to list".into(),
                ));
                Some(Vec::new())
            }
        };
        if let (Some(views), Some(q)) = (views.as_mut(), &q) {
            for v in views.iter_mut() {
                let has = v["arch_db"]
                    .as_str()
                    .is_some_and(|a| a.contains(q.as_str()));
                if let Some(o) = v.as_object_mut() {
                    o.remove("arch_db");
                    o.insert("has_q".into(), json!(has));
                }
            }
        }
        let mut meta = Map::new();
        let views = views.map(|mut v| {
            // The chain is fetched whole, unlike Odoo-paged collections.
            v.drain(..offset.min(v.len()));
            let page = page_of(&mut v, limit, offset);
            meta.insert("page".into(), page);
            Value::Array(v)
        });
        let arch_bytes = arch.len();
        if to_file {
            let out = finish(json!(arch), meta, warnings, &up, format, trunc);
            let mut saved = self
                .output_to_file(out, OutputKind::Xml, &format!("{model}-{view_type}"))
                .await?;
            if let Value::Object(d) = &mut saved.data {
                d.insert("view_id".into(), json!(root));
                d.insert("views".into(), views.unwrap_or(Value::Null));
            }
            return Ok(saved);
        }
        // Separate adjacent tags for searching/truncation without changing XML.
        let mut data = json!({"view_id": root, "view_type": view_type, "arch_bytes": arch_bytes,
                              "views": views.unwrap_or(Value::Null)});
        if let Some(q) = &q {
            let lines = arch.replace("><", ">\n<");
            let mut matches: Vec<Value> = lines
                .lines()
                .enumerate()
                .filter(|(_, l)| l.contains(q.as_str()))
                .map(|(n, l)| json!({"line": n + 1, "text": l.trim()}))
                .take(MAX_MATCHES + 1)
                .collect();
            if matches.len() > MAX_MATCHES {
                matches.truncate(MAX_MATCHES);
                meta.insert("truncated".into(), json!(true));
                warnings_into(
                    &mut meta,
                    "matches_truncated",
                    format!("showing only {MAX_MATCHES} matches; use to_file"),
                );
            }
            if matches.is_empty() {
                warnings_into(
                    &mut meta,
                    "no_match",
                    format!("'{q}' not found in the final architecture"),
                );
            }
            data["arch"] = Value::Null;
            data["matches"] = json!(matches);
        } else if arch_bytes > arch_budget {
            // Mark truncation after a closing tag on a character boundary.
            let mut cut = arch_budget;
            while !arch.is_char_boundary(cut) {
                cut -= 1;
            }
            let cut = arch[..cut].rfind('>').map_or(cut, |i| i + 1);
            data["arch"] = json!(&arch[..cut]);
            meta.insert("truncated".into(), json!(true));
            warnings_into(
                &mut meta,
                "arch_truncated",
                format!("architecture truncated to {cut} of {arch_bytes} bytes; use q or to_file"),
            );
        } else {
            data["arch"] = json!(arch);
        }
        Ok(finish(data, meta, warnings, &up, format, trunc))
    }

    /// Bounded, cycle-free primary ancestors and extension chain.
    #[allow(clippy::too_many_arguments)]
    async fn view_chain(
        &self,
        client: &ClientIdentity,
        target: &Target,
        root: i64,
        with_arch: bool,
        context: &Value,
        up: &mut Upstream,
        warnings: &mut Warnings,
    ) -> Result<Option<Vec<Value>>, ToolError> {
        let mut fields = vec!["name", "priority", "mode", "inherit_id", "xml_id", "model"];
        if with_arch {
            fields.push("arch_db");
        }
        let read = || kw(&[("fields", json!(fields))]);
        let first = self
            .sub(
                client,
                target,
                "ir.ui.view",
                "read",
                json!([[root]]),
                read(),
                context,
                up,
            )
            .await;
        let Some(first) = soft(first, warnings, "ir.ui.view")? else {
            return Ok(None);
        };
        let mut chain: Vec<Value> = first.as_array().cloned().unwrap_or_default();
        let mut visited: HashSet<i64> = chain.iter().filter_map(|v| v["id"].as_i64()).collect();
        // A primary view with inherit_id extends its parent.
        let mut depth = 0;
        while let Some(head) = chain.first() {
            if head["mode"] != "primary" {
                break;
            }
            let Some(parent) = m2o_id(&head["inherit_id"]) else {
                break;
            };
            if !visited.insert(parent) {
                warnings.push((
                    "inheritance_cycle",
                    format!("inheritance cycle at view {parent}"),
                ));
                break;
            }
            depth += 1;
            if depth > MAX_VIEW_DEPTH {
                warnings.push((
                    "chain_truncated",
                    format!("more than {MAX_VIEW_DEPTH} ancestors; incomplete chain"),
                ));
                break;
            }
            let parent = self
                .sub(
                    client,
                    target,
                    "ir.ui.view",
                    "read",
                    json!([[parent]]),
                    read(),
                    context,
                    up,
                )
                .await;
            let Some(parent) = soft(parent, warnings, "ir.ui.view")? else {
                break;
            };
            let Some(parent) = parent.get(0).cloned() else {
                break;
            };
            chain.insert(0, parent);
        }
        // Traverse extensions by level in Odoo order: priority, id.
        let mut frontier: Vec<i64> = chain.iter().filter_map(|v| v["id"].as_i64()).collect();
        let mut level = 0;
        while !frontier.is_empty() {
            level += 1;
            if level > MAX_VIEW_DEPTH {
                warnings.push((
                    "chain_truncated",
                    format!("more than {MAX_VIEW_DEPTH} extension levels; incomplete chain"),
                ));
                break;
            }
            let room = MAX_VIEWS.saturating_sub(chain.len());
            if room == 0 {
                warnings.push((
                    "chain_truncated",
                    format!("more than {MAX_VIEWS} views; incomplete chain"),
                ));
                break;
            }
            let kids = self
                .sub(
                    client,
                    target,
                    "ir.ui.view",
                    "search_read",
                    json!([]),
                    kw(&[
                        (
                            "domain",
                            json!([["inherit_id", "in", frontier], ["mode", "=", "extension"]]),
                        ),
                        ("fields", json!(fields)),
                        ("limit", json!(room + 1)),
                        ("order", json!("priority asc, id asc")),
                    ]),
                    context,
                    up,
                )
                .await;
            let Some(kids) = soft(kids, warnings, "ir.ui.view")? else {
                break;
            };
            let mut kids = kids.as_array().cloned().unwrap_or_default();
            if kids.len() > room {
                kids.truncate(room);
                warnings.push((
                    "chain_truncated",
                    format!("more than {MAX_VIEWS} views; incomplete chain"),
                ));
                frontier.clear();
            } else {
                frontier = Vec::new();
            }
            for k in kids {
                let Some(id) = k["id"].as_i64() else { continue };
                if !visited.insert(id) {
                    warnings.push((
                        "inheritance_cycle",
                        format!("inheritance cycle at view {id}"),
                    ));
                    continue;
                }
                frontier.push(id);
                chain.push(k);
            }
            if warnings.iter().any(|(c, _)| *c == "chain_truncated") {
                break;
            }
        }
        Ok(Some(
            chain
                .into_iter()
                .map(|v| {
                    let mut row = json!({"id": v["id"], "xml_id": v["xml_id"], "mode": v["mode"],
                        "priority": v["priority"], "inherit_id": v["inherit_id"], "name": v["name"],
                        "model": v["model"]});
                    if with_arch {
                        row["arch_db"] = v["arch_db"].clone();
                    }
                    row
                })
                .collect(),
        ))
    }

    // access

    pub(crate) async fn tool_access(
        &self,
        client: &ClientIdentity,
        arguments: Value,
    ) -> Result<ToolOutput, ToolError> {
        let mut args = Args::new(arguments, &allowed(&["model", "limit", "offset"]))?;
        let (format, trunc) = common(&mut args)?;
        let env_name = args.str("env")?;
        let context = Value::Object(args.object("context")?);
        let model = model(&mut args)?;
        let limit = args.int("limit", 50, 1, 1000)? as usize;
        let offset = args.int("offset", 0, 0, i64::MAX)? as usize;
        let target = self.target(client, &env_name, Capability::OdooRead).await?;
        let mut up = Upstream::start();
        let mut warnings: Warnings = Vec::new();
        // check_access_rights exists in 16–19, deprecated since 18. raise_exception=false returns a boolean; has_access in 18+ checks records.
        let mut rights = Map::new();
        for op in ["read", "write", "create", "unlink"] {
            let r = self
                .sub(
                    client,
                    &target,
                    &model,
                    "check_access_rights",
                    json!([]),
                    kw(&[("operation", json!(op)), ("raise_exception", json!(false))]),
                    &context,
                    &mut up,
                )
                .await;
            let r = soft(r, &mut warnings, &format!("check_access_rights({op})"))?;
            rights.insert(op.into(), r.unwrap_or(Value::Null));
        }
        let groups = self
            .user_groups(client, &target, &context, &mut up, &mut warnings)
            .await?;
        let perms = |r: &Value| -> String {
            ["read", "write", "create", "unlink"]
                .iter()
                .zip(['r', 'w', 'c', 'd'])
                .map(|(p, c)| {
                    if r[format!("perm_{p}")] == json!(true) {
                        c
                    } else {
                        '-'
                    }
                })
                .collect()
        };
        let mut continuations = Map::new();
        let by_model = json!([["model_id.model", "=", model]]);
        let acls = self
            .sub(
                client,
                &target,
                "ir.model.access",
                "search_read",
                json!([]),
                kw(&[
                    ("domain", by_model.clone()),
                    (
                        "fields",
                        json!([
                            "name",
                            "group_id",
                            "perm_read",
                            "perm_write",
                            "perm_create",
                            "perm_unlink"
                        ]),
                    ),
                    ("offset", json!(offset)),
                    ("limit", json!(limit + 1)),
                    ("order", json!("id asc")),
                ]),
                &context,
                &mut up,
            )
            .await;
        let acls = soft(acls, &mut warnings, "ir.model.access")?.map(|rows| {
            let mut rows: Vec<Value> = rows
                .as_array()
                .into_iter()
                .flatten()
                .map(|a| {
                    let applies = match (m2o_id(&a["group_id"]), &groups) {
                        (None, _) => json!(true),
                        (Some(g), Some(mine)) => json!(mine.contains(&g)),
                        (Some(_), None) => Value::Null,
                    };
                    json!({"id": a["id"], "name": a["name"], "group": a["group_id"], "perms": perms(a), "applies": applies})
                })
                .collect();
            continuations.insert("acls".into(), page_of(&mut rows, limit, offset));
            Value::Array(rows)
        });
        let rules = self
            .sub(
                client,
                &target,
                "ir.rule",
                "search_read",
                json!([]),
                kw(&[
                    ("domain", by_model),
                    (
                        "fields",
                        json!([
                            "name",
                            "groups",
                            "domain_force",
                            "perm_read",
                            "perm_write",
                            "perm_create",
                            "perm_unlink"
                        ]),
                    ),
                    ("offset", json!(offset)),
                    ("limit", json!(limit + 1)),
                    ("order", json!("id asc")),
                ]),
                &context,
                &mut up,
            )
            .await;
        let rules = soft(rules, &mut warnings, "ir.rule")?.map(|rows| {
            let mut rows: Vec<Value> = rows
                .as_array()
                .into_iter()
                .flatten()
                .map(|r| {
                    let rule_groups = int_list(&r["groups"]);
                    let applies = match (&groups, rule_groups.is_empty()) {
                        (_, true) => json!(true),
                        (Some(mine), false) => json!(rule_groups.iter().any(|g| mine.contains(g))),
                        (None, false) => Value::Null,
                    };
                    json!({"id": r["id"], "name": r["name"], "groups": r["groups"], "global": rule_groups.is_empty(),
                           "perms": perms(r), "domain": r["domain_force"], "applies": applies})
                })
                .collect();
            continuations.insert("rules".into(), page_of(&mut rows, limit, offset));
            Value::Array(rows)
        });
        let mut meta = Map::new();
        meta.insert("continuations".into(), Value::Object(continuations));
        Ok(finish(
            json!({"rights": rights, "acls": acls, "rules": rules}),
            meta,
            warnings,
            &up,
            format,
            trunc,
        ))
    }

    /// User groups: all_group_ids (19, implied included), groups_id (16–18) or group_ids (19, direct only). None if unreadable.
    async fn user_groups(
        &self,
        client: &ClientIdentity,
        target: &Target,
        context: &Value,
        up: &mut Upstream,
        warnings: &mut Warnings,
    ) -> Result<Option<HashSet<i64>>, ToolError> {
        const NAMES: [&str; 3] = ["all_group_ids", "groups_id", "group_ids"];
        let described = self
            .sub(
                client,
                target,
                "res.users",
                "fields_get",
                json!([]),
                kw(&[("allfields", json!(NAMES)), ("attributes", json!(["type"]))]),
                context,
                up,
            )
            .await;
        let Some(described) = soft(described, warnings, "res.users")? else {
            return Ok(None);
        };
        let Some(field) = NAMES.into_iter().find(|f| described.get(*f).is_some()) else {
            warnings.push((
                "partial_permissions",
                "res.users: no readable group field".into(),
            ));
            return Ok(None);
        };
        if field == "group_ids" {
            warnings.push((
                "groups_direct_only",
                "only direct groups are known; applies may omit implied groups".into(),
            ));
        }
        let user = self
            .read_call_with(
                client,
                target,
                "res.users",
                "read",
                |uid| json!([[uid]]),
                kw(&[("fields", json!([field]))]),
                context,
                up,
            )
            .await;
        let Some(user) = soft(user, warnings, "res.users")? else {
            return Ok(None);
        };
        Ok(Some(int_list(&user[0][field]).into_iter().collect()))
    }

    // compare

    pub(crate) async fn tool_compare(
        &self,
        client: &ClientIdentity,
        arguments: Value,
    ) -> Result<ToolOutput, ToolError> {
        let mut args = Args::new(
            arguments,
            &[
                "env_a", "env_b", "model", "fields", "domain", "key", "context", "limit", "offset",
                "format", "trunc", "to_file",
            ],
        )?;
        let (format, trunc) = common(&mut args)?;
        let env_a = args.str("env_a")?;
        let env_b = args.str("env_b")?;
        if env_a == env_b {
            return Err(ToolError::invalid(
                "env_a and env_b must be distinct environments",
            ));
        }
        let context = Value::Object(args.object("context")?);
        let model = model(&mut args)?;
        if !args.has("fields") {
            return Err(ToolError::invalid("missing fields"));
        }
        let fields = fields(&mut args, &[])?;
        let domain = domain(&mut args)?;
        let key = args.str_opt("key")?.unwrap_or_else(|| "xmlid".into());
        if key != "xmlid" && (!is_identifier(&key, false) || key == "id") {
            return Err(ToolError::invalid(
                "key must be xmlid or an explicit field other than id; IDs do not identify records across databases",
            ));
        }
        let limit = args.int("limit", 50, 1, 1000)? as usize;
        let offset = args.int("offset", 0, 0, i64::MAX)? as usize;
        let to_file = args.bool("to_file")?;
        // Authorize both environments independently before any Odoo call.
        let target_a = self.target(client, &env_a, Capability::OdooRead).await?;
        let target_b = self.target(client, &env_b, Capability::OdooRead).await?;
        if key == "xmlid"
            && let Some(t) = [&target_a, &target_b]
                .into_iter()
                .find(|t| t.env.protocol == crate::config::Protocol::Xmlrpc)
        {
            return Err(ToolError::new(
                "incompatible_protocol",
                format!(
                    "{} uses XML-RPC, which cannot serialize get_external_id integer dictionary keys; use an explicit key field",
                    t.env.name
                ),
                Outcome::NotSent,
            ));
        }
        let started = Upstream::start();
        let query = SideQuery {
            model: &model,
            fields: &fields,
            domain: &domain,
            key: &key,
            limit,
            offset,
            context: &context,
        };
        let (a, b) = tokio::join!(
            self.compare_side(client, &target_a, &query),
            self.compare_side(client, &target_b, &query),
        );
        let (a, b) = (a?, b?);
        let mut up = started;
        up.calls = a.up.calls + b.up.calls;
        up.bytes = a.up.bytes + b.up.bytes;
        let result = diff(&a, &b, &fields, offset);
        let mut warnings: Warnings = a.warnings.iter().chain(&b.warnings).cloned().collect();
        warnings.push((
            "separate_reads",
            "env_a and env_b use independent calls, not one transaction".into(),
        ));
        let partial = a.has_more || b.has_more || offset > 0;
        let inconclusive = !result.ambiguous.is_empty()
            || !result.missing_keys.is_empty()
            || !result.non_comparable.is_empty();
        if partial || inconclusive {
            warnings.push((
                "inconclusive",
                "partial page or incomparable rows: zero differences do not imply equality".into(),
            ));
        }
        let mut meta = Map::new();
        meta.insert(
            "continuations".into(),
            json!({"env_a": a.page, "env_b": b.page}),
        );
        meta.insert("criteria".into(), json!({"key": key, "relations": "xmlid"}));
        let data = json!({
            "differences": result.differences,
            "ambiguous": result.ambiguous,
            "missing_keys": result.missing_keys,
            "non_comparable": result.non_comparable,
            "scanned": {"env_a": a.rows.len(), "env_b": b.rows.len()},
        });
        if to_file {
            let mut flat = Vec::new();
            for (kind, list) in [
                ("difference", &result.differences),
                ("ambiguous", &result.ambiguous),
                ("missing_key", &result.missing_keys),
                ("non_comparable", &result.non_comparable),
            ] {
                for r in list {
                    flat.push(
                        json!({"kind": kind, "key": r.get("key").cloned().unwrap_or(Value::Null),
                        "field": r.get("field").cloned().unwrap_or(Value::Null),
                        "env_a": r.get("env_a").cloned().unwrap_or(Value::Null),
                        "env_b": r.get("env_b").cloned().unwrap_or(Value::Null),
                        "detail": r}),
                    );
                }
            }
            let out = finish(Value::Array(flat), meta, warnings, &up, format, trunc);
            return self
                .output_to_file(out, OutputKind::Tsv, &format!("{model}-compare"))
                .await;
        }
        Ok(finish(data, meta, warnings, &up, format, trunc))
    }

    async fn compare_side(
        &self,
        client: &ClientIdentity,
        target: &Target,
        q: &SideQuery<'_>,
    ) -> Result<Side, ToolError> {
        let mut up = Upstream::start();
        let mut warnings: Warnings = Vec::new();
        let env = target.env.name.clone();
        let mut wanted: Vec<String> = q.fields.to_vec();
        if q.key != "xmlid" && !wanted.iter().any(|f| f == q.key) {
            wanted.push(q.key.into());
        }
        let described = self
            .sub(
                client,
                target,
                q.model,
                "fields_get",
                json!([]),
                kw(&[
                    ("allfields", json!(wanted)),
                    ("attributes", json!(["type", "relation"])),
                ]),
                q.context,
                &mut up,
            )
            .await?;
        let types: BTreeMap<String, (String, Option<String>)> = described
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(name, f)| {
                Some((
                    name.clone(),
                    (
                        f["type"].as_str()?.to_string(),
                        f["relation"].as_str().map(String::from),
                    ),
                ))
            })
            .collect();
        if q.key != "xmlid" {
            match types.get(q.key) {
                None => {
                    return Err(ToolError::invalid(format!(
                        "key field {} does not exist in {env}",
                        q.key
                    )));
                }
                Some((t, _)) if is_relational(t) => {
                    return Err(ToolError::invalid(format!(
                        "key {} is relational ({t}); use a scalar field or xmlid",
                        q.key
                    )));
                }
                Some(_) => {}
            }
        }
        let readable: Vec<&String> = wanted.iter().filter(|f| types.contains_key(*f)).collect();
        let order = if q.key == "xmlid" {
            "id asc".to_string()
        } else {
            format!("{} asc, id asc", q.key)
        };
        let rows = self
            .sub(
                client,
                target,
                q.model,
                "search_read",
                json!([]),
                kw(&[
                    ("domain", q.domain.clone()),
                    ("fields", json!(readable)),
                    ("offset", json!(q.offset)),
                    ("limit", json!(q.limit + 1)),
                    ("order", json!(order)),
                ]),
                q.context,
                &mut up,
            )
            .await?;
        let mut rows = rows
            .as_array()
            .cloned()
            .ok_or_else(|| unexpected("search_read"))?;
        let page = page_of(&mut rows, q.limit, q.offset);
        let has_more = page["has_more"] == json!(true);
        let ids: Vec<i64> = rows.iter().filter_map(|r| r["id"].as_i64()).collect();
        let mut keys: Vec<Option<Value>> = Vec::new();
        if q.key == "xmlid" {
            let ext = if ids.is_empty() {
                json!({})
            } else {
                self.sub(
                    client,
                    target,
                    q.model,
                    "get_external_id",
                    json!([ids]),
                    Map::new(),
                    q.context,
                    &mut up,
                )
                .await?
            };
            for id in &ids {
                keys.push(
                    ext[id.to_string()]
                        .as_str()
                        .filter(|s| !s.is_empty())
                        .map(|s| json!(s)),
                );
            }
        } else {
            for r in &rows {
                let v = &r[q.key];
                keys.push(match v {
                    Value::Null | Value::Bool(false) => None,
                    Value::String(s) if s.is_empty() => None,
                    other => Some(other.clone()),
                });
            }
        }
        // Resolve relational XML IDs per model within a fixed budget.
        let mut related: BTreeMap<String, Vec<i64>> = BTreeMap::new();
        for f in q.fields {
            let Some((t, Some(rel))) = types.get(f) else {
                continue;
            };
            if !is_relational(t) {
                continue;
            }
            let entry = related.entry(rel.clone()).or_default();
            for r in &rows {
                match t.as_str() {
                    "many2one" => entry.extend(m2o_id(&r[f])),
                    _ => entry.extend(int_list(&r[f])),
                }
            }
        }
        let mut xmlids: HashMap<(String, i64), String> = HashMap::new();
        let mut resolved: HashSet<String> = HashSet::new();
        for (index, (rel, mut rel_ids)) in related.into_iter().enumerate() {
            rel_ids.sort_unstable();
            rel_ids.dedup();
            if index >= MAX_RELATION_MODELS || rel_ids.len() > MAX_RELATION_IDS {
                warnings.push((
                    "relations_unresolved",
                    format!("{env}: {rel} relations unresolved within budget"),
                ));
                continue;
            }
            if rel_ids.is_empty() {
                resolved.insert(rel);
                continue;
            }
            if target.env.protocol == crate::config::Protocol::Xmlrpc {
                // Like key=xmlid, avoid sending a call that can only fault.
                warnings.push((
                    "relations_unresolved",
                    format!(
                        "{env}: XML-RPC cannot return get_external_id; {rel} relations lack XML IDs"
                    ),
                ));
                continue;
            }
            let ext = self
                .sub(
                    client,
                    target,
                    &rel,
                    "get_external_id",
                    json!([rel_ids]),
                    Map::new(),
                    q.context,
                    &mut up,
                )
                .await;
            let Some(ext) = soft(ext, &mut warnings, &format!("{env}: {rel}"))? else {
                continue;
            };
            for id in rel_ids {
                if let Some(x) = ext[id.to_string()].as_str().filter(|s| !s.is_empty()) {
                    xmlids.insert((rel.clone(), id), x.into());
                }
            }
            resolved.insert(rel);
        }
        Ok(Side {
            rows,
            keys,
            types,
            xmlids,
            resolved,
            page,
            has_more,
            up,
            warnings,
        })
    }
}

fn warnings_into(meta: &mut Map<String, Value>, code: &str, message: String) {
    let list = meta
        .entry("warnings")
        .or_insert_with(|| Value::Array(vec![]));
    if let Value::Array(list) = list {
        list.push(json!({"code": code, "message": message}));
    }
}

fn is_relational(t: &str) -> bool {
    matches!(t, "many2one" | "one2many" | "many2many")
}

struct SideQuery<'a> {
    model: &'a str,
    fields: &'a [String],
    domain: &'a Value,
    key: &'a str,
    limit: usize,
    offset: usize,
    context: &'a Value,
}

struct Side {
    rows: Vec<Value>,
    keys: Vec<Option<Value>>,
    types: BTreeMap<String, (String, Option<String>)>,
    xmlids: HashMap<(String, i64), String>,
    /// Related models whose XML IDs were queried.
    resolved: HashSet<String>,
    page: Value,
    has_more: bool,
    up: Upstream,
    warnings: Warnings,
}
impl Side {
    /// Cross-database comparison: scalar values unchanged, relations by XML ID.
    fn comparable(&self, field: &str, row: &Value) -> Result<Value, String> {
        let Some((t, rel)) = self.types.get(field) else {
            return Err("field does not exist".into());
        };
        if !is_relational(t) {
            return Ok(row[field].clone());
        }
        let rel = rel.clone().unwrap_or_default();
        if !self.resolved.contains(&rel) {
            return Err(format!("XML IDs for {rel} unavailable"));
        }
        let ids = if t == "many2one" {
            m2o_id(&row[field]).into_iter().collect()
        } else {
            int_list(&row[field])
        };
        let mut out = Vec::new();
        for id in ids {
            match self.xmlids.get(&(rel.clone(), id)) {
                Some(x) => out.push(x.clone()),
                None => return Err(format!("{rel}({id}) has no XML ID; no shared identity")),
            }
        }
        if t == "many2one" {
            return Ok(out.pop().map_or(json!(false), |x| json!(x)));
        }
        out.sort();
        Ok(json!(out))
    }
}

#[derive(Default)]
struct DiffResult {
    differences: Vec<Value>,
    ambiguous: Vec<Value>,
    missing_keys: Vec<Value>,
    non_comparable: Vec<Value>,
}

fn same(a: &Value, b: &Value) -> bool {
    match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) if a.is_number() && b.is_number() => x == y,
        _ => a == b,
    }
}

fn diff(a: &Side, b: &Side, fields: &[String], offset: usize) -> DiffResult {
    let mut out = DiffResult::default();
    // Index canonical JSON keys to distinguish string "1" from number 1.
    let index = |side: &Side, label: &str, out: &mut DiffResult| {
        let mut map: BTreeMap<String, (Value, Vec<usize>)> = BTreeMap::new();
        for (i, key) in side.keys.iter().enumerate() {
            match key {
                None => out
                    .missing_keys
                    .push(json!({"env": label, "id": side.rows[i]["id"]})),
                Some(k) => map
                    .entry(k.to_string())
                    .or_insert_with(|| (k.clone(), vec![]))
                    .1
                    .push(i),
            }
        }
        map
    };
    let ia = index(a, "env_a", &mut out);
    let ib = index(b, "env_b", &mut out);
    // Duplicate keys on either side are incomparable; retain every matching row from both environments.
    let ids = |side: &Side, entry: Option<&(Value, Vec<usize>)>| -> Vec<Value> {
        entry.map_or(vec![], |e| {
            e.1.iter().map(|i| side.rows[*i]["id"].clone()).collect()
        })
    };
    let all_keys: std::collections::BTreeSet<&String> = ia.keys().chain(ib.keys()).collect();
    for k in &all_keys {
        let (ea, eb) = (ia.get(*k), ib.get(*k));
        if ea.is_some_and(|e| e.1.len() > 1) || eb.is_some_and(|e| e.1.len() > 1) {
            let key = ea.or(eb).map(|e| e.0.clone()).unwrap_or(Value::Null);
            out.ambiguous
                .push(json!({"key": key, "ids_a": ids(a, ea), "ids_b": ids(b, eb)}));
        }
    }
    // Fields incomparable in every row.
    let mut comparable_fields = Vec::new();
    for f in fields {
        let reason = if f == "id" {
            Some("IDs are not shared identities across databases".to_string())
        } else {
            match (a.types.get(f), b.types.get(f)) {
                (None, _) => Some("field does not exist in env_a".into()),
                (_, None) => Some("field does not exist in env_b".into()),
                (Some(x), Some(y)) if x != y => Some(format!(
                    "tipo distinto: {}/{:?} frente a {}/{:?}",
                    x.0, x.1, y.0, y.1
                )),
                _ => None,
            }
        };
        match reason {
            Some(reason) => out
                .non_comparable
                .push(json!({"key": null, "field": f, "reason": reason})),
            None => comparable_fields.push(f),
        }
    }
    for k in all_keys {
        let (ea, eb) = (ia.get(k), ib.get(k));
        if ea.is_some_and(|e| e.1.len() > 1) || eb.is_some_and(|e| e.1.len() > 1) {
            continue; // ya listada como ambigua
        }
        match (ea, eb) {
            (Some((key, ra)), None) => out.differences.push(json!({
                "key": key, "field": "*", "env_a": "present", "env_b": "absent",
                "id_a": a.rows[ra[0]]["id"], "certain": !(b.has_more || offset > 0),
            })),
            (None, Some((key, rb))) => out.differences.push(json!({
                "key": key, "field": "*", "env_a": "absent", "env_b": "present",
                "id_b": b.rows[rb[0]]["id"], "certain": !(a.has_more || offset > 0),
            })),
            (Some((key, ra)), Some((_, rb))) => {
                let (row_a, row_b) = (&a.rows[ra[0]], &b.rows[rb[0]]);
                for f in &comparable_fields {
                    match (a.comparable(f, row_a), b.comparable(f, row_b)) {
                        (Ok(va), Ok(vb)) => {
                            if !same(&va, &vb) {
                                out.differences.push(json!({"key": key, "field": f,
                                    "env_a": va, "env_b": vb, "id_a": row_a["id"], "id_b": row_b["id"]}));
                            }
                        }
                        (Err(reason), _) | (_, Err(reason)) => out
                            .non_comparable
                            .push(json!({"key": key, "field": f, "reason": reason})),
                    }
                }
            }
            (None, None) => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_group_specs_are_validated_without_evaluation() {
        for ok in ["partner_id", "date:month", "date_order:iso_week_number"] {
            assert!(valid_groupby(ok), "{ok}");
        }
        for bad in ["", "a:b:c", "x;drop", "date:"] {
            assert!(!valid_groupby(bad), "{bad}");
        }
        for ok in ["amount", "amount:sum", "total:sum(amount)", "id:count"] {
            assert!(valid_aggregate(ok), "{ok}");
        }
        for bad in ["amount:sum(", "t:sum(a))", "a:b(c d)", "x:"] {
            assert!(!valid_aggregate(bad), "{bad}");
        }
        assert!(group_order("partner_id desc, amount:sum").is_ok());
        assert!(group_order("partner_id; drop").is_err());
    }
}
