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
    main_verification_complete: bool,
    server: &str,
    tool: &str,
    arguments: Option<&JsonValue>,
) -> Option<CallToolResult> {
    if !is_adr_write_request(tool, arguments) {
        return None;
    }
    let trusted_backend = is_trusted_adr_backend(server);
    if matches!(actor, ActorIdentity::MainCc)
        && matches!(harness, HarnessKind::Cc)
        && main_verification_complete
        && trusted_backend
    {
        return None;
    }

    if matches!(actor, ActorIdentity::MainCc) {
        let (message, reason, next_action) = if trusted_backend {
            (
                "cccx PreDecisionWrite denied ADR commit: Task Graph verification is incomplete.",
                "task_graph_verification_incomplete",
                "complete independent verification before committing the ADR",
            )
        } else {
            (
                "cccx PreDecisionWrite denied ADR commit through an untrusted MCP alias.",
                "untrusted_adr_backend",
                "use the configured codebase-memory-mcp ADR backend",
            )
        };
        return Some(CallToolResult {
            content: vec![serde_json::json!({
                "type": "text",
                "text": message,
            })],
            structured_content: Some(serde_json::json!({
                "schema": "cccx.adr_commit_denied.v1",
                "reason": reason,
                "next_action": next_action,
            })),
            is_error: Some(true),
            meta: Some(serde_json::json!({ ADR_GATE_META_KEY: true })),
        });
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

pub(crate) fn maybe_intercept_namespaced_adr_commit(
    actor: ActorIdentity,
    harness: HarnessKind,
    main_verification_complete: bool,
    namespace: Option<&str>,
    tool: &str,
    arguments: Option<&JsonValue>,
) -> Option<CallToolResult> {
    let server = namespace.and_then(mcp_server_from_tool_namespace)?;
    maybe_intercept_adr_commit(
        actor,
        harness,
        main_verification_complete,
        &server,
        tool,
        arguments,
    )
}

fn mcp_server_from_tool_namespace(namespace: &str) -> Option<String> {
    namespace.strip_prefix("mcp__").map(ToString::to_string)
}

pub(crate) async fn task_graph_verification_complete(
    turn_context: &crate::session::turn_context::TurnContext,
) -> bool {
    #[allow(deprecated)]
    let project_root = turn_context.cwd.as_path();
    crate::cccx_task_graph_state::load_task_graph(project_root)
        .await
        .ok()
        .flatten()
        .is_some_and(|graph| {
            graph.validate().is_ok()
                && graph.can_stop()
                && crate::cccx_task_graph_state::matches_verified_graph_seal(project_root, &graph)
        })
}

pub(crate) fn is_adr_write_request(tool: &str, arguments: Option<&JsonValue>) -> bool {
    if !tool.eq_ignore_ascii_case("manage_adr") {
        return false;
    }
    let operation = arguments
        .and_then(JsonValue::as_object)
        .and_then(|object| {
            ["mode", "action", "operation"]
                .into_iter()
                .find_map(|key| object.get(key).and_then(JsonValue::as_str))
        })
        .map(str::trim)
        .map(str::to_ascii_lowercase);
    !matches!(
        operation.as_deref(),
        Some("get" | "read" | "list" | "search" | "show" | "query")
    )
}

pub(crate) fn is_trusted_adr_backend(server: &str) -> bool {
    let normalized_server = server.to_ascii_lowercase();
    matches!(
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
            true,
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
    fn main_cc_adr_commit_is_denied_until_task_graph_verification_completes() {
        let result = maybe_intercept_adr_commit(
            ActorIdentity::MainCc,
            HarnessKind::Cc,
            false,
            "codebase-memory-mcp",
            "manage_adr",
            Some(&serde_json::json!({"mode": "create"})),
        )
        .expect("unverified Main CC ADR commit must be intercepted");

        assert_eq!(result.is_error, Some(true));
        assert_eq!(
            result.structured_content.expect("structured denial")["reason"],
            "task_graph_verification_incomplete"
        );
    }

    #[test]
    fn cc_subagent_adr_commit_becomes_structured_proposal() {
        let result = maybe_intercept_adr_commit(
            ActorIdentity::CcSubagent,
            HarnessKind::Cc,
            false,
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
        assert_eq!(
            structured["proposed_arguments"]["title"],
            "Use 9router only"
        );
    }

    #[test]
    fn cx_subagent_adr_commit_becomes_structured_proposal() {
        let result = maybe_intercept_adr_commit(
            ActorIdentity::CxSubagent,
            HarnessKind::Cx,
            false,
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
            false,
            "codebase-memory-mcp",
            "search_graph",
            None,
        );

        assert!(result.is_none());
    }

    #[test]
    fn read_only_adr_operations_are_not_treated_as_commits() {
        let result = maybe_intercept_adr_commit(
            ActorIdentity::CxSubagent,
            HarnessKind::Cx,
            false,
            "codebase-memory-mcp",
            "manage_adr",
            Some(&serde_json::json!({"mode": "list"})),
        );

        assert!(result.is_none());
    }

    #[test]
    fn child_adr_write_through_arbitrary_alias_is_still_intercepted() {
        let result = maybe_intercept_adr_commit(
            ActorIdentity::CxSubagent,
            HarnessKind::Cx,
            false,
            "arbitrary-alias",
            "manage_adr",
            Some(&serde_json::json!({"mode": "create"})),
        )
        .expect("alias must not bypass child ADR write gate");

        assert_eq!(result.is_error, Some(false));
        assert_eq!(
            result.structured_content.expect("proposal")["schema"],
            ADR_PROPOSAL_SCHEMA
        );
    }

    #[test]
    fn main_adr_write_through_arbitrary_alias_is_denied() {
        let result = maybe_intercept_adr_commit(
            ActorIdentity::MainCc,
            HarnessKind::Cc,
            true,
            "arbitrary-alias",
            "manage_adr",
            Some(&serde_json::json!({"mode": "create"})),
        )
        .expect("Main must use trusted ADR backend identity");

        assert_eq!(result.is_error, Some(true));
        assert_eq!(
            result.structured_content.expect("denial")["reason"],
            "untrusted_adr_backend"
        );
    }

    #[test]
    fn cx_subagent_namespaced_mcp_adr_commit_becomes_structured_proposal() {
        let result = maybe_intercept_namespaced_adr_commit(
            ActorIdentity::CxSubagent,
            HarnessKind::Cx,
            false,
            Some("mcp__codebase_memory_mcp"),
            "manage_adr",
            Some(&serde_json::json!({"title": "Use 9router only"})),
        )
        .expect("namespaced MCP ADR commit must be intercepted");

        let structured = result
            .structured_content
            .expect("ADR proposal must be structured");
        assert_eq!(structured["schema"], ADR_PROPOSAL_SCHEMA);
        assert_eq!(structured["source_actor"], "cx_subagent");
        assert_eq!(structured["requested_server"], "codebase_memory_mcp");
        assert_eq!(
            structured["proposed_arguments"]["title"],
            "Use 9router only"
        );
    }
}
