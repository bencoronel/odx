use crate::error::{OdxError, Result};
use serde_json::Value;

pub fn validate_create(values: &Value) -> Result<()> {
    let valid = match values {
        Value::Object(values) => !values.is_empty(),
        Value::Array(list) => {
            !list.is_empty()
                && list.len() <= 1000
                && list
                    .iter()
                    .all(|v| v.as_object().is_some_and(|m| !m.is_empty()))
        }
        _ => false,
    };
    if !valid {
        return Err(OdxError::validation(
            "validation.console.write_values_invalid",
            "values must be a nonempty object or 1–1000 nonempty objects",
        ));
    }
    Ok(())
}

/// Positional `index` or named `name`; an error when missing or given both ways.
pub(crate) fn parameter<'a>(
    args: &'a [Value],
    kwargs: &'a serde_json::Map<String, Value>,
    index: usize,
    name: &str,
) -> Result<&'a Value> {
    match (args.get(index), kwargs.get(name)) {
        (Some(_), Some(_)) => Err(OdxError::validation(
            "validation.console.parameter_duplicated",
            format!("duplicate parameter: {name}"),
        )),
        (Some(value), None) | (None, Some(value)) => Ok(value),
        (None, None) => Err(OdxError::validation(
            "validation.console.parameter_required",
            format!("missing {name}"),
        )),
    }
}

/// An ID list as integers; validate_ids checks the values.
pub(crate) fn integer_ids(value: &Value) -> Result<Vec<i64>> {
    serde_json::from_value(value.clone()).map_err(|_| {
        OdxError::validation(
            "validation.console.ids_must_be_integers",
            "ids must be an integer list",
        )
    })
}

/// Values of a write that targets existing records: write(ids, vals) and web_save with IDs.
fn written_values<'a>(
    method: &str,
    args: &'a [Value],
    kwargs: &'a serde_json::Map<String, Value>,
) -> Option<&'a serde_json::Map<String, Value>> {
    let ids = args.first().or_else(|| kwargs.get("ids"))?;
    let has_ids = ids.as_array().is_some_and(|ids| !ids.is_empty());
    if method != "write" && !(method == "web_save" && has_ids) {
        return None;
    }
    args.get(1).or_else(|| kwargs.get("vals"))?.as_object()
}

/// Values Odoo turns into destructive x2many commands if the field is one2many/many2many:
/// false/null becomes Command.clear() and a list whose first item is not a list Command.set(),
/// as in Odoo's _X2Many.write ([5.0] and ["5"] included).
fn shorthand(v: &Value) -> bool {
    v.is_null()
        || *v == Value::Bool(false)
        || v.as_array()
            .is_some_and(|a| a.first().is_some_and(|f| !f.is_array()))
}

/// Keys with shorthand values. Only field types can tell them apart from `{"active": false}`,
/// so the caller resolves these names with fields_get.
pub fn shorthand_keys(
    method: &str,
    args: &[Value],
    kwargs: &serde_json::Map<String, Value>,
) -> Vec<String> {
    written_values(method, args, kwargs)
        .into_iter()
        .flatten()
        .filter(|(_, v)| shorthand(v))
        .map(|(k, _)| k.clone())
        .collect()
}

/// (x2many field, comodel key) pairs with shorthand values inside update commands ([1, id, {..}]).
/// A record being created ([0, 0, {..}]) has no links to remove. The caller resolves the comodel field types.
pub fn nested_shorthand_keys(
    method: &str,
    args: &[Value],
    kwargs: &serde_json::Map<String, Value>,
) -> Vec<(String, String)> {
    let mut keys = Vec::new();
    for (field, value) in written_values(method, args, kwargs).into_iter().flatten() {
        for command in value.as_array().into_iter().flatten() {
            let Some([code, _, Value::Object(values)]) = command.as_array().map(Vec::as_slice)
            else {
                continue;
            };
            if matches!(command_code(code), Some(Some(1))) {
                keys.extend(
                    values
                        .iter()
                        .filter(|(_, v)| shorthand(v))
                        .map(|(k, _)| (field.clone(), k.clone())),
                );
            }
        }
    }
    keys
}

