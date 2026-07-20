use cccx_protocol::ActorIdentity;
use cccx_protocol::HarnessKind;
use codex_protocol::mcp::CallToolResult;
use serde::Serialize;
use serde_json::Value as JsonValue;

const ADR_PROPOSAL_SCHEMA: &str = "cccx.adr_proposal.v1";
const ADR_GATE_META_KEY: &str = "cccx/pre_decision_write_gate";

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct AdrProposal {
    schema: &'static str,
    source_actor: ActorIdentity,
    source_harness: HarnessKind,
    requested_server: String,
    requested_tool: String,
    proposed_arguments: JsonValue,
    reason: &'static str,
    next_action: &'static str,
}

pub(crate) fn maybe_intercept_adr_commit(
    actor: ActorIdentity,
    harness: HarnessKind,
    server: &str,
    tool: &str,
    arguments: Option<&JsonValue>,
) -> Option<CallToolResult> {
    if !is_adr_commit_tool(server, tool) {
        return None;
    }
    if matches!(actor, ActorIdentity::MainCc) && matches!(harness, HarnessKind::Cc) {
        return None;
    }

    let proposal = AdrProposal {
        schema: ADR_PROPOSAL_SCHEMA,
        source_actor: actor,
        source_harness: harness,
        requested_server: server.to_string(),
        requested_tool: tool.to_string(),
        proposed_arguments: arguments.cloned().unwrap_or(JsonValue::Null),
        reason: "only Main CC may commit ADR decisions",
        next_action: "return this ADRProposal artifact to Main CC for review and commit",
    };
    let structured = serde_json::to_value(&proposal)
        .expect("ADR proposal contains only JSON-serializable fields");
    Some(CallToolResult {
        content: vec![serde_json::json!({
            "type": "text",
            "text": format!(
                "cccx PreDecisionWrite gate intercepted ADR commit; returning {ADR_PROPOSAL_SCHEMA} instead."
            ),
        })],
        structured_content: Some(structured),
        is_error: Some(false),
        meta: Some(serde_json::json!({
            ADR_GATE_META_KEY: true,
        })),
    })
}

fn is_adr_commit_tool(server: &str, tool: &str) -> bool {
    let normalized_server = server.to_ascii_lowercase();
    let normalized_tool = tool.to_ascii_lowercase();
    normalized_tool == "manage_adr"
        && matches!(
            normalized_server.as_str(),
            "codebase-memory-mcp" | "codebase_memory_mcp" | "memory"
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn main_cc_adr_commit_passes_through_to_mcp_policy() {
        let result = maybe_intercept_adr_commit(
            ActorIdentity::MainCc,
            HarnessKind::Cc,
            "codebase-memory-mcp",
            "manage_adr",
            Some(&serde_json::json!({"mode": "update"})),
        );

        assert!(
            result.is_none(),
            "Main CC owns ADR commit authority; this gate must not synthesize a proposal"
        );
    }

    #[test]
    fn cc_subagent_adr_commit_becomes_structured_proposal() {
        let result = maybe_intercept_adr_commit(
            ActorIdentity::CcSubagent,
            HarnessKind::Cc,
            "codebase-memory-mcp",
            "manage_adr",
            Some(&serde_json::json!({"title": "Use 9router only"})),
        )
        .expect("child ADR commit must be intercepted");

        assert_eq!(result.is_error, Some(false));
        let structured = result
            .structured_content
            .expect("ADR proposal must be structured");
        assert_eq!(structured["schema"], ADR_PROPOSAL_SCHEMA);
        assert_eq!(structured["source_actor"], "cc_subagent");
        assert_eq!(structured["proposed_arguments"]["title"], "Use 9router only");
    }

    #[test]
    fn cx_subagent_adr_commit_becomes_structured_proposal() {
        let result = maybe_intercept_adr_commit(
            ActorIdentity::CxSubagent,
            HarnessKind::Cx,
            "memory",
            "manage_adr",
            None,
        )
        .expect("CX child ADR commit must be intercepted");

        let structured = result
            .structured_content
            .expect("ADR proposal must be structured");
        assert_eq!(structured["source_actor"], "cx_subagent");
        assert_eq!(
            structured["next_action"],
            "return this ADRProposal artifact to Main CC for review and commit"
        );
    }

    #[test]
    fn unrelated_mcp_tool_is_not_intercepted() {
        let result = maybe_intercept_adr_commit(
            ActorIdentity::CxSubagent,
            HarnessKind::Cx,
            "codebase-memory-mcp",
            "search_graph",
            None,
        );

        assert!(result.is_none());
    }
}
