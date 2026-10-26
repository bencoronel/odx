mod flow;
mod support;
use odx_core::{ActivityFilter, Capability, Core, EnvironmentDraft, Protocol, WriteMode};
use serde_json::json;
use std::sync::Arc;

fn draft(name: &str) -> EnvironmentDraft {
    EnvironmentDraft {
        name: name.into(),
        url: "http://127.0.0.1:9".into(),
        database: "base".into(),
        login: "fixture-user".into(),
        protocol: Protocol::LegacyJsonrpc,
        context: json!({}),
        secret: "fixture-secret".into(),
    }
}

/// What an authenticated client may do: the capability in a granted environment.
fn allows(
    identity: &odx_core::ClientIdentity,
    environment_id: &str,
    capability: Capability,
) -> bool {
    identity.capabilities.contains(&capability)
        && identity
            .environment_ids
            .iter()
            .any(|id| id == environment_id)
}

#[tokio::test]
async fn client_tokens_are_shown_once_replaceable_revocable_and_bound_to_environments() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("vault.sqlite3");
    let core = Core::new(&path).unwrap();
    core.create("fictional master password").await.unwrap();
    let a = core.save_environment(draft("A")).await.unwrap();
    let b = core.save_environment(draft("B")).await.unwrap();

    // Grant validation.
    assert!(
        core.create_client("", vec![a.id.clone()], vec![Capability::OdooRead])
            .await
            .is_err()
    );
    assert!(
        core.create_client("agente", vec![], vec![Capability::OdooRead])
            .await
            .is_err()
    );
    assert!(
        core.create_client(
            "agente",
            vec!["inexistente".into()],
            vec![Capability::OdooRead]
        )
        .await
        .is_err()
    );
    assert!(
        core.create_client("agente", vec![a.id.clone()], vec![])
            .await
            .is_err()
    );

    let issued = core
        .create_client(
            "agente",
            vec![a.id.clone(), a.id.clone()],
            vec![Capability::OdooRead],
        )
        .await
        .unwrap();
    let token = issued.token.clone();
    assert_eq!(issued.client.environment_ids, vec![a.id.clone()]);
    assert!(
        format!("{issued:?}").contains("[redacted]") && !format!("{issued:?}").contains(&token)
    );
    assert!(
        core.create_client("Agente", vec![a.id.clone()], vec![Capability::OdooRead])
            .await
            .is_err(),
        "duplicate name"
    );

    // Admin views and encrypted files exclude plaintext tokens.
    let listed = serde_json::to_string(&core.clients().await.unwrap()).unwrap();
    assert!(!listed.contains(&token) && !listed.contains("token_hash"));
    let raw = std::fs::read(&path).unwrap();
    assert!(!raw.windows(token.len()).any(|w| w == token.as_bytes()));

    let identity = core.authenticate_client(&token).await.unwrap();
    assert!(allows(&identity, &a.id, Capability::OdooRead));
    assert!(
        !allows(&identity, &b.id, Capability::OdooRead),
        "deny ungranted environments"
    );
    assert!(
        !allows(&identity, &a.id, Capability::OdooWriteRequest),
        "deny ungranted capabilities"
    );
    assert!(core.authenticate_client("odx_no-valido").await.is_err());
    assert!(core.authenticate_client("").await.is_err());

    // Encrypted persistence and locking.
    core.lock().await;
    assert!(
        core.authenticate_client(&token).await.is_err(),
        "locked vault rejects authentication"
    );
    assert!(core.clients().await.is_err());
    core.unlock("fictional master password").await.unwrap();
    assert_eq!(
        core.authenticate_client(&token).await.unwrap().client_id,
        issued.client.id
    );

    // Replacing the token invalidates the old one.
    let replaced = core.replace_client_token(&issued.client.id).await.unwrap();
    assert_ne!(replaced.token, token);
    assert!(core.authenticate_client(&token).await.is_err());
    let identity = core.authenticate_client(&replaced.token).await.unwrap();
    assert!(identity.revision > 1);

    // Deleting an environment removes grants; recreation does not inherit them.
    core.delete_environment(&a.id, true).await.unwrap();
    let a2 = core.save_environment(draft("A")).await.unwrap();
    let identity = core.authenticate_client(&replaced.token).await.unwrap();
    assert!(identity.environment_ids.is_empty());
    assert!(!allows(&identity, &a2.id, Capability::OdooRead));

    // Revocation is final and cannot be replaced.
    core.revoke_client(&issued.client.id).await.unwrap();
    assert!(core.authenticate_client(&replaced.token).await.is_err());
    assert!(core.replace_client_token(&issued.client.id).await.is_err());
    assert!(
        core.clients()
            .await
            .unwrap()
            .iter()
            .all(|client| client.local_stdio),
        "revocation deletes the client"
    );
    assert!(core.revoke_client("inexistente").await.is_err());
}