/// `x2many`: names from shorthand_keys that fields_get reported as one2many/many2many.
/// `nested`: pairs from nested_shorthand_keys whose comodel field is one2many/many2many.
pub fn call_effect(
    method: &str,
    args: &[Value],
    kwargs: &serde_json::Map<String, Value>,
    x2many: &[String],
    nested: usize,
) -> Result<crate::policy::Effect> {
    let shorthand = nested
        + written_values(method, args, kwargs).map_or(0, |values| {
            x2many.iter().filter(|k| values.contains_key(*k)).count()
        });
    use crate::policy::Effect;
    let parameter = |index: usize, name: &str| parameter(args, kwargs, index, name);
    let ids = || integer_ids(parameter(0, "ids")?);
    match method {
        "unlink" | "write" => {
            let ids = ids()?;
            validate_ids(&ids)?;
            // One individual approval covers every listed ID.
            if method == "unlink" {
                return Ok(Effect::Delete);
            }
            let values = parameter(1, "vals")?;
            validate_write(&ids, values)?;
            single_delete(relational_delete_count(values) + shorthand, ids.len())
        }
        "create" => {
            let values = parameter(0, "vals_list")?;
            validate_create(values)?;
            if has_relational_delete(values) {
                return Err(OdxError::validation(
                    "validation.console.create_relational_delete",
                    "create forbids relational delete commands",
                ));
            }
            Ok(Effect::Write)
        }
        // web_save(ids, vals, specification) creates without IDs and writes otherwise.
        "web_save" => {
            let ids = ids()?;
            let values = parameter(1, "vals")?;
            if ids.is_empty() {
                validate_create(values)?;
                if has_relational_delete(values) {
                    return Err(OdxError::validation(
                        "validation.console.web_save_relational_delete",
                        "web_save without IDs creates records and forbids relational deletes",
                    ));
                }
                return Ok(Effect::Write);
            }
            validate_write(&ids, values)?;
            single_delete(relational_delete_count(values) + shorthand, ids.len())
        }
        // copy(ids, default) creates copies; defaults cannot contain relational deletes.
        "copy" => {
            let ids = ids()?;
            validate_ids(&ids)?;
            if args
                .iter()
                .skip(1)
                .chain(kwargs.values())
                .any(has_relational_delete)
            {
                return Err(OdxError::validation(
                    "validation.console.copy_relational_delete",
                    "copy forbids relational delete commands in defaults",
                ));
            }
            Ok(Effect::Write)
        }
        // Custom method effects are opaque; add known signatures when further restrictions are needed. Inspect arguments individually; the first IDs list is not a command. Relational deletes require individual approval, as in write.
        _ => {
            let deletes = args
                .iter()
                .chain(kwargs.values())
                .map(relational_delete_count)
                .sum();
            single_delete(deletes, 1)
        }
    }
}
/// Relational deletes require one record, one command and individual approval.
fn single_delete(deletes: usize, records: usize) -> Result<crate::policy::Effect> {
    use crate::policy::Effect;
    if deletes == 0 {
        return Ok(Effect::Write);
    }
    if records != 1 || deletes != 1 {
        return Err(OdxError::validation(
            "validation.console.single_relational_delete",
            "a relational delete must target one record and one command per request",
        ));
    }
    Ok(Effect::Delete)
}
pub fn validate_write(ids: &[i64], values: &Value) -> Result<()> {
    validate_ids(ids)?;
    if !values.is_object() || values.as_object().is_some_and(|m| m.is_empty()) {
        return Err(OdxError::validation(
            "validation.console.write_values_empty",
            "write values cannot be empty",
        ));
    }
    Ok(())
}
pub fn validate_ids(ids: &[i64]) -> Result<()> {
    if ids.is_empty()
        || ids.len() > 1000
        || ids.iter().any(|v| *v <= 0)
        || ids.iter().collect::<std::collections::HashSet<_>>().len() != ids.len()
    {
        return Err(OdxError::validation(
            "validation.console.ids_invalid",
            "IDs must be unique, positive and limited to 1000",
        ));
    }
    Ok(())
}
pub fn has_relational_delete(values: &Value) -> bool {
    relational_delete_count(values) > 0
}
/// Python treats 2.0 == 2; recognize integral floats as command codes. Treat fractional codes in range as potential deletes to prevent bypasses.
fn command_code(value: &Value) -> Option<Option<i64>> {
    let code = value.as_f64().filter(|code| (0.0..=6.0).contains(code))?;
    Some((code.fract() == 0.0).then_some(code as i64))
}
/// x2many command list: an array containing a command array such as [1, id, {..}] or [0, 0, {..}].
pub fn is_command_list(value: &Value) -> bool {
    value.as_array().is_some_and(|a| {
        a.iter().any(|c| {
            c.as_array()
                .and_then(|c| command_code(c.first()?))
                .is_some()
        })
    })
}
pub fn relational_delete_count(values: &Value) -> usize {
    fn visit(v: &Value) -> usize {
        match v {
            Value::Array(a) => a
                .iter()
                .map(|value| {
                    if let Some((command, code)) = value
                        .as_array()
                        .and_then(|c| Some((c, command_code(c.first()?)?)))
                    {
                        usize::from(!matches!(code, Some(0 | 1 | 4)))
                            + command
                                .iter()
                                .filter(|v| v.is_object())
                                .map(visit)
                                .sum::<usize>()
                    } else {
                        visit(value)
                    }
                })
                .sum(),
            Value::Object(o) => o.values().map(visit).sum(),
            _ => 0,
        }
    }
    visit(values)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn detects_relational_delete_commands() {
        assert!(has_relational_delete(
            &serde_json::json!({"tag_ids":[[3,12,0]]})
        ));
        assert!(!has_relational_delete(
            &serde_json::json!({"name":"fixture"})
        ));
        assert_eq!(
            relational_delete_count(&serde_json::json!({"tag_ids":[[2,12],[3,13]]})),
            2
        );
        assert_eq!(
            relational_delete_count(&serde_json::json!({"tag_ids":[[6,0,[3,5]]]})),
            1
        );
        assert!(!has_relational_delete(&serde_json::json!({"ids":[3,5]})));
        assert!(has_relational_delete(
            &serde_json::json!({"lines":[[1,8,{"tags":[[5,0,0]]}]]})
        ));
    }

    #[test]
    fn call_classifies_known_mutations_without_confusing_ids_with_commands() {
        use crate::policy::Effect;
        use serde_json::json;
        let effect = |method, args: Value, kwargs: Value| {
            call_effect(
                method,
                args.as_array().unwrap(),
                kwargs.as_object().unwrap(),
                &[],
                0,
            )
        };
        assert_eq!(
            effect("action_confirm", json!([[3, 5]]), json!({})).unwrap(),
            Effect::Write
        );
        assert_eq!(
            effect("write", json!([[3,5],{"name":"x"}]), json!({})).unwrap(),
            Effect::Write
        );
        assert_eq!(
            effect("create", json!([]), json!({"vals_list":{"name":"x"}})).unwrap(),
            Effect::Write
        );
        assert_eq!(
            effect("unlink", json!([]), json!({"ids":[3]})).unwrap(),
            Effect::Delete
        );
        for command in [
            json!([2, 8, 0]),
            json!([3, 8, 0]),
            json!([5, 0, 0]),
            json!([6, 0, [3, 5]]),
        ] {
            assert_eq!(
                effect("write", json!([[1]]), json!({"vals":{"tags":[command]}})).unwrap(),
                Effect::Delete
            );
        }
        assert_eq!(
            effect("unlink", json!([[1, 2]]), json!({})).unwrap(),
            Effect::Delete
        );
        assert!(effect("unlink", json!([[1]]), json!({"ids":[2]})).is_err());
        assert!(
            effect(
                "write",
                json!([[1],{"name":"x"}]),
                json!({"vals":{"name":"y"}})
            )
            .is_err()
        );
        assert!(effect("write", json!([[1,2],{"tags":[[3,8,0]]}]), json!({})).is_err());
        assert!(effect("create", json!([{"tags":[[2,8,0]]}]), json!({})).is_err());
    }

    #[test]
    fn shorthand_follows_odoo_first_item_rule_at_top_level_and_in_nested_commands() {
        use serde_json::json;
        let kwargs = |vals: Value| json!({"vals": vals}).as_object().unwrap().clone();
        let vals = kwargs(json!({
            "order_line": [5.0], "tag_ids": ["5"], "child_ids": false, "name": "x", "lines": [[0, 0, {}]],
        }));
        let mut keys = shorthand_keys("write", &[json!([1])], &vals);
        keys.sort();
        assert_eq!(keys, ["child_ids", "order_line", "tag_ids"]);
        let vals = kwargs(
            json!({"sale_order_ids": [[1, 5, {"order_line": false, "note": "x"}], [0, 0, {"tag_ids": [2]}], [4, 6, 0]]}),
        );
        assert_eq!(
            nested_shorthand_keys("write", &[json!([1])], &vals),
            // New children ([0, 0, ..]) have no links to clear.
            [("sale_order_ids".to_string(), "order_line".to_string())]
        );
        assert_eq!(
            call_effect("write", &[json!([1])], &vals, &[], 1).unwrap(),
            crate::policy::Effect::Delete
        );
    }

    #[test]
    fn decimal_command_codes_never_hide_deletions() {
        use serde_json::json;
        // Python equates 2.0 with 2.
        assert_eq!(
            relational_delete_count(&json!({"tags":[[2.0,8],[3.0,9],[6.0,0,[1]]]})),
            3
        );
        assert!(!has_relational_delete(
            &json!({"tags":[[4.0,8],[1.0,8,{"name":"x"}]]})
        ));
        assert!(has_relational_delete(
            &json!({"lines":[[1.0,8,{"tags":[[5.0,0,0]]}]]})
        ));
        // Treat fractional codes as potential deletes.
        assert!(has_relational_delete(&json!({"tags":[[2.5,8]]})));
    }

    #[test]
    fn web_save_copy_and_unknown_methods_detect_relational_deletions() {
        use crate::policy::Effect;
        use serde_json::json;
        let effect = |method, args: Value, kwargs: Value| {
            call_effect(
                method,
                args.as_array().unwrap(),
                kwargs.as_object().unwrap(),
                &[],
                0,
            )
        };
        let spec = json!({"name":{}});
        assert_eq!(
            effect("web_save", json!([[1],{"name":"x"},spec]), json!({})).unwrap(),
            Effect::Write
        );
        assert_eq!(
            effect(
                "web_save",
                json!([[1],{"tags":[[3.0,8]]}]),
                json!({"specification":spec})
            )
            .unwrap(),
            Effect::Delete
        );
        assert!(effect("web_save", json!([[1,2],{"tags":[[3,8]]}]), json!({})).is_err());
        assert!(effect("web_save", json!([[1]]), json!({"vals":{}})).is_err());
        assert_eq!(
            effect("web_save", json!([[],{"name":"x"}]), json!({})).unwrap(),
            Effect::Write
        );
        assert!(effect("web_save", json!([[],{"tags":[[2,8]]}]), json!({})).is_err());
        assert_eq!(
            effect("copy", json!([[1]]), json!({"default":{"name":"x"}})).unwrap(),
            Effect::Write
        );
        assert!(effect("copy", json!([[1],{"tags":[[6,0,[]]]}]), json!({})).is_err());
        assert!(effect("copy", json!([[1]]), json!({"default":{"tags":[[5,0,0]]}})).is_err());
        assert_eq!(
            effect("action_custom", json!([[1],{"tags":[[2,8]]}]), json!({})).unwrap(),
            Effect::Delete
        );
        assert_eq!(
            effect(
                "action_custom",
                json!([[1]]),
                json!({"vals":{"tags":[[3.0,8]]}})
            )
            .unwrap(),
            Effect::Delete
        );
        assert!(
            effect(
                "action_custom",
                json!([[1],{"tags":[[2,8],[3,9]]}]),
                json!({})
            )
            .is_err()
        );
    }
}
