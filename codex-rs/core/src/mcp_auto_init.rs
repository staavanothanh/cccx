/// MCP servers that cccx guarantees are always configured at startup.
///
/// architecture.md §10: Global Default registry — 2 MCPs always available
/// for every task: codebase-memory-mcp and sequential-thinking.
pub const CCCX_GLOBAL_MCP_SERVERS: &[&str] = &["codebase-memory-mcp", "sequential-thinking"];

/// Verify that the required cccx global MCP servers are present in config.
///
/// Returns the names of any required servers that are *not* present.
pub fn check_cccx_mcp_defaults(
    mcp_servers: &std::collections::HashMap<String, codex_config::types::McpServerConfig>,
) -> Vec<&'static str> {
    CCCX_GLOBAL_MCP_SERVERS
        .iter()
        .filter(|name| !mcp_servers.contains_key(**name))
        .copied()
        .collect()
}

/// Phase 7: auto-merge missing cccx global MCP servers into config at startup.
///
/// For each default that is not already configured, inserts a sensible
/// Stdio-based MCP server entry so `/mcp` is never empty when cccx gate
/// is active. If the user later adds explicit `[mcp_servers.<name>]`
/// entries in config.toml, those take precedence because they already exist.
pub fn merge_cccx_mcp_defaults(
    mcp_servers: &mut std::collections::HashMap<String, codex_config::types::McpServerConfig>,
) {
    use codex_config::types::McpServerTransportConfig;

    for name in CCCX_GLOBAL_MCP_SERVERS {
        mcp_servers.entry((*name).to_string()).or_insert_with(|| {
            codex_config::types::McpServerConfig {
                transport: McpServerTransportConfig::Stdio {
                    command: (*name).to_string(),
                    args: Vec::new(),
                    env: None,
                    env_vars: Vec::new(),
                    cwd: None,
                },
                auth: Default::default(),
                environment_id: codex_config::DEFAULT_MCP_SERVER_ENVIRONMENT_ID.to_string(),
                enabled: true,
                required: false,
                supports_parallel_tool_calls: false,
                disabled_reason: None,
                startup_timeout_sec: None,
                tool_timeout_sec: None,
                default_tools_approval_mode: None,
                enabled_tools: None,
                disabled_tools: None,
                scopes: None,
                oauth: None,
                oauth_resource: None,
                tools: std::collections::HashMap::new(),
            }
        });
    }
}

/// Log a warning for each missing cccx global MCP server.
/// Used when the cccx gate is off to alert users that global MCP defaults are missing.
pub fn warn_missing_cccx_mcps(missing: &[&str]) {
    for name in missing {
        tracing::warn!(
            "cccx global MCP server `{name}` is not configured. \
             Add `[mcp_servers.{name}]` to config.toml to enable it."
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn cccx_global_mcp_server_contract_is_exactly_two_defaults() {
        assert_eq!(
            CCCX_GLOBAL_MCP_SERVERS,
            &["codebase-memory-mcp", "sequential-thinking"]
        );
    }

    #[test]
    fn missing_global_mcp_servers_are_reported() {
        let configured = HashMap::new();
        assert_eq!(
            check_cccx_mcp_defaults(&configured),
            vec!["codebase-memory-mcp", "sequential-thinking"]
        );
    }

    #[test]
    fn partially_configured_global_mcp_reports_only_the_missing_server() {
        use codex_config::DEFAULT_MCP_SERVER_ENVIRONMENT_ID;
        use codex_config::types::McpServerAuth;
        use codex_config::types::McpServerTransportConfig;

        let mut configured = HashMap::new();
        configured.insert(
            "codebase-memory-mcp".to_string(),
            codex_config::types::McpServerConfig {
                transport: McpServerTransportConfig::Stdio {
                    command: "codebase-memory-mcp".to_string(),
                    args: Vec::new(),
                    env: None,
                    env_vars: Vec::new(),
                    cwd: None,
                },
                auth: McpServerAuth::default(),
                environment_id: DEFAULT_MCP_SERVER_ENVIRONMENT_ID.to_string(),
                enabled: true,
                required: false,
                supports_parallel_tool_calls: false,
                disabled_reason: None,
                startup_timeout_sec: None,
                tool_timeout_sec: None,
                default_tools_approval_mode: None,
                enabled_tools: None,
                disabled_tools: None,
                scopes: None,
                oauth: None,
                oauth_resource: None,
                tools: HashMap::new(),
            },
        );

        assert_eq!(
            check_cccx_mcp_defaults(&configured),
            vec!["sequential-thinking"]
        );
    }

    #[test]
    fn empty_config_gets_both_defaults_merged() {
        use codex_config::types::McpServerTransportConfig;

        let mut configured = HashMap::new();
        merge_cccx_mcp_defaults(&mut configured);

        assert_eq!(configured.len(), 2);
        assert!(configured.contains_key("codebase-memory-mcp"));
        assert!(configured.contains_key("sequential-thinking"));

        let cm = configured.get("codebase-memory-mcp").unwrap();
        assert!(cm.enabled);
        assert!(matches!(
            cm.transport,
            McpServerTransportConfig::Stdio { .. }
        ));
    }

    #[test]
    fn partial_config_merges_only_missing_default() {
        use codex_config::types::McpServerTransportConfig;

        let mut configured = HashMap::new();
        configured.insert(
            "codebase-memory-mcp".to_string(),
            codex_config::types::McpServerConfig {
                transport: McpServerTransportConfig::Stdio {
                    command: "my-custom-cm".to_string(),
                    args: Vec::new(),
                    env: None,
                    env_vars: Vec::new(),
                    cwd: None,
                },
                auth: Default::default(),
                environment_id: codex_config::DEFAULT_MCP_SERVER_ENVIRONMENT_ID.to_string(),
                enabled: true,
                required: false,
                supports_parallel_tool_calls: false,
                disabled_reason: None,
                startup_timeout_sec: None,
                tool_timeout_sec: None,
                default_tools_approval_mode: None,
                enabled_tools: None,
                disabled_tools: None,
                scopes: None,
                oauth: None,
                oauth_resource: None,
                tools: HashMap::new(),
            },
        );

        merge_cccx_mcp_defaults(&mut configured);

        // Only sequential-thinking should have been added.
        assert_eq!(configured.len(), 2);
        // Existing user config must not be overwritten.
        assert_eq!(
            configured
                .get("codebase-memory-mcp")
                .and_then(|c| match &c.transport {
                    McpServerTransportConfig::Stdio { command, .. } => {
                        Some(command.as_str())
                    }
                    _ => None,
                }),
            Some("my-custom-cm")
        );
    }
}
