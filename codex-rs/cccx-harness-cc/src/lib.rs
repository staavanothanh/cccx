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

pub fn main_system_prompt_fragment() -> &'static str {
    "\
You are the root Main CC agent for cccx.
- You are the only scheduler: plan, split work, dispatch bounded subagents, and decide final task outcome.
- Use MCP-first discovery for code work, especially codebase-memory-mcp and sequential-thinking when available.
- Spawn workers only with structured WorkSpec inputs; do not pass raw parent context as the worker contract.
- Accept worker results only as structured Artifacts with concrete evidence.
- Keep one harness per agent lifecycle: Main CC uses CC harness; CC/CX workers remain bounded and cannot schedule other agents.\
"
}

pub fn system_prompt_fragment() -> &'static str {
    "\
You are a CC-harness (control-plane) subagent.
- Plan before acting. Prefer MCP-first code discovery (codebase-memory-mcp, sequential-thinking).
- Use sequential-thinking for any non-trivial analysis or multi-step decision.
- You are a bounded analysis worker — the root Main CC agent holds final decision authority and scheduling power.
- Produce structured WorkSpec drafts for CX subagents when analysis is complete; the Main CC scheduler will dispatch them.
- Return Artifacts with clear evidence and structured findings; do not dump raw context.\
"
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

        assert!(prompt.contains("WorkSpec"));
        assert!(prompt.contains("do not dump raw context"));
    }

    #[test]
    fn main_prompt_claims_scheduler_authority_without_bounding_itself_as_worker() {
        let prompt = main_system_prompt_fragment();

        assert!(prompt.contains("root Main CC"));
        assert!(prompt.contains("only scheduler"));
        assert!(prompt.contains("structured WorkSpec"));
        assert!(prompt.contains("structured Artifacts"));
        assert!(
            !prompt.contains("bounded analysis worker"),
            "Main CC prompt must not reuse the bounded CC subagent identity"
        );
    }

    /// Phase 4: CC subagent must NOT claim scheduler authority.
    /// Architecture.md §3.2: "KHÔNG có quyền của Main CC: không commit ADR,
    /// không làm scheduler, không spawn subagent khác"
    #[test]
    fn p4_cc_subagent_prompt_does_not_claim_scheduler_authority() {
        let prompt = system_prompt_fragment();

        assert!(
            !prompt.contains("You are the control plane with final"),
            "Phase 4: CC subagent must not claim first-person scheduler authority."
        );
        assert!(
            !prompt.contains("Commit architectural decisions through ADR"),
            "Phase 4: CC subagent must not claim ADR commit authority."
        );
        assert!(
            prompt.contains("bounded"),
            "Phase 4: CC subagent prompt must identify itself as bounded"
        );
        // The prompt must reference Main CC as the authority
        assert!(
            prompt.contains("Main CC"),
            "Phase 4: CC subagent prompt must reference Main CC"
        );
    }
}
