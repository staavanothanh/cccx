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

/// Log a warning for each missing cccx global MCP server.
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
        use std::collections::HashMap;

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
}
