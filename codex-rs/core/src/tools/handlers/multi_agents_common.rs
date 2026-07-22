use crate::cccx_runtime::{
    CccxSpawnRuntime, parent_capability_set_for_work_spec, permission_ceiling_from_approval_policy,
};
use crate::config::Config;
use crate::config::DEFAULT_MULTI_AGENT_V2_MIN_WAIT_TIMEOUT_MS;
use crate::config::HARD_MAX_MULTI_AGENT_V2_TIMEOUT_MS;
use crate::function_tool::FunctionCallError;
use crate::session::session::Session;
use crate::session::step_context::StepContext;
use crate::session::turn::built_tools;
use crate::session::turn_context::TurnContext;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolOutput;
use crate::tools::context::ToolPayload;
use codex_models_manager::manager::RefreshStrategy;
use codex_protocol::AgentPath;
use codex_protocol::ThreadId;
use codex_protocol::error::CodexErr;
use codex_protocol::models::BaseInstructions;
use codex_protocol::models::ResponseInputItem;
use codex_protocol::openai_models::ModelPreset;
use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::openai_models::ReasoningEffortPreset;
use codex_protocol::protocol::MultiAgentVersion;
use codex_protocol::protocol::SessionSource;
use codex_protocol::protocol::SubAgentSource;
use codex_protocol::user_input::UserInput;
use serde::Serialize;
use serde_json::Value as JsonValue;
use std::collections::BTreeSet;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// Minimum wait timeout to prevent tight polling loops from burning CPU.
pub(crate) const MIN_WAIT_TIMEOUT_MS: i64 = DEFAULT_MULTI_AGENT_V2_MIN_WAIT_TIMEOUT_MS;
pub(crate) const DEFAULT_WAIT_TIMEOUT_MS: i64 = 30_000;
pub(crate) const MAX_WAIT_TIMEOUT_MS: i64 = HARD_MAX_MULTI_AGENT_V2_TIMEOUT_MS;
pub(crate) const MAX_SPAWN_AGENT_MODEL_OVERRIDES: usize = 5;
const CCCX_SPAWN_INPUT_ENVELOPE_SCHEMA_VERSION: &str = "cccx.spawn_input.v1";
const CCCX_MAX_EXECUTION_BRIEF_CHARS: usize = 4_000;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CccxSpawnInputEnvelope<'a> {
    schema_version: &'static str,
    kind: &'static str,
    work_spec: &'a cccx_protocol::WorkSpec,
    execution_brief: &'a str,
}

fn filter_cx_extension_mcp_grants_for_projection(
    parent_allowed_mcp_tools: &mut BTreeSet<String>,
    catalog: &codex_core_plugins::cccx_extension::CccxExtensionMcpCatalog,
) {
    parent_allowed_mcp_tools.retain(|grant| catalog.grant_allowed_for_cx(grant));
}

pub(crate) fn model_supports_multi_agent_backend(
    model: &ModelPreset,
    multi_agent_version: MultiAgentVersion,
) -> bool {
    multi_agent_version != MultiAgentVersion::V2
        || model.multi_agent_version == Some(multi_agent_version)
}

pub(crate) fn function_arguments(payload: ToolPayload) -> Result<String, FunctionCallError> {
    match payload {
        ToolPayload::Function { arguments } => Ok(arguments),
        _ => Err(FunctionCallError::RespondToModel(
            "collab handler received unsupported payload".to_string(),
        )),
    }
}

pub(crate) fn tool_output_json_text<T>(value: &T, tool_name: &str) -> String
where
    T: Serialize,
{
    serde_json::to_string(value).unwrap_or_else(|err| {
        JsonValue::String(format!("failed to serialize {tool_name} result: {err}")).to_string()
    })
}

pub(crate) fn tool_output_response_item<T>(
    call_id: &str,
    payload: &ToolPayload,
    value: &T,
    success: Option<bool>,
    tool_name: &str,
) -> ResponseInputItem
where
    T: Serialize,
{
    FunctionToolOutput::from_text(tool_output_json_text(value, tool_name), success)
        .to_response_item(call_id, payload)
}

pub(crate) fn tool_output_code_mode_result<T>(value: &T, tool_name: &str) -> JsonValue
where
    T: Serialize,
{
    serde_json::to_value(value).unwrap_or_else(|err| {
        JsonValue::String(format!("failed to serialize {tool_name} result: {err}"))
    })
}

