//! The compare tool: the same records read from two environments, matched by XML ID or a key field.
use super::*;

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
                    "type differs: {}/{:?} vs {}/{:?}",
                    x.0, x.1, y.0, y.1
                )),
                (Some((t, _)), _) if matches!(t.as_str(), "reference" | "many2one_reference") => {
                    Some("record references are not shared identities across databases".into())
                }
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
            continue; // already listed as ambiguous
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

impl Core {
    pub(crate) async fn tool_compare(
        &self,
        client: &ClientIdentity,
        arguments: Value,
    ) -> Result<ToolOutput, ToolError> {
        let mut args = Args::for_tool("compare", arguments)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn side(row: Value, types: &[(&str, &str)]) -> Side {
        Side {
            keys: vec![Some(json!("k"))],
            rows: vec![row],
            types: types
                .iter()
                .map(|(f, t)| (f.to_string(), (t.to_string(), None)))
                .collect(),
            xmlids: HashMap::new(),
            resolved: HashSet::new(),
            page: Value::Null,
            has_more: false,
            up: Upstream::start(),
            warnings: vec![],
        }
    }

    #[test]
    fn record_references_are_not_compared_across_databases() {
        let types = [
            ("res_id", "many2one_reference"),
            ("ref", "reference"),
            ("name", "char"),
        ];
        let a = side(
            json!({"id": 1, "res_id": 7, "ref": "res.partner,7", "name": "x"}),
            &types,
        );
        let b = side(
            json!({"id": 2, "res_id": 9, "ref": "res.partner,9", "name": "x"}),
            &types,
        );
        let fields = ["res_id", "ref", "name"].map(String::from);
        let out = diff(&a, &b, &fields, 0);
        assert!(out.differences.is_empty(), "{:?}", out.differences);
        let skipped: Vec<_> = out
            .non_comparable
            .iter()
            .map(|n| n["field"].clone())
            .collect();
        assert_eq!(skipped, [json!("res_id"), json!("ref")]);
    }
}
