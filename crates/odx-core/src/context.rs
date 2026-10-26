use crate::{
    Core,
    config::validate_json,
    error::{OdxError, Result},
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashSet;

#[derive(Clone, Serialize, Deserialize)]
pub struct ContextEntry {
    pub name: String,
    pub value: Value,
    pub enabled: bool,
}
#[derive(Serialize)]
pub struct ContextSettings {
    pub entries: Vec<ContextEntry>,
    pub inherited: Value,
}
fn entries(active: &Value, disabled: &Map<String, Value>) -> Vec<ContextEntry> {
    active
        .as_object()
        .into_iter()
        .flat_map(|m| m.iter())
        .map(|(name, value)| ContextEntry {
            name: name.clone(),
            value: value.clone(),
            enabled: true,
        })
        .chain(disabled.iter().map(|(name, value)| ContextEntry {
            name: name.clone(),
            value: value.clone(),
            enabled: false,
        }))
        .collect()
}
fn dictionaries(entries: Vec<ContextEntry>) -> Result<(Value, Map<String, Value>)> {
    if entries.len() > 64 {
        return Err(OdxError::validation(
            "validation.context.too_many_entries",
            "maximum 64 context entries",
        ));
    }
    let mut names = HashSet::new();
    let (mut active, mut disabled) = (Map::new(), Map::new());
    for entry in entries {
        if entry.name.trim() != entry.name {
            return Err(OdxError::validation(
                "validation.context.trimmed_key",
                "context keys cannot have leading or trailing spaces",
            ));
        }
        if entry.name.is_empty() || !names.insert(entry.name.clone()) {
            return Err(OdxError::validation(
                "validation.context.empty_or_duplicate_key",
                "context keys must be nonempty and unique",
            ));
        }
        if entry.enabled {
            active.insert(entry.name, entry.value);
        } else {
            disabled.insert(entry.name, entry.value);
        }
    }
    validate_json(&serde_json::to_vec(&serde_json::json!([active, disabled]))?)?;
    Ok((Value::Object(active), disabled))
}
impl Core {
    pub async fn context_settings(&self, environment_id: Option<&str>) -> Result<ContextSettings> {
        let s = self.inner.lock().await;
        let data = s.vault()?.data();
        if let Some(id) = environment_id {
            let env = data.environment(id)?;
            Ok(ContextSettings {
                entries: entries(&env.context, &env.disabled_context),
                inherited: data.global_context.clone(),
            })
        } else {
            Ok(ContextSettings {
                entries: entries(&data.global_context, &data.global_disabled_context),
                inherited: serde_json::json!({}),
            })
        }
    }
    pub async fn set_context(
        &self,
        environment_id: Option<&str>,
        entries: Vec<ContextEntry>,
    ) -> Result<()> {
        let (active, disabled) = dictionaries(entries)?;
        let mut s = self.inner.lock().await;
        let ids = s.vault_mut()?.update(|data| {
            Ok(if let Some(id) = environment_id {
                let env = data.environment_mut(id)?;
                env.context = active;
                env.disabled_context = disabled;
                env.revision += 1;
                vec![id.to_owned()]
            } else {
                data.global_context = active;
                data.global_disabled_context = disabled;
                data.environments
                    .iter_mut()
                    .map(|e| {
                        e.revision += 1;
                        e.id.clone()
                    })
                    .collect()
            })
        })?;
        self.environment_changed(s, &ids).await;
        Ok(())
    }
}