pub(crate) fn collab_spawn_error(err: CodexErr) -> FunctionCallError {
    match err {
        CodexErr::UnsupportedOperation(message) if message == "thread manager dropped" => {
            FunctionCallError::RespondToModel("collab manager unavailable".to_string())
        }
        CodexErr::UnsupportedOperation(message) => FunctionCallError::RespondToModel(message),
        err => FunctionCallError::RespondToModel(format!("collab spawn failed: {err}")),
    }
}

pub(crate) fn collab_agent_error(agent_id: ThreadId, err: CodexErr) -> FunctionCallError {
    match err {
        CodexErr::ThreadNotFound(id) => {
            FunctionCallError::RespondToModel(format!("agent with id {id} not found"))
        }
        CodexErr::InternalAgentDied => {
            FunctionCallError::RespondToModel(format!("agent with id {agent_id} is closed"))
        }
        CodexErr::UnsupportedOperation(_) => {
            FunctionCallError::RespondToModel("collab manager unavailable".to_string())
        }
        err => FunctionCallError::RespondToModel(format!("collab tool failed: {err}")),
    }
}

pub(crate) fn thread_spawn_source(
    parent_thread_id: ThreadId,
    parent_session_source: &SessionSource,
    depth: i32,
    agent_role: Option<&str>,
    task_name: Option<String>,
) -> Result<SessionSource, FunctionCallError> {
    let agent_path = task_name
        .as_deref()
        .map(|task_name| {
            parent_session_source
                .get_agent_path()
                .unwrap_or_else(AgentPath::root)
                .join(task_name)
                .map_err(FunctionCallError::RespondToModel)
        })
        .transpose()?;
    Ok(SessionSource::SubAgent(SubAgentSource::ThreadSpawn {
        parent_thread_id,
        depth,
        agent_path,
        agent_nickname: None,
        agent_role: agent_role.map(str::to_string),
    }))
}

pub(crate) fn parse_collab_input(
    message: Option<String>,
    items: Option<Vec<UserInput>>,
) -> Result<Vec<UserInput>, FunctionCallError> {
    match (message, items) {
        (Some(_), Some(_)) => Err(FunctionCallError::RespondToModel(
            "Provide either message or items, but not both".to_string(),
        )),
        (None, None) => Err(FunctionCallError::RespondToModel(
            "Provide one of: message or items".to_string(),
        )),
        (Some(message), None) => {
            if message.trim().is_empty() {
                return Err(FunctionCallError::RespondToModel(
                    "Empty message can't be sent to an agent".to_string(),
                ));
            }
            Ok(vec![UserInput::Text {
                text: message,
                text_elements: Vec::new(),
            }])
        }
        (None, Some(items)) => {
            if items.is_empty() {
                return Err(FunctionCallError::RespondToModel(
                    "Items can't be empty".to_string(),
                ));
            }
            Ok(items)
        }
    }
}

fn bounded_cccx_execution_brief(message: &str) -> Result<String, FunctionCallError> {
    if message.trim().is_empty() {
        return Err(FunctionCallError::RespondToModel(
            "cccx WorkSpec spawn requires a non-empty bounded execution brief in `message`"
                .to_string(),
        ));
    }
    if message.chars().count() > CCCX_MAX_EXECUTION_BRIEF_CHARS {
        return Err(FunctionCallError::RespondToModel(format!(
            "cccx WorkSpec spawn execution brief is too large; keep `message` under {CCCX_MAX_EXECUTION_BRIEF_CHARS} characters and put durable state in WorkSpec/Artifact"
        )));
    }
    Ok(message.to_string())
}

pub(crate) fn cccx_spawn_v1_execution_brief(
    work_spec: Option<&cccx_protocol::WorkSpec>,
    message: Option<&str>,
    items: Option<&[UserInput]>,
) -> Result<Option<String>, FunctionCallError> {
    if !cccx_owns_spawn_model_resolution(work_spec) {
        return Ok(None);
    }
    if items.is_some() {
        return Err(FunctionCallError::RespondToModel(
            "cccx WorkSpec spawn accepts only a bounded `message` execution brief; structured `items` would expose arbitrary parent context to the child"
                .to_string(),
        ));
    }
    let message = message.ok_or_else(|| {
        FunctionCallError::RespondToModel(
            "cccx WorkSpec spawn requires `message` as a bounded execution brief".to_string(),
        )
    })?;
    bounded_cccx_execution_brief(message).map(Some)
}

pub(crate) fn cccx_spawn_v2_execution_brief(
    work_spec: Option<&cccx_protocol::WorkSpec>,
    message: &str,
) -> Result<Option<String>, FunctionCallError> {
    if !cccx_owns_spawn_model_resolution(work_spec) {
        return Ok(None);
    }
    bounded_cccx_execution_brief(message).map(Some)
}

