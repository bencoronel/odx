// Every Tauri command, once: main.rs registers them and build.rs grants them per window
// (capabilities/*.json). `for_each_command!(m)` calls `m!` with all their names.
macro_rules! for_each_command {
    ($m:ident) => {
        $m! {
            desktop_ready,
            desktop_set_texts,
            agent_setup,
            agent_install,
            desktop_take_approval,
            agent_take,
            table_window_open,
            window_keep_above,
            window_keep_above_supported,
            table_window_take,
            agent_reply,
            desktop_restore,
            desktop_suspend,
            desktop_cancel_suspend,
            desktop_flushed,
            window_present,
            context_get,
            context_set,
            headers_get,
            headers_set,
            proxies_get,
            proxy_save,
            proxy_delete,
            proxy_test,
            tag_colors_get,
            tag_color_set,
            proxy_default_set,
            environment_proxy_get,
            environment_proxy_set,
            vault_status,
            vault_create,
            vault_unlock,
            vault_change_password,
            vault_lock,
            vault_reset,
            service_status,
            mcp_disconnect,
            activity_list,
            activity_detail,
            shell_call,
            shell_history_list,
            shell_history_add,
            shell_history_clear,
            shell_export,
            shell_sessions_load,
            shell_sessions_save,
            environments_list,
            environment_save,
            environment_update,
            environment_clone,
            environment_delete,
            environment_write_mode,
            environment_check,
            environment_check_draft,
            environment_databases,
            environment_tags_set,
            environment_favicon_page,
            environment_favicon_image,
            environment_open_url,
            updates_open_releases,
            clients_list,
            client_update,
            approvals_list,
            approval_decide,
            rules_list,
            rule_save,
            rule_delete,
            desktop_status,
            desktop_restart,
            settings_get,
            settings_save
        }
    };
}

#[cfg(test)]
mod tests {
    macro_rules! names {
        ($($command:ident),* $(,)?) => {
            [$(stringify!($command)),*]
        };
    }
    /// App command names a capability allows (`allow-foo-bar` → `foo_bar`); plugin permissions skipped.
    fn allowed(capability: &str) -> Vec<String> {
        let json: serde_json::Value = serde_json::from_str(capability).unwrap();
        let mut names: Vec<String> = json["permissions"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|p| p.as_str()?.strip_prefix("allow-"))
            .map(|p| p.replace('-', "_"))
            .collect();
        names.sort();
        names
    }
    #[test]
    fn the_main_window_gets_every_command_and_table_windows_a_subset() {
        let mut all: Vec<String> = for_each_command!(names)
            .iter()
            .map(|c| c.to_string())
            .collect();
        all.sort();
        assert_eq!(allowed(include_str!("../capabilities/main.json")), all);
        for command in allowed(include_str!("../capabilities/table.json")) {
            assert!(all.contains(&command), "{command}");
        }
    }
}
