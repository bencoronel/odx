//! The view tool: a view's inheritance chain with combined and per-view architecture.
use super::*;

/// Maximum view inheritance depth in either direction.
const MAX_VIEW_DEPTH: usize = 16;
/// Maximum views in a chain.
const MAX_VIEWS: usize = 500;
/// Maximum inline view architecture in transported bytes; use q or to_file for more.
const ARCH_BUDGET: usize = 192 * 1024;
/// Byte index where the architecture exceeds ARCH_BUDGET as sent, or None if it fits. Compact text escapes the arch twice (JSON data, then the JSON-RPC string); JSON format also sends it once escaped as structuredContent.
fn arch_cut(arch: &str, format: Format) -> Option<usize> {
    let mut used = 0;
    for (i, c) in arch.char_indices() {
        // Bytes after one and two rounds of JSON string escaping.
        let (once, twice) = match c {
            '"' | '\\' => (2, 4),
            '\n' | '\r' | '\t' | '\u{8}' | '\u{c}' => (2, 3),
            c if c < ' ' => (6, 7),
            c => (c.len_utf8(), c.len_utf8()),
        };
        used += twice + if format == Format::Json { once } else { 0 };
        if used > ARCH_BUDGET {
            return Some(i);
        }
    }
    None
}

impl Core {
    pub(crate) async fn tool_view(
        &self,
        client: &ClientIdentity,
        arguments: Value,
    ) -> Result<ToolOutput, ToolError> {
        let mut args = Args::for_tool("view", arguments)?;
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
                warnings.push((
                    "matches_truncated",
                    format!("showing only {MAX_MATCHES} matches; use to_file"),
                ));
            }
            if matches.is_empty() {
                warnings.push((
                    "no_match",
                    format!("'{q}' not found in the final architecture"),
                ));
            }
            data["arch"] = Value::Null;
            data["matches"] = json!(matches);
        } else if let Some(cut) = arch_cut(&arch, format) {
            // Mark truncation after a closing tag.
            let cut = arch[..cut].rfind('>').map_or(cut, |i| i + 1);
            data["arch"] = json!(&arch[..cut]);
            meta.insert("truncated".into(), json!(true));
            warnings.push((
                "arch_truncated",
                format!("architecture truncated to {cut} of {arch_bytes} bytes; use q or to_file"),
            ));
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_dense_arch_cut_fits_the_output_limit() {
        let arch = "<field name=\"a\" string=\"b\"/>".repeat(20_000);
        for format in [Format::Compact, Format::Json] {
            let cut = arch_cut(&arch, format).unwrap();
            let out = ToolOutput {
                data: json!({"view_id": 1, "view_type": "form", "arch_bytes": arch.len(),
                             "views": [], "arch": &arch[..cut]}),
                meta: Map::new(),
                format,
                trunc: 0,
            };
            assert!(crate::output::fit(out).is_ok(), "{format:?}");
        }
        assert_eq!(arch_cut("<form/>", Format::Json), None);
    }
}