pub(crate) fn cccx_work_spec_spawn_envelope_text(
    work_spec: &cccx_protocol::WorkSpec,
    execution_brief: &str,
) -> Result<String, FunctionCallError> {
    work_spec.validate().map_err(|err| {
        FunctionCallError::RespondToModel(format!("cccx WorkSpec is invalid: {err}"))
    })?;
    let execution_brief = bounded_cccx_execution_brief(execution_brief)?;
    serde_json::to_string(&CccxSpawnInputEnvelope {
        schema_version: CCCX_SPAWN_INPUT_ENVELOPE_SCHEMA_VERSION,
        kind: "cccx.work_spec_spawn",
        work_spec,
        execution_brief: execution_brief.as_str(),
    })
    .map_err(|err| {
        FunctionCallError::RespondToModel(format!(
            "cccx could not serialize WorkSpec spawn envelope: {err}"
        ))
    })
}

pub(crate) fn cccx_work_spec_spawn_input_items(
    cccx_runtime: Option<&Arc<CccxSpawnRuntime>>,
    execution_brief: Option<&str>,
) -> Result<Option<Vec<UserInput>>, FunctionCallError> {
    let Some(runtime) = cccx_runtime else {
        return Ok(None);
    };
    let execution_brief = execution_brief.ok_or_else(|| {
        FunctionCallError::RespondToModel(
            "cccx WorkSpec spawn missing bounded execution brief after runtime projection"
                .to_string(),
        )
    })?;
    Ok(Some(vec![UserInput::Text {
        text: cccx_work_spec_spawn_envelope_text(&runtime.work_spec, execution_brief)?,
        text_elements: Vec::new(),
    }]))
}

pub(crate) fn cccx_work_spec_spawn_message(
    cccx_runtime: Option<&Arc<CccxSpawnRuntime>>,
    execution_brief: Option<&str>,
) -> Result<Option<String>, FunctionCallError> {
    let Some(runtime) = cccx_runtime else {
        return Ok(None);
    };
    let execution_brief = execution_brief.ok_or_else(|| {
        FunctionCallError::RespondToModel(
            "cccx WorkSpec spawn missing bounded execution brief after runtime projection"
                .to_string(),
        )
    })?;
    cccx_work_spec_spawn_envelope_text(&runtime.work_spec, execution_brief).map(Some)
}

/// Builds the base config snapshot for a newly spawned sub-agent.
///
/// The returned config starts from the parent's effective config and then refreshes the
/// runtime-owned fields carried on `turn`, including model selection, reasoning settings,
/// approval policy, sandbox, and cwd. Role-specific overrides are layered after this step;
/// skipping this helper and cloning stale config state directly can send the child agent out with
/// the wrong provider or runtime policy.
pub(crate) fn build_agent_spawn_config(
    base_instructions: &BaseInstructions,
    turn: &TurnContext,
) -> Result<Config, FunctionCallError> {
    let mut config = build_agent_shared_config(turn)?;
    config.base_instructions = Some(base_instructions.text.clone());
    Ok(config)
}

pub(crate) fn build_agent_resume_config(turn: &TurnContext) -> Result<Config, FunctionCallError> {
    let mut config = build_agent_shared_config(turn)?;
    // For resume, keep base instructions sourced from rollout/session metadata.
    config.base_instructions = None;
    Ok(config)
}

fn build_agent_shared_config(turn: &TurnContext) -> Result<Config, FunctionCallError> {
    let base_config = turn.config.clone();
    let mut config = (*base_config).clone();
    config.model = Some(turn.model_info.slug.clone());
    config.model_provider = turn.provider.info().clone();
    config.model_reasoning_effort = turn
        .reasoning_effort
        .clone()
        .or_else(|| turn.model_info.default_reasoning_level.clone());
    config.model_reasoning_summary = Some(turn.reasoning_summary);
    config.developer_instructions = turn.developer_instructions.clone();
    apply_spawn_agent_runtime_overrides(&mut config, turn)?;

    Ok(config)
}

pub(crate) fn reject_full_fork_spawn_overrides(
    agent_type: Option<&str>,
    model: Option<&str>,
    reasoning_effort: Option<ReasoningEffort>,
) -> Result<(), FunctionCallError> {
    if agent_type.is_some() || model.is_some() || reasoning_effort.is_some() {
        return Err(FunctionCallError::RespondToModel(
            "Full-history forked agents inherit the parent agent type, model, and reasoning effort; omit agent_type, model, and reasoning_effort, or spawn without a full-history fork.".to_string(),
        ));
    }
    Ok(())
}