#[tokio::test]
async fn local_stdio_profile_is_permanent_and_preserves_edited_permissions_on_unlock() {
    let tmp = tempfile::tempdir().unwrap();
    let core = support::unlocked_core(&tmp).await;
    let clients = core.clients().await.unwrap();
    assert_eq!(clients.len(), 1);
    let local = &clients[0];
    assert_eq!(local.label, "MCP local");
    assert!(local.local_stdio && local.all_environments);
    assert!(local.capabilities.contains(&Capability::OdooRead));
    assert!(local.capabilities.contains(&Capability::OdooWriteRequest));
    assert!(core.revoke_client(&local.id).await.is_err());
    assert!(core.replace_client_token(&local.id).await.is_err());
    let env = core.save_environment(draft("A")).await.unwrap();
    assert!(
        core.update_client(
            &local.id,
            "My local profile",
            vec![env.id.clone()],
            vec![Capability::OdooRead]
        )
        .await
        .is_err()
    );
    core.update_client(
        &local.id,
        "MCP local",
        vec![env.id.clone()],
        vec![Capability::OdooRead],
    )
    .await
    .unwrap();
    for _ in 0..2 {
        core.lock().await;
        core.unlock("fictional master password").await.unwrap();
        let clients = core.clients().await.unwrap();
        assert_eq!(clients.len(), 1);
        assert!(clients[0].local_stdio);
        assert_eq!(clients[0].id, local.id);
        assert_eq!(clients[0].label, "MCP local");
        assert!(!clients[0].all_environments);
        assert_eq!(clients[0].environment_ids, vec![env.id.clone()]);
        assert_eq!(clients[0].capabilities, vec![Capability::OdooRead]);
    }
}

