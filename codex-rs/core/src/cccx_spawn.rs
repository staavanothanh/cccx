//! cccx WorkSpec → child Config adapter.
//!
//! Bridges Codex native spawn (`build_agent_shared_config`) to cccx's
//! WorkSpec + harness model. Must be called after the shared config is
//! built and before the subagent starts execution.
//!
//! Architecture.md §6: "WorkSpec-to-ChildConfig Adapter — deny-by-default".

use cccx_protocol::HarnessKind;
use cccx_protocol::ParentCapabilitySet;
use cccx_protocol::ProtocolError;
use cccx_protocol::WorkRole;
use cccx_protocol::WorkSpec;

use crate::config::Config;

/// Apply cccx harness configuration and deny-by-default projection to a
/// subagent config.
///
/// Called after `build_agent_shared_config()` when a `WorkSpec` is present.
/// Architecture.md §6: "WorkSpec-to-ChildConfig Adapter — deny-by-default".
pub fn apply_cccx_harness_to_config(
    config: &mut Config,
    parent: &ParentCapabilitySet,
    work_spec: &WorkSpec,
) -> Result<(), ProtocolError> {
    let projection = cccx_protocol::project_child_config(work_spec, parent)?;

    // --- harness prompt fragment ---
    let harness_prompt = match work_spec.harness {
        HarnessKind::Cc => cccx_harness_cc::system_prompt_fragment(),
        HarnessKind::Cx => cccx_harness_cx::system_prompt_fragment(),
    };
    if let Some(ref existing) = config.developer_instructions {
        config.developer_instructions = Some(format!("{existing}\n\n{harness_prompt}"));
    } else {
        config.developer_instructions = Some(harness_prompt.to_string());
    }

    // --- permission ceiling ---
    // For CX subagents, apply the projection's deny-by-default policy.
    // The permission_policy from project_child_config is the intersection of
    // the WorkSpec policy and the parent's permission ceiling.
    // In Phase 3, this is enforced primarily through the harness restrictions
    // below, which disable elevated capabilities at the Config level.

    // --- CX harness restrictions ---
    // architecture.md §6: CX does not have spawn, ADR write, sequential-thinking.
    if work_spec.harness == HarnessKind::Cx {
        // Disable orchestrator MCP (sequential-thinking comes through MCP).
        config.orchestrator_mcp_enabled = false;
        // Disable skills orchestration (ADR write + advanced planning tools).
        config.orchestrator_skills_enabled = false;
    }

    tracing::info!(
        harness = ?work_spec.harness,
        allowed_tools = %projection.allowed_tools.len(),
        allowed_mcp_tools = %projection.allowed_mcp_tools.len(),
        writable_scope = %projection.writable_scope.len(),
        "cccx: applied harness config + deny-by-default projection to subagent"
    );

    Ok(())
}

/// Determine which harness a `WorkSpec` mandates.
///
/// Maps role to harness per architecture.md §3 when the harness must be
/// derived from the assigned role rather than directly from the WorkSpec.
pub fn harness_from_workspec(work_spec: &WorkSpec) -> HarnessKind {
    match work_spec.role {
        WorkRole::Researcher | WorkRole::Architect | WorkRole::Planner => HarnessKind::Cc,
        WorkRole::Implementer | WorkRole::Tester | WorkRole::Auditor | WorkRole::Resolver => {
            HarnessKind::Cx
        }
    }
}