/// Copies runtime-only turn state onto a child config before it is handed to `AgentControl`.
///
/// These values are chosen by the live turn rather than persisted config, so leaving them stale
/// can make a child agent disagree with its parent about approval policy, cwd, or sandboxing.
pub(crate) fn apply_spawn_agent_runtime_overrides(
    config: &mut Config,
    turn: &TurnContext,
) -> Result<(), FunctionCallError> {
    config
        .permissions
        .approval_policy
        .set(turn.approval_policy.value())
        .map_err(|err| {
            FunctionCallError::RespondToModel(format!("approval_policy is invalid: {err}"))
        })?;
    config.approvals_reviewer = turn.config.approvals_reviewer;
    #[allow(deprecated)]
    let turn_cwd = turn.cwd.clone();
    config.cwd = turn_cwd;
    config
        .permissions
        .set_permission_profile(turn.permission_profile())
        .map_err(|err| {
            FunctionCallError::RespondToModel(format!("permission_profile is invalid: {err}"))
        })?;
    Ok(())
}

pub(crate) async fn apply_cccx_work_spec_to_spawn_config(
    session: &Session,
    step_context: &StepContext,
    cancellation_token: &CancellationToken,
    config: &mut Config,
    turn: &TurnContext,
    work_spec: Option<cccx_protocol::WorkSpec>,
    task_name: Option<&str>,
    agent_model_suggestion: Option<&str>,
) -> Result<Option<Arc<CccxSpawnRuntime>>, FunctionCallError> {
    if !codex_features::cccx_mvp::surface_gate_enabled() {
        return Ok(None);
    }

    // Task 2: guard — reject spawn if caller is not Main CC.
    // Subagents are forbidden from spawning more workers. This uses the typed
    // runtime identity instead of prompt substring heuristics, so future prompt
    // edits cannot accidentally grant scheduler authority.
    if !turn.cccx_actor_identity().can_spawn() {
        return Err(FunctionCallError::RespondToModel(format!(
            "cccx: {:?} workers cannot spawn additional agents. Only Main CC has scheduler authority. \
             Return a SchedulingRequest Artifact describing what additional worker/scope you need; the scheduler will dispatch it.",
            turn.cccx_actor_identity()
        )));
    }

    let Some(mut work_spec) = work_spec else {
        return Err(FunctionCallError::RespondToModel(
            "cccx spawn requires a structured `work_spec`. Retry spawn_agent with a WorkSpec containing task_id, harness, role, objective, scope, allowed_tools, allowed_mcp_tools, permission_policy, and model_route. Keep `message` as a compact execution brief; do not pass raw parent context."
                .to_string(),
        ));
    };
    #[allow(deprecated)]
    let project_root = turn.cwd.clone().to_path_buf();
    work_spec.scope.canonical_paths = crate::cccx_paths::resolve_physical_paths(
        work_spec.scope.canonical_paths.iter().cloned(),
        &project_root,
    )
    .map_err(|error| {
        FunctionCallError::RespondToModel(format!(
            "cccx could not canonicalize WorkSpec scope: {error}"
        ))
    })?
    .into_iter()
    .collect();
    work_spec.scope.protected_paths = crate::cccx_paths::resolve_physical_paths(
        work_spec.scope.protected_paths.iter().cloned(),
        &project_root,
    )
    .map_err(|error| {
        FunctionCallError::RespondToModel(format!(
            "cccx could not canonicalize WorkSpec protected paths: {error}"
        ))
    })?
    .into_iter()
    .collect();
    let role_preset =
        crate::cccx_agent_model_routes::route_for_work_role(&config.codex_home, &work_spec.role)
            .await
            .map_err(|error| FunctionCallError::RespondToModel(error.to_string()))?;
    let configured_model_catalog =
        cccx_provider_gateway::LocalConfigCatalog::load_from_home_or_current(
            &config.codex_home,
            &turn.model_info.slug,
        )
        .map_err(|error| {
            FunctionCallError::RespondToModel(format!(
                "cccx could not load the configured 9router model catalog before spawn: {error}"
            ))
        })?;
    // The requested model is always known locally. An effective route is only
    // eligible for `inherit_main_effective` after 9router has actually
    // reported it on this Main thread; unknown remains fail-closed.
    let main_effective_route_id = crate::cccx_gateway_provenance_state::load_gateway_provenance(
        &project_root,
        &session.thread_id().to_string(),
    )
    .await
    .map_err(|error| {
        FunctionCallError::RespondToModel(format!(
            "cccx could not load Main gateway provenance before spawn: {error}"
        ))
    })?
    .and_then(|provenance| provenance.effective_route_id);
    let resolved_model_route =
        crate::cccx_agent_model_routes::resolve_spawn_model_route_with_agent_suggestion_and_main_effective(
            &configured_model_catalog,
            &turn.model_info.slug,
            main_effective_route_id.as_deref(),
            &work_spec.model_route,
            role_preset,
            agent_model_suggestion,
        )
        .map_err(|error| FunctionCallError::RespondToModel(error.to_string()))?;
    work_spec.model_route = cccx_protocol::ModelRouteSelection::Explicit {
        route_id: resolved_model_route.route_id.clone(),
    };
    work_spec.fallback_policy_id = resolved_model_route.fallback_policy_id.clone();
    work_spec.resolved_model_route = Some(cccx_protocol::ResolvedModelRoute {
        selection_source: match resolved_model_route.selection_source.as_str() {
            "user_override" => cccx_protocol::RouteSelectionSource::UserOverride,
            "role_preset" => cccx_protocol::RouteSelectionSource::RolePreset,
            "agent_suggested" => cccx_protocol::RouteSelectionSource::AgentSuggested,
            "session_default" => cccx_protocol::RouteSelectionSource::SessionDefault,
            "inherit_main_requested" => cccx_protocol::RouteSelectionSource::InheritMainRequested,
            "inherit_main_effective" => cccx_protocol::RouteSelectionSource::InheritMainEffective,
            "global_default" => cccx_protocol::RouteSelectionSource::GlobalDefault,
            source => {
                return Err(FunctionCallError::RespondToModel(format!(
                    "cccx model route resolver returned unknown selection source `{source}`"
                )));
            }
        },
        requested_route_id: resolved_model_route.route_id.clone(),
        model_profile_id: resolved_model_route.model_profile_id.clone(),
        capability_profile_id: resolved_model_route.capability_profile_id.clone(),
        fallback_policy_id: resolved_model_route.fallback_policy_id.clone(),
        parent_attempt_id: Some(turn.sub_id.clone()),
        parent_requested_route_id: Some(turn.model_info.slug.clone()),
        parent_effective_route_id: main_effective_route_id.clone(),
        fallback_chain: Vec::new(),
        candidate_route_ids: configured_model_catalog.available_models.clone(),
    });

    let parent_router = built_tools(session, step_context, cancellation_token)
        .await
        .map_err(|error| {
            FunctionCallError::RespondToModel(format!(
                "cccx could not snapshot the parent tool catalog before spawn: {error}"
            ))
        })?;
    let parent_allowed_tools = parent_router
        .model_visible_specs()
        .into_iter()
        .map(|spec| spec.name().to_string())
        .collect();
    let mut parent_allowed_mcp_tools: BTreeSet<String> = step_context
        .mcp_tools()
        .await
        .iter()
        .flat_map(|tool| {
            [
                tool.server_name.clone(),
                tool.tool.name.to_string(),
                tool.canonical_tool_name().to_string(),
                format!("mcp:{}.{}", tool.server_name, tool.tool.name),
            ]
        })
        .collect();
    if matches!(work_spec.harness, cccx_protocol::HarnessKind::Cx) {
        let loaded_plugins = session
            .services
            .plugins_manager
            .plugins_for_config(&turn.config.plugins_config_input())
            .await;
        let extension_mcp_catalog =
            codex_core_plugins::cccx_extension::cccx_extension_mcp_catalog(&loaded_plugins);
        filter_cx_extension_mcp_grants_for_projection(
            &mut parent_allowed_mcp_tools,
            &extension_mcp_catalog,
        );
    }
    let parent_model_routes = configured_model_catalog
        .available_models
        .iter()
        .cloned()
        .collect();
    let parent_file_system_policy = turn.file_system_sandbox_policy();
    let parent_writable_roots = if parent_file_system_policy.has_full_disk_write_access() {
        work_spec.scope.canonical_paths.clone()
    } else {
        crate::cccx_paths::resolve_physical_paths(
            parent_file_system_policy
                .get_writable_roots_with_cwd(config.cwd.as_path())
                .into_iter()
                .map(|root| root.root.to_string_lossy().to_string()),
            &project_root,
        )
        .map_err(|error| {
            FunctionCallError::RespondToModel(format!(
                "cccx could not canonicalize parent writable roots: {error}"
            ))
        })?
        .into_iter()
        .collect()
    };
    let parent_permission_ceiling = permission_ceiling_from_approval_policy(
        turn.approval_policy.value(),
        config.cccx_child_permission_policy.clone(),
    );

    // Parent capability is captured from the live turn registry, never from
    // the child's requested WorkSpec. A child can only receive the strict
    // intersection at this exact spawn boundary.
    let parent = parent_capability_set_for_work_spec(
        &work_spec,
        parent_allowed_tools,
        parent_allowed_mcp_tools,
        parent_writable_roots,
        parent_model_routes,
        parent_permission_ceiling,
    );
    crate::cccx_spawn::apply_cccx_harness_to_config(config, &parent, &work_spec)
        .map_err(|error| FunctionCallError::RespondToModel(error.to_string()))?;
    let route_summary = resolved_model_route.route_id.clone();
    let display_name = task_name
        .map(str::to_string)
        .unwrap_or_else(|| format!("{}-{:?}", work_spec.task_id, work_spec.role));

    Ok(Some(Arc::new(CccxSpawnRuntime {
        project_root,
        logical_session_id: work_spec.work_spec_id.clone(),
        work_spec,
        display_name,
        model_route_summary: route_summary,
        model_route_selection_source: Some(resolved_model_route.selection_source),
        gateway_provenance: None,
        tool_call_sequence_digest: None,
        parent_capabilities: Some(parent),
    })))
}