#[tokio::test]
async fn all_environments_scope_includes_environments_created_later() {
    use odx_core::ClientScope;
    let tmp = tempfile::tempdir().unwrap();
    let core = support::unlocked_core(&tmp).await;
    let a = core.save_environment(draft("A")).await.unwrap();
    let all = core
        .create_client("all", ClientScope::All, vec![Capability::OdooRead])
        .await
        .unwrap();
    assert!(all.client.all_environments && all.client.environment_ids.is_empty());
    let only = core
        .create_client("only-a", vec![a.id.clone()], vec![Capability::OdooRead])
        .await
        .unwrap();
    let all_id = core.authenticate_client(&all.token).await.unwrap();
    let only_id = core.authenticate_client(&only.token).await.unwrap();

    let names = |out: odx_core::ToolOutput| {
        out.data
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["name"].as_str().unwrap().to_string())
            .collect::<Vec<_>>()
    };
    core.save_environment(draft("B")).await.unwrap();
    assert_eq!(
        names(core.run_tool(&all_id, "envs", json!({})).await.unwrap()),
        ["A", "B"]
    );
    assert_eq!(
        names(core.run_tool(&only_id, "envs", json!({})).await.unwrap()),
        ["A"]
    );

    core.delete_environment(&a.id, true).await.unwrap();
    assert_eq!(
        names(core.run_tool(&all_id, "envs", json!({})).await.unwrap()),
        ["B"]
    );
    assert!(
        core.run_tool(&only_id, "envs", json!({}))
            .await
            .unwrap()
            .data
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn editing_a_client_changes_scope_and_capabilities_without_new_token() {
    use odx_core::ClientScope;
    let tmp = tempfile::tempdir().unwrap();
    let core = support::unlocked_core(&tmp).await;
    let a = core.save_environment(draft("A")).await.unwrap();
    core.save_environment(draft("B")).await.unwrap();
    let issued = core
        .create_client("agente", ClientScope::All, vec![Capability::OdooRead])
        .await
        .unwrap();
    let identity = core.authenticate_client(&issued.token).await.unwrap();
    let names = |out: odx_core::ToolOutput| out.data.as_array().unwrap().len();
    assert_eq!(
        names(core.run_tool(&identity, "envs", json!({})).await.unwrap()),
        2
    );

    let view = core
        .update_client(
            &issued.client.id,
            "agente-a",
            vec![a.id.clone()],
            vec![Capability::OdooRead, Capability::OdooWriteRequest],
        )
        .await
        .unwrap();
    assert!(!view.all_environments && view.label == "agente-a");
    // Apply updated grants on the next call with the same token/connection.
    assert_eq!(
        names(core.run_tool(&identity, "envs", json!({})).await.unwrap()),
        1
    );
    assert!(
        core.authenticate_client(&issued.token)
            .await
            .unwrap()
            .capabilities
            .contains(&Capability::OdooWriteRequest)
    );

    assert!(
        core.update_client(
            &issued.client.id,
            "agente-a",
            vec![],
            vec![Capability::OdooRead]
        )
        .await
        .is_err()
    );
    assert!(
        core.update_client(&issued.client.id, "x", ClientScope::All, vec![])
            .await
            .is_err()
    );
    assert!(
        core.update_client(
            "inexistente",
            "x",
            ClientScope::All,
            vec![Capability::OdooRead]
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn replacing_a_token_revokes_only_that_clients_pending_requests() {
    let tmp = tempfile::tempdir().unwrap();
    let sim = support::simulator().await;
    let core = Arc::new(Core::new(tmp.path().join("vault.sqlite3")).unwrap());
    core.create("fictional master password").await.unwrap();
    let mut env = draft("A");
    env.url = sim.url.clone();
    env.database = "base-a".into();
    let a = core.save_environment(env).await.unwrap();
    core.set_mode(&a.id, WriteMode::Approval).await.unwrap();
    let caps = vec![Capability::OdooRead, Capability::OdooWriteRequest];
    let issued = core
        .create_client("rotado", vec![a.id.clone()], caps.clone())
        .await
        .unwrap();
    let old = core.authenticate_client(&issued.token).await.unwrap();
    let other = flow::client(&core).await;
    let write = json!({"env":"A","model":"res.partner","ids":[1],"values":{"name":"Ada"}});

    let (call, pending) = flow::queue(&core, &old, "write", write.clone()).await;
    let (other_call, other_pending) = flow::queue(&core, &other, "write", write.clone()).await;

    let replaced = core.replace_client_token(&issued.client.id).await.unwrap();
    assert_eq!(flow::error_code(call).await, "approval_revoked");
    // Revoked requests leave the inbox and cannot be approved.
    assert!(core.decide(&pending.id, true).await.is_err());
    let ids: Vec<String> = core
        .approvals()
        .await
        .unwrap()
        .into_iter()
        .map(|p| p.id)
        .collect();
    assert_eq!(
        ids,
        [other_pending.id.as_str()],
        "the other client keeps its grant"
    );
    let revoked = core
        .activity(None, ActivityFilter::default())
        .await
        .unwrap()
        .events
        .iter()
        .any(|e| e.request_id.as_deref() == Some(&pending.id) && e.outcome == "revoked");
    assert!(revoked, "record revocation in activity");
    assert_eq!(sim.counts.writes(), 0);

    // Reject both the old token and its previously authenticated identity.
    assert!(core.authenticate_client(&issued.token).await.is_err());
    assert_eq!(
        core.run_tool(&old, "envs", json!({}))
            .await
            .unwrap_err()
            .code,
        "unauthenticated"
    );
    assert_eq!(
        core.run_tool(&old, "write", write.clone())
            .await
            .unwrap_err()
            .code,
        "unauthenticated"
    );
    assert_eq!(core.approvals().await.unwrap().len(), 1);

    // The replacement token keeps its grant and can request/execute approved writes.
    let new = core.authenticate_client(&replaced.token).await.unwrap();
    assert_eq!(new.client_id, old.client_id);
    let (call, pending) = flow::queue(&core, &new, "write", write).await;
    core.decide(&pending.id, true).await.unwrap();
    assert_eq!(call.await.unwrap().unwrap().data, json!(true));
    core.decide(&other_pending.id, true).await.unwrap();
    assert_eq!(other_call.await.unwrap().unwrap().data, json!(true));
    assert_eq!(sim.counts.writes(), 2);
}
