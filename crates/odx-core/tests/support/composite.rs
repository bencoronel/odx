//! Composite/file simulator responses based on official Odoo 16–19 models.py, orm/models.py (19), mail_tracking_value.py, ir_ui_view.py and res_users.py. base-16 models Odoo 16 tracking columns; base-19 models Odoo 19 user groups. Reject positional arguments where named arguments are required.
#![allow(dead_code)]
use serde_json::{Value, json};

type Answer = Option<Result<Value, String>>;

/// Return the first matching value in [[field, "=", value], ...].
fn term<'a>(domain: &'a Value, field: &str) -> Option<&'a Value> {
    domain
        .as_array()?
        .iter()
        .find(|t| t.get(0).and_then(Value::as_str) == Some(field))
        .and_then(|t| t.get(2))
}
fn page(rows: Vec<Value>, kwargs: &Value) -> Value {
    let offset = kwargs["offset"].as_u64().unwrap_or(0) as usize;
    let limit = kwargs["limit"].as_u64().unwrap_or(1000) as usize;
    Value::Array(rows.into_iter().skip(offset).take(limit).collect())
}
fn only(row: Value, kwargs: &Value) -> Value {
    let Some(fields) = kwargs["fields"].as_array() else {
        return row;
    };
    let mut out = serde_json::Map::new();
    out.insert("id".into(), row["id"].clone());
    for f in fields.iter().filter_map(Value::as_str) {
        if let Some(v) = row.get(f) {
            out.insert(f.into(), v.clone());
        }
    }
    Value::Object(out)
}
fn described(all: Value, kwargs: &Value) -> Value {
    let wanted: Vec<&str> = kwargs["allfields"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let mut out = serde_json::Map::new();
    for (k, v) in all.as_object().unwrap() {
        if wanted.is_empty() || wanted.contains(&k.as_str()) {
            out.insert(k.clone(), v.clone());
        }
    }
    Value::Object(out)
}
fn ids(args: &[Value]) -> Vec<i64> {
    args.first()
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_i64).collect())
        .unwrap_or_default()
}
fn named(args: &[Value], method: &str) -> Result<(), String> {
    if args.is_empty() {
        Ok(())
    } else {
        Err(format!("{method}: simulator requires named arguments"))
    }
}
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