#[cfg(test)]
mod cccx_plugin_projection_tests {
    use super::*;
    use cccx_protocol::{
        ForkPolicy, HarnessKind, ModelRouteSelection, PermissionPolicy, RevisionRef, WorkRole,
        WorkScope, WorkSpec,
    };
    use codex_core_plugins::cccx_extension::CccxExtensionMcpCatalog;
    use serde_json::Value;
    use std::collections::BTreeSet;
    use std::sync::Arc;

    fn work_spec() -> WorkSpec {
        WorkSpec {
            schema_version: cccx_protocol::CCCX_WORK_SPEC_SCHEMA_VERSION.to_string(),
            work_spec_id: "ws-boundary-1".to_string(),
            task_id: "T-boundary".to_string(),
            attempt_id: "attempt-boundary-1".to_string(),
            created_at: "2026-07-22T00:00:00Z".to_string(),
            expires_at: None,
            base_revision: RevisionRef {
                git_head: Some("base-head".to_string()),
                working_tree_digest: None,
            },
            harness: HarnessKind::Cx,
            role: WorkRole::Implementer,
            runtime_identity: None,
            objective: "Apply the bounded patch and run focused tests".to_string(),
            scope: WorkScope {
                canonical_paths: BTreeSet::from(["D:/repo/codex-rs/core".to_string()]),
                protected_paths: BTreeSet::new(),
                advisory_globs: BTreeSet::new(),
            },
            allowed_tools: BTreeSet::from(["shell_command".to_string()]),
            forbidden_tools: BTreeSet::new(),
            allowed_mcp_tools: BTreeSet::new(),
            tool_grants: Vec::new(),
            permission_policy: PermissionPolicy::Ask,
            model_route: ModelRouteSelection::Explicit {
                route_id: "9router/cx-implementer".to_string(),
            },
            resolved_model_route: None,
            fallback_policy_id: "strict".to_string(),
            fork_policy: ForkPolicy::WorkSpecOnly,
            enforced_constraints: vec!["do-not-touch-unowned-files".to_string()],
            typed_constraints: Vec::new(),
            advisory_constraints: Vec::new(),
            risk_threshold: None,
            action_classes: None,
            approval_policy: None,
            verification_spec: None,
            audit_depth: None,
            expected_artifact: None,
            checkpoint_id: None,
            base_commit: None,
        }
    }

