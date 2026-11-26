//! Test bridge for the desktop rule form. Uses the same vault and save path as the app.
use odx_core::{Core, RuleDraft};
use serde_json::{Value, json};
use std::io::Read;
use std::sync::Arc;

#[path = "../tests/flow/mod.rs"]
mod flow;
#[path = "../tests/support/mod.rs"]
mod support;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).ok_or("missing vault path")?;
    let core = Arc::new(Core::new(path)?);
    if core.status().await == "absent" {
        core.create("desktop rule contract test password").await?;
    } else {
        core.unlock("desktop rule contract test password").await?;
    }
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;
    let command: Value = serde_json::from_str(&input)?;
    let result = match command["op"].as_str() {
        Some("save") => {
            let draft: RuleDraft = serde_json::from_value(command["draft"].clone())?;
            serde_json::to_value(core.save_rule(draft).await?)?
        }
        Some("list") => serde_json::to_value(core.rules(None).await?)?,
        Some("verify") => {
            let sim = support::simulator().await;
            core.save_environment(support::draft("Rule contract simulator", &sim.url))
                .await?;
            let client = flow::client(&core).await;
            let result = core
                .run_tool(
                    &client,
                    "search_read",
                    json!({
                        "env":"Rule contract simulator", "model":"res.partner",
                        "fields":["id", "name", "api_token", "password"]
                    }),
                )
                .await
                .map_err(|error| format!("{error:?}"))?;
            let sent = sim
                .counts
                .calls()
                .into_iter()
                .find(|call| call[4] == "search_read")
                .ok_or("missing simulated call")?;
            json!({"data":result.data, "fields":sent[6]["fields"]})
        }
        _ => return Err("invalid operation".into()),
    };
    println!("{}", json!(result));
    Ok(())
}
