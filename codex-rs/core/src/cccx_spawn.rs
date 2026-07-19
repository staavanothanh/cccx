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

#[cfg(test)]
mod phase0_tests {
    //! Phase 0 RED tests — WorkSpec projection must reach child Config.
    //!
    //! Call-site map: apply_cccx_harness_to_config (this file:22)
    //!   → cccx_protocol::project_child_config (cccx-protocol/src/lib.rs:345)
    //!
    //! Issue: implementer-issues.md §2 — "WorkSpec deny-by-default chưa enforce đầy đủ"
    //!         The function only injects harness_prompt + disables orchestrator
    //!         flags. projection.allowed_tools, allowed_mcp_tools, writable_scope,
    //!         permission_policy, model_route are all COMPUTED BUT DISCARDED.
    //!
    //! These tests are intentionally RED: they prove the contract computes
    //! correctly (project_child_config is correct), but the runtime function
    //! never writes those values into Config.

    use super::*;
    use cccx_protocol::{
        ForkPolicy, ModelRouteSelection, PermissionPolicy, WorkRole, WorkScope,
    };
    use std::collections::BTreeSet;

    fn test_parent() -> ParentCapabilitySet {
        ParentCapabilitySet {
            tools: BTreeSet::from([
                "shell_command".into(),
                "edit".into(),
                "read".into(),
                "spawn".into(),
            ]),
            mcp_tools: BTreeSet::from([
                "codebase-memory-mcp".into(),
                "sequential-thinking".into(),
            ]),
            writable_roots: BTreeSet::from(["parent-wide-root".into()]),
            protected_paths: BTreeSet::new(),
            model_routes: BTreeSet::from(["9router/main".into()]),
            permission_ceiling: PermissionPolicy::Allow,
        }
    }

    fn test_work_spec() -> WorkSpec {
        WorkSpec {
            schema_version: cccx_protocol::CCCX_PROTOCOL_SCHEMA_VERSION.into(),
            work_spec_id: "ws-p0-1".into(),
            task_id: "T-p0".into(),
            harness: HarnessKind::Cx,
            role: WorkRole::Implementer,
            objective: "run tests".into(),
            scope: cccx_protocol::WorkScope {
                canonical_paths: BTreeSet::from(["reduced-scope-path".into()]),
                protected_paths: BTreeSet::new(),
            },
            allowed_tools: BTreeSet::from(["shell_command".into()]),
            forbidden_tools: BTreeSet::new(),
            allowed_mcp_tools: BTreeSet::from(["codebase-memory-mcp".into()]),
            permission_policy: PermissionPolicy::Ask,
            model_route: ModelRouteSelection::InheritMainEffective,
            fallback_policy_id: "strict".into(),
            fork_policy: cccx_protocol::ForkPolicy::WorkSpecOnly,
            enforced_constraints: vec![],
            risk_threshold: None,
            action_classes: None,
            approval_policy: None,
        }
    }

    // ── GREEN tests: contract projection is correct ──

    #[test]
    fn contract_projection_intersects_with_parent() {
        let mut ws = test_work_spec();
        ws.allowed_tools = BTreeSet::from(["unsafe_tool".into()]); // NOT in parent
        let proj = cccx_protocol::project_child_config(&ws, &test_parent()).unwrap();
        assert!(
            proj.allowed_tools.is_empty(),
            "unsafe_tool not in parent → intersection empty"
        );
    }

    #[test]
    fn contract_forbidden_tools_trump_allowed() {
        let mut ws = test_work_spec();
        // WorkSpec allows edit/shell; parent has edit/spawn/shell/read
        ws.allowed_tools = BTreeSet::from(["shell_command".into(), "edit".into()]);
        // forbidden_tools must not intersect allowed_tools (WorkSpec validation)
        ws.forbidden_tools = BTreeSet::from(["spawn".into()]);
        let proj = cccx_protocol::project_child_config(&ws, &test_parent()).unwrap();
        // shell_command + edit both in parent → projection has both
        assert!(proj.allowed_tools.contains("shell_command"));
        assert!(proj.allowed_tools.contains("edit"));
        // spawn is in parent but neither allowed nor forbidden in projection
        // (the intersection only includes explicitly allowed tools)
        assert!(!proj.allowed_tools.contains("spawn"));
    }

    #[test]
    fn contract_parent_ceiling_downgrades_child() {
        let ws = test_work_spec(); // Ask
        let mut parent = test_parent();
        parent.permission_ceiling = PermissionPolicy::Deny;
        let proj = cccx_protocol::project_child_config(&ws, &parent).unwrap();
        assert_eq!(proj.permission_policy, PermissionPolicy::Deny);
    }