    #[test]
    fn cx_projection_strips_incompatible_extension_mcp_but_preserves_builtin_mcp() {
        let mut parent_allowed = BTreeSet::from([
            "mcp:plugin.cc_only".to_string(),
            "mcp:plugin.cx_read".to_string(),
            "mcp:codebase-memory.search_graph".to_string(),
        ]);
        let catalog = CccxExtensionMcpCatalog {
            extension_stable_ids: BTreeSet::from([
                "mcp:plugin.cc_only".to_string(),
                "mcp:plugin.cx_read".to_string(),
            ]),
            cx_compatible_stable_ids: BTreeSet::from(["mcp:plugin.cx_read".to_string()]),
        };

        filter_cx_extension_mcp_grants_for_projection(&mut parent_allowed, &catalog);

        assert_eq!(
            parent_allowed,
            BTreeSet::from([
                "mcp:plugin.cx_read".to_string(),
                "mcp:codebase-memory.search_graph".to_string(),
            ]),
            "CX child projection must fail closed for extension MCP grants that are not CX-compatible while preserving non-extension MCP grants"
        );
    }

    #[test]
    fn cccx_spawn_envelope_contains_validated_work_spec_and_bounded_brief() {
        let ws = work_spec();
        let envelope =
            cccx_work_spec_spawn_envelope_text(&ws, "Use the WorkSpec as authority.").unwrap();
        let value: Value = serde_json::from_str(&envelope).unwrap();

        assert_eq!(value["schemaVersion"], "cccx.spawn_input.v1");
        assert_eq!(value["kind"], "cccx.work_spec_spawn");
        assert_eq!(value["executionBrief"], "Use the WorkSpec as authority.");
        assert_eq!(value["workSpec"]["schema_version"], "cccx.work_spec.v1");
        assert_eq!(value["workSpec"]["work_spec_id"], "ws-boundary-1");
        assert_eq!(
            value["workSpec"]["model_route"]["route_id"],
            "9router/cx-implementer"
        );
    }

