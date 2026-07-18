pub const HARNESS: cccx_protocol::HarnessKind = cccx_protocol::HarnessKind::Cc;

pub fn allowed_tools() -> Vec<String> {
    vec![
        // --- codebase-memory-mcp ---
        "mcp:codebase-memory-mcp.index_status".into(),
        "mcp:codebase-memory-mcp.index_repository".into(),
        "mcp:codebase-memory-mcp.search_graph".into(),
        "mcp:codebase-memory-mcp.search_code".into(),
        "mcp:codebase-memory-mcp.trace_path".into(),
        "mcp:codebase-memory-mcp.query_graph".into(),
        "mcp:codebase-memory-mcp.get_architecture".into(),
        "mcp:codebase-memory-mcp.get_code_snippet".into(),
        "mcp:codebase-memory-mcp.get_graph_schema".into(),
        "mcp:codebase-memory-mcp.detect_changes".into(),
        "mcp:codebase-memory-mcp.manage_adr".into(),
        // --- sequential-thinking ---
        "mcp:sequential-thinking.sequentialthinking".into(),
    ]
}

pub fn system_prompt_fragment() -> &'static str {
    "\
You are a CC-harness (control-plane) agent.
- Plan before acting. Prefer MCP-first code discovery (codebase-memory-mcp, sequential-thinking).
- Use sequential-thinking for any non-trivial analysis or multi-step decision.
- You are the control plane with final decision authority.
- Define structured WorkSpecs for CX subagents; do not dump raw context.
- Commit architectural decisions through ADR (manage_adr), not as conversational notes."
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cc_harness_exposes_control_plane_mcp_tools() {
        let tools = allowed_tools();

        assert_eq!(HARNESS, cccx_protocol::HarnessKind::Cc);
        assert!(tools.contains(&"mcp:codebase-memory-mcp.search_graph".to_string()));
        assert!(tools.contains(&"mcp:codebase-memory-mcp.manage_adr".to_string()));
        assert!(tools.contains(&"mcp:sequential-thinking.sequentialthinking".to_string()));
    }

    #[test]
    fn cc_prompt_requires_structured_workspec_not_raw_context_dump() {
        let prompt = system_prompt_fragment();

        assert!(prompt.contains("WorkSpecs"));
        assert!(prompt.contains("do not dump raw context"));
        assert!(prompt.contains("control plane"));
    }
}