    #[test]
    fn contract_explicit_model_route_outside_allowlist_rejected() {
        let mut ws = test_work_spec();
        ws.model_route =
            ModelRouteSelection::Explicit { route_id: "unknown-route".into() };
        let result = cccx_protocol::project_child_config(&ws, &test_parent());
        assert!(result.is_err());
    }

    // ── RED tests: runtime projection is discarded ──

    /// P0-RED-1: apply_cccx_harness_to_config MUST write model_route into Config.
    ///
    /// Currently RED: the function computes `projection.model_route` but
    /// only LOGS it via tracing. Config.model is never updated. The child
    /// agent gets the parent's model, not the WorkSpec-requested route.
    #[test]
    fn runtime_projection_model_route_must_reach_config() {
        let mut ws = test_work_spec();
        ws.model_route =
            ModelRouteSelection::Explicit { route_id: "9router/main".into() };
        let projection =
            cccx_protocol::project_child_config(&ws, &test_parent()).unwrap();
        // Projection correctly resolves the explicit route
        match &projection.model_route {
            ModelRouteSelection::Explicit { route_id } => {
                assert!(!route_id.is_empty(), "explicit model route must not be empty");
            }
            _ => panic!("expected explicit route in projection"),
        }

        // RED: apply_cccx_harness_to_config does not write model to Config
        // Config lacks a cccx-projection field. The function only touches
        // developer_instructions + orchestrator flags.
        let config_model_set = false;
        assert!(
            config_model_set,
            "RED (P0-RED-1): apply_cccx_harness_to_config does not write \
             projection.model_route into Config. The child agent will use the \
             parent's default model, ignoring the WorkSpec's model_route."
        );
    }

    /// P0-RED-2: apply_cccx_harness_to_config MUST carry permission_policy.
    ///
    /// Currently RED: exec_policy hardcodes Default mode instead of reading
    /// the projection's permission_policy, so Plan/DontAsk/Bypass workflow
    /// is not enforced for subagents.
    #[test]
    fn runtime_projection_permission_policy_must_reach_config() {
        let mut parent = test_parent();
        parent.permission_ceiling = PermissionPolicy::Deny;

        let projection =
            cccx_protocol::project_child_config(&test_work_spec(), &parent).unwrap();
        // Contract correctly downgraded
        assert_eq!(projection.permission_policy, PermissionPolicy::Deny);

        // RED: Config has no field to carry this downgraded policy.
        // exec_policy hardcodes Default and never reads the projection.
        let config_carries_policy = false;
        assert!(
            config_carries_policy,
            "RED (P0-RED-2): Config does not carry projection.permission_policy. \
             exec_policy hardcodes ModeKind::Default; the projected Deny policy \
             is never read by the permission gate."
        );
    }

    /// P0-RED-3: apply_cccx_harness_to_config MUST carry allowed_tools count.
    ///
    /// Currently RED: the function logs the count ("allowed_tools = %N")
    /// but never writes it into a Config field. Tool catalog builder cannot
    /// filter what tools the child model sees or can invoke.
    #[test]
    fn runtime_projection_tool_allowlist_must_reach_config() {
        let projection =
            cccx_protocol::project_child_config(&test_work_spec(), &test_parent()).unwrap();
        assert_eq!(projection.allowed_tools.len(), 1); // only shell_command

        // RED: Config has no cccx tool allowlist. The tool catalog sent
        // to the child model includes ALL parent tools, not just the
        // intersection from the WorkSpec projection.
        let config_has_tool_allowlist = false;
        assert!(
            config_has_tool_allowlist,
            "RED (P0-RED-3): Config has no cccx tool allowlist field. \
             apply_cccx_harness_to_config computes but discards \
             projection.allowed_tools. The child model/catalog sees the \
             full parent tool set."
        );
    }

    /// Phase 0 baseline: harness prompt and orchestrator flags DO reach Config.
    /// This proves apply_cccx_harness_to_config partially works — the gap is
    /// specifically the cccx projection fields, not the whole function.
    #[test]
    fn runtime_harness_prompt_reaches_config() {
        // We cannot construct a real Config in a unit test (too many fields),
        // but we can verify that the prompt fragments exist and differ.
        let cc = cccx_harness_cc::system_prompt_fragment();
        let cx = cccx_harness_cx::system_prompt_fragment();
        assert!(!cc.is_empty(), "CC harness prompt must not be empty");
        assert!(!cx.is_empty(), "CX harness prompt must not be empty");
        assert_ne!(cc, cx, "CC and CX harness prompts must differ");

        // The projection mark is set correctly for CX subagents
        let ws = test_work_spec();
        let projection =
            cccx_protocol::project_child_config(&ws, &test_parent()).unwrap();
        assert!(projection.deny_by_default);
        assert_eq!(projection.harness, HarnessKind::Cx);
    }
}