    #[test]
    fn cccx_spawn_envelope_rejects_invalid_work_spec() {
        let mut ws = work_spec();
        ws.objective.clear();

        let err = cccx_work_spec_spawn_envelope_text(&ws, "brief").unwrap_err();

        assert_eq!(
            err,
            FunctionCallError::RespondToModel(
                "cccx WorkSpec is invalid: missing required field `objective`".to_string()
            )
        );
    }

    #[test]
    fn cccx_v1_work_spec_spawn_rejects_structured_items() {
        let ws = work_spec();
        let items = vec![UserInput::Text {
            text: "raw parent context".to_string(),
            text_elements: Vec::new(),
        }];

        let err =
            cccx_spawn_v1_execution_brief(Some(&ws), None, Some(items.as_slice())).unwrap_err();

        assert_eq!(
            err,
            FunctionCallError::RespondToModel(
                "cccx WorkSpec spawn accepts only a bounded `message` execution brief; structured `items` would expose arbitrary parent context to the child"
                    .to_string()
            )
        );
    }

    #[test]
    fn cccx_work_spec_spawn_input_items_replaces_free_form_child_input() {
        let runtime = Arc::new(CccxSpawnRuntime {
            project_root: std::path::PathBuf::from("D:/repo"),
            logical_session_id: "ws-boundary-1".to_string(),
            work_spec: work_spec(),
            display_name: "boundary-worker".to_string(),
            model_route_summary: "9router/cx-implementer".to_string(),
            model_route_selection_source: None,
            gateway_provenance: None,
            tool_call_sequence_digest: None,
            parent_capabilities: None,
        });

        let input = cccx_work_spec_spawn_input_items(Some(&runtime), Some("brief only")).unwrap();
        let input = input.unwrap();

        let [UserInput::Text { text, .. }] = input.as_slice() else {
            panic!("expected one text envelope input");
        };
        assert!(text.contains("\"schemaVersion\":\"cccx.spawn_input.v1\""));
        assert!(text.contains("\"work_spec_id\":\"ws-boundary-1\""));
        assert!(!text.contains("raw parent context"));
    }
}

/// A structured CCCX spawn resolves opaque 9router model IDs at the scheduler
/// boundary. Sending those IDs through Codex's upstream model manager first
/// would reject valid owner-catalog routes before CCCX can validate them.
pub(crate) fn cccx_owns_spawn_model_resolution(
    work_spec: Option<&cccx_protocol::WorkSpec>,
) -> bool {
    codex_features::cccx_mvp::surface_gate_enabled() && work_spec.is_some()
}

/// A WorkSpec is the cross-harness boundary.  A child governed by
/// `work_spec_only` must not also receive a fork of its parent's transcript,
/// including a partial fork: that would bypass the structured projection.
pub(crate) fn enforce_cccx_fork_policy(
    work_spec: Option<&cccx_protocol::WorkSpec>,
    includes_parent_context: bool,
) -> Result<(), FunctionCallError> {
    if !codex_features::cccx_mvp::surface_gate_enabled() {
        return Ok(());
    }
    let Some(work_spec) = work_spec else {
        return Ok(());
    };
    if matches!(
        work_spec.fork_policy,
        cccx_protocol::ForkPolicy::WorkSpecOnly
    ) && includes_parent_context
    {
        return Err(FunctionCallError::RespondToModel(
            "cccx work_spec_only workers cannot fork parent history; use fork_turns=none and pass bounded state through WorkSpec/Artifact".to_string(),
        ));
    }
    Ok(())
}

