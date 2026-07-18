pub const HARNESS: cccx_protocol::HarnessKind = cccx_protocol::HarnessKind::Cx;

pub fn allowed_tools() -> Vec<String> {
    vec![
        "mcp:codebase-memory-mcp.search_graph".into(),
        "mcp:codebase-memory-mcp.search_code".into(),
        "mcp:codebase-memory-mcp.trace_path".into(),
        "mcp:codebase-memory-mcp.get_architecture".into(),
        "mcp:codebase-memory-mcp.get_code_snippet".into(),
        "mcp:codebase-memory-mcp.detect_changes".into(),
    ]
}

pub fn forbidden_tools() -> Vec<String> {
    vec![
        "spawn_agent".into(),
        "mcp:codebase-memory-mcp.manage_adr".into(),
        "mcp:codebase-memory-mcp.index_repository".into(),
        "mcp:sequential-thinking.sequentialthinking".into(),
    ]
}

pub fn system_prompt_fragment() -> &'static str {
    "\
You are a CX-harness (execution-plane) agent.
- Execute your bounded WorkSpec objective. Do not expand scope.
- Verify output before declaring done: run tests, check diffs, review evidence.
- Return structured Artifact with status, evidence, and provenance.
- Do not schedule tasks, spawn other agents, or commit ADRs.
- If blocked by scope/constraints, report status:blocked with a clear reason."
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cx_harness_is_execution_plane_only() {
        let allowed = allowed_tools();
        let forbidden = forbidden_tools();

        assert_eq!(HARNESS, cccx_protocol::HarnessKind::Cx);
        assert!(allowed.contains(&"mcp:codebase-memory-mcp.search_graph".to_string()));
        assert!(!allowed.contains(&"mcp:codebase-memory-mcp.manage_adr".to_string()));
        assert!(forbidden.contains(&"spawn_agent".to_string()));
        assert!(forbidden.contains(&"mcp:codebase-memory-mcp.manage_adr".to_string()));
        assert!(forbidden.contains(&"mcp:sequential-thinking.sequentialthinking".to_string()));
    }

    #[test]
    fn cx_prompt_requires_artifact_and_no_scheduling() {
        let prompt = system_prompt_fragment();

        assert!(prompt.contains("bounded WorkSpec"));
        assert!(prompt.contains("Artifact"));
        assert!(prompt.contains("evidence"));
        assert!(prompt.contains("provenance"));
        assert!(prompt.contains("Do not schedule tasks"));
    }
}