pub fn respond(db: &str, model: &str, method: &str, args: &[Value], kwargs: &Value) -> Answer {
    let r = match (model, method) {
        // wait returns one record until the domain timestamp in ms, then zero.
        ("x.queue", "search_count") => named(args, method).map(|_| {
            let until = term(&kwargs["domain"], "create_date")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            json!(if now_ms() < until { 1 } else { 0 })
        }),
        ("x.forbidden", _) => Err(format!("You are not allowed to access {model}")),
        // ref IDs 1–3 exist.
        (_, "search_count") if term(&kwargs["domain"], "id").is_some() => named(args, method)
            .map(|_| {
                let id = term(&kwargs["domain"], "id").and_then(Value::as_i64).unwrap_or(0);
                json!(if (1..=3).contains(&id) { 1 } else { 0 })
            }),
        ("ir.model.data", "search_read") => named(args, method).map(|_| {
            let module = term(&kwargs["domain"], "module").and_then(Value::as_str);
            let name = term(&kwargs["domain"], "name").and_then(Value::as_str);
            json!(match (module, name) {
                (Some("base"), Some("main_company")) =>
                    vec![json!({"id":1,"model":"res.company","res_id":1})],
                (Some("base"), Some("dangling")) =>
                    vec![json!({"id":2,"model":"res.partner","res_id":999})],
                (Some("base"), Some("secret")) =>
                    vec![json!({"id":3,"model":"x.forbidden","res_id":1})],
                _ => vec![],
            })
        }),
        ("res.partner", "read_group") => named(args, method).and_then(|_| {
            if kwargs["lazy"] != json!(false) {
                return Err("read_group: expected lazy=false".into());
            }
            if kwargs["groupby"] == json!([]) {
                if kwargs["fields"] != json!(["amount_total:sum"]) { return Err("unexpected total aggregate".into()); }
                return Ok(page(vec![json!({"__count":15,"amount_total":150})], kwargs));
            }
            let groups = (1..=5)
                .map(|i| json!({"parent_id":[i,format!("P{i}")],"__count":i,"amount_total":i*10,
                                "__domain":[["parent_id","=",i]],"__range":{}}))
                .collect();
            Ok(page(groups, kwargs))
        }),
        // tracking_value_ids requires base.group_system in 16–19; base-b models a user receiving AccessError without it.
        ("mail.message", "search_read") if db == "base-b"
            && kwargs["fields"].as_array().is_some_and(|f| f.contains(&json!("tracking_value_ids"))) =>
        {
            Err("The requested operation can not be completed due to security restrictions.".into())
        }
        ("mail.message", "search_read") => named(args, method).map(|_| {
            let res_id = term(&kwargs["domain"], "res_id").and_then(Value::as_i64);
            let rows = match res_id {
                Some(1) => vec![
                    json!({"id":30,"date":"2026-10-03 10:00:00","author_id":[3,"Admin"],"message_type":"notification",
                           "subtype_id":[2,"Note"],"body":"<p>Updated</p>","tracking_value_ids":[301,302]}),
                    json!({"id":20,"date":"2026-10-02 10:00:00","author_id":[3,"Admin"],"message_type":"comment",
                           "subtype_id":[1,"Discussions"],"body":"<p>Comentario</p>","tracking_value_ids":[]}),
                    json!({"id":10,"date":"2026-10-01 10:00:00","author_id":false,"message_type":"notification",
                           "subtype_id":false,"body":"<p>Created</p>","tracking_value_ids":[101]}),
                ],
                _ => vec![],
            };
            page(rows.into_iter().map(|r| only(r, kwargs)).collect(), kwargs)
        }),
        ("mail.tracking.value", "fields_get") => named(args, method).map(|_| {
            let all = if db == "base-16" {
                json!({"field":{"type":"many2one"},"field_desc":{"type":"char"},"field_type":{"type":"char"},
                       "mail_message_id":{"type":"many2one"},
                       "old_value_integer":{"type":"integer"},"old_value_float":{"type":"float"},
                       "old_value_monetary":{"type":"float"},"old_value_char":{"type":"char"},
                       "old_value_text":{"type":"text"},"old_value_datetime":{"type":"datetime"},
                       "new_value_integer":{"type":"integer"},"new_value_float":{"type":"float"},
                       "new_value_monetary":{"type":"float"},"new_value_char":{"type":"char"},
                       "new_value_text":{"type":"text"},"new_value_datetime":{"type":"datetime"}})
            } else {
                json!({"field_id":{"type":"many2one"},"field_info":{"type":"json"},
                       "mail_message_id":{"type":"many2one"},
                       "old_value_integer":{"type":"integer"},"old_value_float":{"type":"float"},
                       "old_value_char":{"type":"char"},"old_value_text":{"type":"text"},
                       "old_value_datetime":{"type":"datetime"},
                       "new_value_integer":{"type":"integer"},"new_value_float":{"type":"float"},
                       "new_value_char":{"type":"char"},"new_value_text":{"type":"text"},
                       "new_value_datetime":{"type":"datetime"}})
            };
            described(all, kwargs)
        }),
        // mail.tracking.value ACLs deny reads without the system group in 16–19.
        ("mail.tracking.value", "search_read") if db == "base-b" => {
            Err("You are not allowed to access 'Mail Tracking Value' (mail.tracking.value) records.".into())
        }
        ("mail.tracking.value", "search_read") => named(args, method).map(|_| {
            let messages: Vec<i64> = term(&kwargs["domain"], "mail_message_id")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_i64).collect())
                .unwrap_or_default();
            let v16 = db == "base-16";
            let field = if v16 { "field" } else { "field_id" };
            let mut rows = vec![
                json!({"id":101,"mail_message_id":[10,"m"],field:[503,"Partner"],"field_desc":"Partner","field_type":"many2one",
                       "old_value_integer":0,"old_value_char":"","new_value_integer":7,"new_value_char":"ACME"}),
                json!({"id":301,"mail_message_id":[30,"m"],field:[501,"Status"],"field_desc":"Status","field_type":"selection",
                       "old_value_char":"Draft","new_value_char":"Done"}),
            ];
            rows.push(if v16 {
                json!({"id":302,"mail_message_id":[30,"m"],"field":[502,"Amount"],"field_desc":"Amount","field_type":"monetary",
                       "old_value_float":0.0,"new_value_float":0.0,"old_value_monetary":10.0,"new_value_monetary":12.5})
            } else {
                json!({"id":302,"mail_message_id":[30,"m"],"field_id":[502,"Amount"],
                       "old_value_float":10.0,"new_value_float":12.5})
            });
            page(
                rows.into_iter()
                    .filter(|r| messages.contains(&r["mail_message_id"][0].as_i64().unwrap()))
                    .map(|r| only(r, kwargs))
                    .collect(),
                kwargs,
            )
        }),
        ("ir.model.fields", "read") => Ok(json!(
            [
                json!({"id":501,"name":"state","field_description":"Status","ttype":"selection"}),
                json!({"id":502,"name":"amount","field_description":"Amount","ttype":"monetary"}),
                json!({"id":503,"name":"partner_id","field_description":"Partner","ttype":"many2one"}),
            ]
            .into_iter()
            .filter(|r| ids(args).contains(&r["id"].as_i64().unwrap()))
            .collect::<Vec<_>>()
        )),
        (_, "get_views") => named(args, method).and_then(|_| {
            let requested = &kwargs["views"][0];
            let view_type = requested[1].as_str().unwrap_or("form").to_string();
            let (id, arch) = match model {
                "sale.order" if view_type == "kanban" => (json!(false), "<kanban/>".to_string()),
                "sale.order" => (
                    json!(100),
                    "<form><sheet><field name=\"name\"/><field name=\"x_foo\"/><field name=\"partner_id\"/></sheet></form>"
                        .to_string(),
                ),
                "x.cycle" => (json!(200), "<form><field name=\"name\"/></form>".to_string()),
                "x.noviews" => (json!(300), "<form/>".to_string()),
                "x.medium" => (
                    json!(false),
                    format!("<form>{}</form>", "<field name=\"name\"/>".repeat(7_300)),
                ),
                "x.big" => (
                    json!(false),
                    format!("<form>{}</form>", "<field name=\"name\"/>".repeat(12_000)),
                ),
                _ => return Err(format!("View types not defined {view_type} found in act_window action")),
            };
            Ok(json!({"views": {view_type: {"arch": arch, "id": id, "model": model}}, "models": {}}))
        }),
        ("ir.ui.view", "read") => {
            let id = ids(args).first().copied().unwrap_or(0);
            let view = |id: i64, mode: &str, inherit: Value, xml_id: &str, arch: &str| {
                json!({"id":id,"name":format!("view-{id}"),"priority":16,"mode":mode,"inherit_id":inherit,
                       "xml_id":xml_id,"model":"sale.order","arch_db":arch})
            };
            match id {
                100 => Ok(json!([view(100, "primary", json!(false), "sale.view_order_form", "<form><field name=\"name\"/></form>")])),
                200 => Ok(json!([view(200, "primary", json!([201, "v"]), "x.v200", "<form/>")])),
                201 => Ok(json!([view(201, "primary", json!([200, "v"]), "x.v201", "<form/>")])),
                300 => Err("You are not allowed to access 'View' (ir.ui.view) records.".into()),
                _ => Ok(json!([])),
            }
        }
        ("ir.ui.view", "search_read") => named(args, method).map(|_| {
            let parents: Vec<i64> = term(&kwargs["domain"], "inherit_id")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_i64).collect())
                .unwrap_or_default();
            let ext = |id: i64, parent: i64, xml_id: &str, arch: &str| {
                json!({"id":id,"name":format!("view-{id}"),"priority":16,"mode":"extension",
                       "inherit_id":[parent,"v"],"xml_id":xml_id,"model":"sale.order","arch_db":arch})
            };
            let mut rows = vec![];
            if parents.contains(&100) {
                rows.push(ext(101, 100, "x_mod.view_order_form_x_foo", "<field name=\"x_foo\"/>"));
                rows.push(ext(102, 100, "sale_stock.view_order_form", "<field name=\"warehouse_id\"/>"));
            }
            if parents.contains(&101) {
                rows.push(ext(103, 101, "x_mod2.deeper", "<field name=\"partner_id\"/>"));
            }
            if parents.contains(&200) {
                // An extension points back to a visited view, forming a cycle.
                rows.push(ext(201, 200, "x.v201", "<form/>"));
            }
            page(rows, kwargs)
        }),
        (_, "check_access_rights") => named(args, method).map(|_| {
            let op = kwargs["operation"].as_str().unwrap_or("");
            json!(matches!(op, "read") || (model == "res.partner" && op == "write"))
        }),
        ("res.users", "fields_get") => named(args, method).map(|_| {
            let all = if db == "base-19" {
                json!({"all_group_ids":{"type":"many2many"},"group_ids":{"type":"many2many"}})
            } else {
                json!({"groups_id":{"type":"many2many"}})
            };
            described(all, kwargs)
        }),
        ("res.users", "read")
            if kwargs["fields"].as_array().is_some_and(|f| {
                f.iter().any(|f| {
                    matches!(f.as_str(), Some("groups_id" | "group_ids" | "all_group_ids"))
                })
            }) =>
        {
            let field = kwargs["fields"][0].as_str().unwrap().to_string();
            Ok(json!([{"id": ids(args).first(), field: [1, 2]}]))
        }
        ("ir.model.access", "search_read") => named(args, method).and_then(|_| {
            match term(&kwargs["domain"], "model_id.model").and_then(Value::as_str) {
                Some("x.secret") => Err("You are not allowed to access 'Access Controls' (ir.model.access) records.".into()),
                Some("res.partner") => Ok(page(
                    vec![
                        json!({"id":1,"name":"partner user","group_id":[1,"User"],"perm_read":true,"perm_write":true,"perm_create":true,"perm_unlink":false}),
                        json!({"id":2,"name":"partner manager","group_id":[9,"Manager"],"perm_read":true,"perm_write":true,"perm_create":true,"perm_unlink":true}),
                        json!({"id":3,"name":"public","group_id":false,"perm_read":true,"perm_write":false,"perm_create":false,"perm_unlink":false}),
                    ],
                    kwargs,
                )),
                _ => Ok(json!([])),
            }
        }),
        ("ir.rule", "search_read") => named(args, method).map(|_| {
            match term(&kwargs["domain"], "model_id.model").and_then(Value::as_str) {
                Some("res.partner") => page(
                    vec![
                        json!({"id":7,"name":"company rule","groups":[],"domain_force":"[('company_id', 'in', company_ids)]",
                               "perm_read":true,"perm_write":true,"perm_create":true,"perm_unlink":true}),
                        json!({"id":8,"name":"portal","groups":[10],"domain_force":"[('id', '=', user.partner_id.id)]",
                               "perm_read":true,"perm_write":false,"perm_create":false,"perm_unlink":false}),
                    ],
                    kwargs,
                ),
                _ => json!([]),
            }
        }),
        ("account.tax", "fields_get") => named(args, method).map(|_| {
            let mut all = json!({"name":{"type":"char"},"amount":{"type":"float"},
                "country_id":{"type":"many2one","relation":"res.country"},
                "tag_ids":{"type":"many2many","relation":"account.account.tag"},
                "description":{"type":"char"}});
            if db == "base-a" {
                all["only_a"] = json!({"type":"char"});
            } else {
                all["description"] = json!({"type":"html"});
            }
            described(all, kwargs)
        }),
        ("account.tax", "search_read") => named(args, method).map(|_| {
            let rows = if db == "base-a" {
                vec![
                    json!({"id":1,"name":"VAT 21","amount":21.0,"country_id":[1,"Spain"],"tag_ids":[5,6],"description":"a","only_a":"x"}),
                    json!({"id":2,"name":"VAT 10","amount":10.0,"country_id":[1,"Spain"],"tag_ids":[7],"description":"a","only_a":"x"}),
                    json!({"id":3,"name":"Manual","amount":5.0,"country_id":false,"tag_ids":[],"description":"a","only_a":"x"}),
                    json!({"id":4,"name":"VAT 10","amount":10.0,"country_id":[1,"Spain"],"tag_ids":[],"description":"a","only_a":"x"}),
                ]
            } else {
                vec![
                    json!({"id":11,"name":"VAT 21","amount":21.0,"country_id":[9,"Spain"],"tag_ids":[15,17],"description":"<p>b</p>"}),
                    json!({"id":12,"name":"VAT 10","amount":10.5,"country_id":[9,"Spain"],"tag_ids":[17,18],"description":"<p>b</p>"}),
                    json!({"id":13,"name":"Only B","amount":1.0,"country_id":false,"tag_ids":[],"description":""}),
                ]
            };
            page(rows.into_iter().map(|r| only(r, kwargs)).collect(), kwargs)
        }),
        (_, "get_external_id") => {
            let table: &[(i64, &str)] = match (db, model) {
                ("base-a", "account.tax") => &[(1, "l10n.tax21"), (2, "l10n.tax10")],
                ("base-b", "account.tax") => &[(11, "l10n.tax21"), (12, "l10n.tax10"), (13, "l10n.onlyb")],
                ("base-a", "res.country") => &[(1, "base.es")],
                ("base-b", "res.country") => &[(9, "base.es")],
                ("base-a", "account.account.tag") => &[(5, "l10n.tag_a"), (6, "l10n.tag_b"), (7, "l10n.tag_c")],
                ("base-b", "account.account.tag") => &[(15, "l10n.tag_a"), (17, "l10n.tag_c")],
                _ => &[],
            };
            let mut out = serde_json::Map::new();
            for id in ids(args) {
                let x = table.iter().find(|(i, _)| *i == id).map_or("", |(_, x)| *x);
                out.insert(id.to_string(), json!(x));
            }
            Ok(Value::Object(out))
        }
        ("ir.attachment", "fields_get") => named(args, method).map(|_| {
            described(
                json!({"datas":{"type":"binary"},"name":{"type":"char"},"mimetype":{"type":"char"}}),
                kwargs,
            )
        }),
        ("ir.attachment", "read") => {
            let bin_size = kwargs["context"]["bin_size"] == json!(true);
            let row = match ids(args).first() {
                Some(9) => Some(json!({"id":9,"datas": if bin_size { json!("14.00 bytes") } else { json!("JVBERiBmaXh0dXJlAAE=") },
                                       "name":"../../etc/passwd","mimetype":"application/pdf"})),
                Some(10) => Some(json!({"id":10,"datas":false,"name":"empty.txt","mimetype":"text/plain"})),
                Some(11) => Some(json!({"id":11,"datas":"%%%no-base64%%%","name":"bad.bin","mimetype":"application/octet-stream"})),
                // Zero-filled base64 uses 4/3 of decoded size. 13 MiB exceeds ordinary 16 MiB responses; 30 MiB fits download; 33 MiB exceeds decoded 32 MiB while fitting encoded 48 MiB; encoded 50 MiB exceeds download responses.
                Some(id @ 12..=14) => {
                    let mib = match id { 12 => 13, 13 => 30, _ => 33 };
                    Some(json!({"id":id,"datas":"A".repeat(mib * 1024 * 1024 / 3 * 4),"name":"big.bin","mimetype":"application/octet-stream"}))
                }
                Some(15) => Some(json!({"id":15,"datas":"A".repeat(50 * 1024 * 1024),"name":"huge.bin","mimetype":"application/octet-stream"})),
                _ => None,
            };
            Ok(json!(row.into_iter().map(|r| only(r, kwargs)).collect::<Vec<_>>()))
        }
        _ => return None,
    };
    Some(r)
}