pub(crate) async fn apply_requested_spawn_agent_model_overrides(
    session: &Session,
    turn: &TurnContext,
    config: &mut Config,
    requested_model: Option<&str>,
    requested_reasoning_effort: Option<ReasoningEffort>,
) -> Result<(), FunctionCallError> {
    if requested_model.is_none() && requested_reasoning_effort.is_none() {
        return Ok(());
    }

    if let Some(requested_model) = requested_model {
        let available_models = session
            .services
            .models_manager
            .list_models(RefreshStrategy::Offline, config.http_client_factory())
            .await;
        let selected_model_name = find_spawn_agent_model_name(
            &available_models,
            requested_model,
            turn.multi_agent_version,
        )?;
        let selected_model_info = session
            .services
            .models_manager
            .get_model_info(&selected_model_name, &config.to_models_manager_config())
            .await;

        config.model = Some(selected_model_name.clone());
        if let Some(reasoning_effort) = requested_reasoning_effort {
            validate_spawn_agent_reasoning_effort(
                &selected_model_name,
                &selected_model_info.supported_reasoning_levels,
                &reasoning_effort,
            )?;
            config.model_reasoning_effort = Some(reasoning_effort);
        } else {
            config.model_reasoning_effort = selected_model_info.default_reasoning_level;
        }

        return Ok(());
    }

    if let Some(reasoning_effort) = requested_reasoning_effort {
        validate_spawn_agent_reasoning_effort(
            &turn.model_info.slug,
            &turn.model_info.supported_reasoning_levels,
            &reasoning_effort,
        )?;
        config.model_reasoning_effort = Some(reasoning_effort);
    }

    Ok(())
}

pub(crate) async fn apply_spawn_agent_service_tier(
    session: &Session,
    config: &mut Config,
    parent_service_tier: Option<&str>,
    requested_service_tier: Option<&str>,
) -> Result<(), FunctionCallError> {
    let candidate_service_tiers = [
        config.service_tier.clone(),
        requested_service_tier.map(str::to_string),
        parent_service_tier.map(str::to_string),
    ];
    if candidate_service_tiers.iter().all(Option::is_none) {
        config.service_tier = None;
        return Ok(());
    }

    let model = config.model.clone().ok_or_else(|| {
        FunctionCallError::RespondToModel(
            "spawn_agent could not resolve the child model for service tier validation".to_string(),
        )
    })?;
    let model_info = session
        .services
        .models_manager
        .get_model_info(model.as_str(), &config.to_models_manager_config())
        .await;

    if let Some(requested_service_tier) = requested_service_tier
        && !model_info.supports_service_tier(requested_service_tier)
    {
        let supported_service_tiers = if model_info.service_tiers.is_empty() {
            "none".to_string()
        } else {
            model_info
                .service_tiers
                .iter()
                .map(|tier| tier.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        };
        return Err(FunctionCallError::RespondToModel(format!(
            "Service tier `{requested_service_tier}` is not supported for model `{model}`. Supported service tiers: {supported_service_tiers}"
        )));
    }

    config.service_tier =
        candidate_service_tiers
            .into_iter()
            .flatten()
            .find(|candidate_service_tier| {
                model_info.supports_service_tier(candidate_service_tier.as_str())
            });
    Ok(())
}

fn find_spawn_agent_model_name(
    available_models: &[ModelPreset],
    requested_model: &str,
    multi_agent_version: MultiAgentVersion,
) -> Result<String, FunctionCallError> {
    available_models
        .iter()
        .find(|model| {
            model.model == requested_model
                && model_supports_multi_agent_backend(model, multi_agent_version)
        })
        .map(|model| model.model.clone())
        .ok_or_else(|| {
            let available = available_models
                .iter()
                .filter(|model| model.show_in_picker)
                .filter(|model| model_supports_multi_agent_backend(model, multi_agent_version))
                .take(MAX_SPAWN_AGENT_MODEL_OVERRIDES)
                .map(|model| model.model.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            FunctionCallError::RespondToModel(format!(
                "Unknown model `{requested_model}` for spawn_agent. Available models: {available}"
            ))
        })
}

fn validate_spawn_agent_reasoning_effort(
    model: &str,
    supported_reasoning_levels: &[ReasoningEffortPreset],
    requested_reasoning_effort: &ReasoningEffort,
) -> Result<(), FunctionCallError> {
    if supported_reasoning_levels
        .iter()
        .any(|preset| &preset.effort == requested_reasoning_effort)
    {
        return Ok(());
    }

    let supported = supported_reasoning_levels
        .iter()
        .map(|preset| preset.effort.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    Err(FunctionCallError::RespondToModel(format!(
        "Reasoning effort `{requested_reasoning_effort}` is not supported for model `{model}`. Supported reasoning efforts: {supported}"
    )))
}
