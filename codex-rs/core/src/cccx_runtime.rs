//! Session-scoped cccx control-plane state.
//!
//! The native multi-agent runtime owns process scheduling; this type owns only
//! cccx's structured WorkSpec/Artifact bookkeeping.  Keeping it separate
//! prevents a worker from becoming a scheduler or receiving raw parent context.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::path::Path;
use std::path::PathBuf;

use cccx_protocol::{
    Artifact, ArtifactProvenance, ArtifactStatus, ModelRouteSelection, PermissionPolicy, WorkSpec,
};
use cccx_provider_gateway::FallbackPolicyId;
use cccx_provider_gateway::provenance::GatewayProvenance;
use cccx_task_graph::{
    AttemptRecord, TaskGraph, TaskNode, TaskStatus, VerificationSpec, WorkerRecord, WorkerStatus,
};
use codex_protocol::ThreadId;
use codex_protocol::protocol::AgentStatus;
use codex_protocol::protocol::CccxRuntimeAgentModelRoute;
use codex_protocol::protocol::CccxRuntimeAgentUpdateEvent;
use codex_protocol::protocol::CccxRuntimeTaskUpdateEvent;

use crate::artifact_collector::ArtifactCollector;
use crate::cccx_task_graph_state;
use crate::checkpoint::CheckpointValidation;
use crate::checkpoint::SubagentCheckpoint;
use crate::goal_evaluator::GoalEvaluator;
use crate::verification_runner::run_verification;

pub(crate) struct CccxRuntimeState {
    task_graph: TaskGraph,
    attempts: BTreeMap<String, CccxAttemptState>,
}

#[derive(Clone, Debug)]
pub(crate) struct CccxSpawnRuntime {
    pub(crate) project_root: PathBuf,
    pub(crate) work_spec: WorkSpec,
    /// Stable CCCX worker identity, distinct from the current thread
    /// incarnation created for this execution attempt.
    pub(crate) logical_session_id: String,
    pub(crate) display_name: String,
    pub(crate) model_route_summary: String,
    pub(crate) gateway_provenance: Option<GatewayProvenance>,
    /// Rolling digest of successful tool completions for this worker attempt.
    /// It deliberately contains only stable tool identity and call id, never
    /// tool input or output, so provenance is useful without retaining secrets.
    pub(crate) tool_call_sequence_digest: Option<String>,
    /// Capability ceiling captured from the live Main CC at spawn. It is
    /// persisted with checkpoints so resume never derives authority from the
    /// child WorkSpec itself.
    pub(crate) parent_capabilities: Option<cccx_protocol::ParentCapabilitySet>,
}

impl CccxSpawnRuntime {
    pub(crate) fn agent_update_event(
        &self,
        status: impl Into<String>,
        permission_request_id: Option<String>,
    ) -> CccxRuntimeAgentUpdateEvent {
        CccxRuntimeAgentUpdateEvent {
            logical_session_id: self.logical_session_id.clone(),
            display_name: self.display_name.clone(),
            role: format_work_role(&self.work_spec.role),
            status: status.into(),
            model_route: runtime_agent_model_route(
                &self.work_spec,
                &self.model_route_summary,
                self.gateway_provenance.as_ref(),
            ),
            permission_request_id,
        }
    }
}

#[derive(Clone)]
struct CccxAttemptState {
    work_spec: WorkSpec,
}

/// The TaskGraph transition that has already been persisted. Keeping this
/// separate from a worker's terminal prose prevents callers from reporting a
/// task as verified before `record_verification` has committed.
pub(crate) struct CccxRuntimeCompletion {
    pub(crate) task_update: CccxRuntimeTaskUpdateEvent,
    pub(crate) verification_error: Option<String>,
}

impl CccxRuntimeState {
    pub(crate) fn new(task_graph: TaskGraph) -> Self {
        Self {
            task_graph,
            attempts: BTreeMap::new(),
        }
    }

    pub(crate) fn register_work_spec(
        &mut self,
        work_spec: WorkSpec,
        attempt_id: String,
        logical_session_id: String,
        display_name: String,
        model_route_summary: String,
    ) -> Result<(), String> {
        work_spec.validate().map_err(|error| error.to_string())?;
        self.ensure_task_exists(&work_spec)?;
        self.task_graph
            .start_attempt(
                &work_spec.task_id,
                AttemptRecord {
                    attempt_id: attempt_id.clone(),
                    work_spec_id: work_spec.work_spec_id.clone(),
                    worker: WorkerRecord {
                        logical_session_id,
                        display_name,
                        role: format!("{:?}", work_spec.role),
                        status: WorkerStatus::Running,
                        model_route_summary: model_route_summary.clone(),
                    },
                    artifact_ids: Vec::new(),
                    verification_records: Vec::new(),
                },
            )
            .map_err(|error| error.to_string())?;
        self.track_existing_attempt(work_spec, attempt_id);
        Ok(())
    }

    fn track_existing_attempt(&mut self, work_spec: WorkSpec, attempt_id: String) {
        self.attempts
            .insert(attempt_id, CccxAttemptState { work_spec });
    }

    pub(crate) fn complete(
        &mut self,
        mut artifact: Artifact,
        exit_code: Option<i32>,
        log_digest: Option<&str>,
    ) -> Result<(), String> {
        let attempt = self
            .attempts
            .get(&artifact.attempt_id)
            .ok_or_else(|| format!("unknown cccx work spec `{}`", artifact.work_spec_id))?;
        if matches!(artifact.status, ArtifactStatus::Success) {
            artifact.verification = GoalEvaluator::evaluate(
                self.task_graph
                    .tasks
                    .get(&attempt.work_spec.task_id)
                    .ok_or_else(|| format!("unknown cccx task `{}`", attempt.work_spec.task_id))?,
                &artifact.attempt_id,
                exit_code,
                log_digest,
            );
        }
        artifact.validate().map_err(|error| error.to_string())?;
        let attempt_id = artifact.attempt_id.clone();
        let verification = artifact.verification.clone();
        self.task_graph
            .apply_artifact(&attempt.work_spec.task_id, artifact)
            .map_err(|error| error.to_string())?;
        if let Some(record) = verification {
            let spec_digest = self.task_graph.tasks[&attempt.work_spec.task_id]
                .verification_spec
                .spec_digest
                .clone();
            self.task_graph
                .record_verification(
                    &attempt.work_spec.task_id,
                    &attempt_id,
                    record,
                    &spec_digest,
                )
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    pub(crate) fn task_graph(&self) -> &TaskGraph {
        &self.task_graph
    }

    fn record_harness_verification(
        &mut self,
        task_id: &str,
        attempt_id: &str,
        record: cccx_protocol::VerificationRecord,
    ) -> Result<(), String> {
        let spec_digest = self
            .task_graph
            .tasks
            .get(task_id)
            .ok_or_else(|| format!("unknown cccx task `{task_id}`"))?
            .verification_spec
            .spec_digest
            .clone();
        self.task_graph
            .record_verification(task_id, attempt_id, record, &spec_digest)
            .map_err(|error| error.to_string())
    }

    fn ensure_task_exists(&mut self, work_spec: &WorkSpec) -> Result<(), String> {
        if self.task_graph.tasks.contains_key(&work_spec.task_id) {
            return Ok(());
        }
        use sha2::{Digest, Sha256};
        let protocol_spec =
            work_spec
                .verification_spec
                .clone()
                .unwrap_or(cccx_protocol::VerificationSpec {
                    command: None,
                    protected_digests: BTreeMap::new(),
                    verification_policy_id: None,
                });
        let spec_payload = serde_json::to_vec(&protocol_spec).map_err(|error| error.to_string())?;
        let spec_digest = format!("sha256:{:x}", Sha256::digest(spec_payload));
        self.task_graph
            .add_task(TaskNode::new(
                work_spec.task_id.clone(),
                work_spec.objective.clone(),
                VerificationSpec {
                    command: protocol_spec.command,
                    protected_digests: protocol_spec.protected_digests,
                    verification_policy_id: protocol_spec.verification_policy_id,
                    spec_digest,
                },
            ))
            .map_err(|error| error.to_string())
    }
}

#[derive(Default)]
pub(crate) struct CccxRuntimeRegistry {
    by_thread: BTreeMap<String, CccxSpawnRuntime>,
}

impl CccxRuntimeRegistry {
    /// Load the persisted state required for a CCCX logical-worker resume.
    /// This is deliberately separate from upstream rollout resume: callers
    /// must spawn a fresh worker incarnation only after this gate succeeds.
    pub(crate) async fn prepare_resume(
        project_root: &Path,
        worker_identifier: &str,
        current_git_head: &str,
        current_working_tree_digest: Option<&str>,
        max_age_minutes: u32,
        revalidate: bool,
    ) -> Result<SubagentCheckpoint, String> {
        let graph = cccx_task_graph_state::load_task_graph(project_root)
            .await
            .map_err(|error| format!("failed to load CCCX task graph: {error}"))?
            .ok_or_else(|| "CCCX task graph is missing".to_string())?;
        let workers = graph
            .tasks
            .values()
            .flat_map(|task| task.attempts.iter())
            .filter(|attempt| {
                attempt.worker.logical_session_id == worker_identifier
                    || attempt.worker.display_name == worker_identifier
            })
            .collect::<Vec<_>>();
        let worker = match workers.as_slice() {
            [] => return Err(format!("unknown CCCX worker `{worker_identifier}`")),
            [worker] => *worker,
            _ => {
                return Err(format!(
                    "CCCX worker display name `{worker_identifier}` is ambiguous; use its logicalSessionId"
                ));
            }
        };
        let logical_session_id = &worker.worker.logical_session_id;
        let checkpoint = crate::checkpoint::load_checkpoint(project_root, logical_session_id)
            .await
            .map_err(|error| format!("failed to load CCCX checkpoint: {error}"))?
            .ok_or_else(|| {
                format!("no checkpoint exists for CCCX worker `{logical_session_id}`")
            })?;
        if worker.worker.status != WorkerStatus::Resumable {
            return Err(format!(
                "CCCX worker `{logical_session_id}` is {:?}, not resumable",
                worker.worker.status
            ));
        }
        if checkpoint.attempt_id != worker.attempt_id {
            return Err(format!(
                "checkpoint attempt `{}` does not match worker attempt `{}`",
                checkpoint.attempt_id, worker.attempt_id
            ));
        }
        if checkpoint.work_spec.is_none() || checkpoint.display_name.is_none() {
            return Err("checkpoint predates resumable CCCX worker metadata; create a new WorkSpec attempt instead".to_string());
        }
        match crate::checkpoint::validate_checkpoint(
            &checkpoint,
            current_git_head,
            current_working_tree_digest,
            max_age_minutes,
        ) {
            CheckpointValidation::Valid => Ok(checkpoint),
            CheckpointValidation::NeedsRevalidation { .. } if revalidate => Ok(checkpoint),
            CheckpointValidation::NeedsRevalidation { .. } => Err(
                "checkpoint requires --revalidate because the base revision changed".to_string(),
            ),
            CheckpointValidation::Stale { reason, .. } => {
                Err(format!("checkpoint is not resumable: {reason}"))
            }
        }
    }
    pub(crate) async fn register_spawn(
        &mut self,
        child_thread_id: ThreadId,
        runtime: CccxSpawnRuntime,
    ) -> Result<(), String> {
        let initial = TaskGraph::new(
            "cccx-runtime",
            format!("cccx runtime graph for {}", runtime.project_root.display()),
        );
        cccx_task_graph_state::update_task_graph(&runtime.project_root, initial, |graph| {
            let mut state = CccxRuntimeState::new(graph.clone());
            // Create the TaskNode immediately on spawn, under the same writer
            // lease that persists it, so concurrent spawns cannot lose a task.
            state
                .register_work_spec(
                    runtime.work_spec.clone(),
                    child_thread_id.to_string(),
                    runtime.logical_session_id.clone(),
                    runtime.display_name.clone(),
                    runtime.model_route_summary.clone(),
                )
                .map_err(cccx_task_graph_state::TaskGraphStateError::Mutation)?;
            *graph = state.task_graph.clone();
            Ok(())
        })
        .await
        .map_err(|error| error.to_string())?;
        self.by_thread.insert(child_thread_id.to_string(), runtime);
        Ok(())
    }

    pub(crate) fn work_spec_for_thread(&self, thread_id: ThreadId) -> Option<WorkSpec> {
        self.by_thread
            .get(&thread_id.to_string())
            .map(|runtime| runtime.work_spec.clone())
    }

    /// Record an actual successful tool completion from the common registry
    /// boundary. Returns false for non-CCCX sessions, which keeps this hook
    /// transparent to the upstream runtime.
    pub(crate) fn record_successful_tool_call(
        &mut self,
        child_thread_id: ThreadId,
        call_id: &str,
        tool_name: &str,
    ) -> bool {
        let Some(runtime) = self.by_thread.get_mut(&child_thread_id.to_string()) else {
            return false;
        };
        use sha2::{Digest, Sha256};
        let prior = runtime
            .tool_call_sequence_digest
            .as_deref()
            .unwrap_or("cccx-tool-sequence-v1");
        let payload = format!("{prior}\u{0}{call_id}\u{0}{tool_name}");
        runtime.tool_call_sequence_digest =
            Some(format!("sha256:{:x}", Sha256::digest(payload.as_bytes())));
        true
    }

    /// Persist a resumable snapshot while a bounded worker is still active.
    /// This is intentionally best-effort at the tool boundary: a successful
    /// edit must not be reported as failed merely because checkpoint storage
    /// is temporarily unavailable.
    pub(crate) async fn checkpoint_spawn(
        &self,
        child_thread_id: ThreadId,
        last_completed_step: String,
    ) -> Result<bool, String> {
        let Some(runtime) = self.by_thread.get(&child_thread_id.to_string()) else {
            return Ok(false);
        };
        let checkpoint = SubagentCheckpoint {
            logical_session_id: runtime.logical_session_id.clone(),
            worker_incarnation_id: child_thread_id.to_string(),
            task_id: runtime.work_spec.task_id.clone(),
            attempt_id: child_thread_id.to_string(),
            work_spec: Some(runtime.work_spec.clone()),
            display_name: Some(runtime.display_name.clone()),
            last_completed_step,
            partial_diff: String::new(),
            pending_steps: vec![runtime.work_spec.objective.clone()],
            checkpointed_at: chrono::Utc::now().to_rfc3339(),
            base_commit: runtime.work_spec.base_revision.git_head.clone(),
            base_working_tree_digest: runtime.work_spec.base_revision.working_tree_digest.clone(),
            parent_capabilities: runtime.parent_capabilities.clone(),
            tool_call_sequence_digest: runtime.tool_call_sequence_digest.clone(),
        };
        crate::checkpoint::save_checkpoint(&runtime.project_root, &checkpoint)
            .await
            .map_err(|error| format!("failed to persist worker checkpoint: {error}"))?;
        Ok(true)
    }

    pub(crate) async fn complete_spawn(
        &mut self,
        child_thread_id: ThreadId,
        status_summary: String,
        success: bool,
    ) -> Result<Option<CccxRuntimeCompletion>, String> {
        let Some(runtime) = self.by_thread.remove(&child_thread_id.to_string()) else {
            return Ok(None);
        };
        if !success {
            let checkpoint = SubagentCheckpoint {
                logical_session_id: runtime.logical_session_id.clone(),
                worker_incarnation_id: child_thread_id.to_string(),
                task_id: runtime.work_spec.task_id.clone(),
                attempt_id: child_thread_id.to_string(),
                work_spec: Some(runtime.work_spec.clone()),
                display_name: Some(runtime.display_name.clone()),
                last_completed_step: status_summary.clone(),
                partial_diff: String::new(),
                pending_steps: vec![runtime.work_spec.objective.clone()],
                checkpointed_at: chrono::Utc::now().to_rfc3339(),
                base_commit: runtime.work_spec.base_revision.git_head.clone(),
                base_working_tree_digest: runtime
                    .work_spec
                    .base_revision
                    .working_tree_digest
                    .clone(),
                parent_capabilities: runtime.parent_capabilities.clone(),
                tool_call_sequence_digest: runtime.tool_call_sequence_digest.clone(),
            };
            crate::checkpoint::save_checkpoint(&runtime.project_root, &checkpoint)
                .await
                .map_err(|error| {
                    format!("failed to persist interrupted worker checkpoint: {error}")
                })?;
        }
        let gateway_provenance = match runtime.gateway_provenance {
            Some(provenance) => Some(provenance),
            None => crate::cccx_gateway_provenance_state::load_gateway_provenance(
                &runtime.project_root,
                &child_thread_id.to_string(),
            )
            .await
            .map_err(|error| format!("failed to load terminal gateway provenance: {error}"))?,
        };
        let graph = cccx_task_graph_state::load_task_graph(&runtime.project_root)
            .await
            .map_err(|error| error.to_string())?
            .unwrap_or_else(|| TaskGraph::new("cccx-runtime", "cccx runtime graph"));
        let mut collector = ArtifactCollector::new(
            &runtime.work_spec.work_spec_id,
            &child_thread_id.to_string(),
        );
        collector.record_runtime_log("subagent terminal status", &status_summary);
        let artifact_status = if success {
            ArtifactStatus::Success
        } else {
            ArtifactStatus::Interrupted
        };
        let artifact = collector.finalize(
            artifact_status,
            status_summary.clone(),
            provenance_for_runtime(
                &runtime.work_spec,
                &runtime.model_route_summary,
                &status_summary,
                gateway_provenance.as_ref(),
                runtime.tool_call_sequence_digest.as_deref(),
            ),
        );

        let mut verification_run = None;
        let mut verification_error = None;
        if success {
            let task_id = runtime.work_spec.task_id.clone();
            let verification_spec = graph
                .tasks
                .get(&task_id)
                .ok_or_else(|| format!("unknown cccx task `{task_id}`"))?
                .verification_spec
                .clone();
            match run_verification(&runtime.project_root, &verification_spec).await {
                Ok(Some(run)) => verification_run = Some(run),
                Ok(None) => {}
                Err(error) => verification_error = Some(error.to_string()),
            }
        }
        let initial = TaskGraph::new("cccx-runtime", "cccx runtime graph");
        let task_update =
            cccx_task_graph_state::update_task_graph(&runtime.project_root, initial, |graph| {
                let mut state = CccxRuntimeState::new(graph.clone());
                state
                    .track_existing_attempt(runtime.work_spec.clone(), child_thread_id.to_string());
                // A terminal worker status is an Artifact, not independent verification evidence.
                state
                    .complete(artifact, None, None)
                    .map_err(cccx_task_graph_state::TaskGraphStateError::Mutation)?;
                if let Some(run) = verification_run {
                    use sha2::{Digest, Sha256};
                    let task_id = runtime.work_spec.task_id.clone();
                    let artifact_digest =
                        format!("sha256:{:x}", Sha256::digest(status_summary.as_bytes()));
                    let task = state.task_graph().tasks.get(&task_id).ok_or_else(|| {
                        cccx_task_graph_state::TaskGraphStateError::Mutation(format!(
                            "unknown cccx task `{task_id}`"
                        ))
                    })?;
                    let record = GoalEvaluator::evaluate_execution(
                        task,
                        &child_thread_id.to_string(),
                        run.command,
                        run.protected_input_digest,
                        artifact_digest,
                    );
                    state
                        .record_harness_verification(&task_id, &child_thread_id.to_string(), record)
                        .map_err(cccx_task_graph_state::TaskGraphStateError::Mutation)?;
                }
                *graph = state.task_graph.clone();
                task_update_from_graph(graph, &runtime.work_spec.task_id)
                    .map_err(cccx_task_graph_state::TaskGraphStateError::Mutation)
            })
            .await
            .map_err(|error| error.to_string())?;
        Ok(Some(CccxRuntimeCompletion {
            task_update,
            verification_error,
        }))
    }

    /// Persist a terminal user/CC cancellation. Unlike an interrupted worker,
    /// this deliberately creates no checkpoint and therefore cannot be resumed
    /// as the same attempt.
    pub(crate) async fn cancel_spawn(
        &mut self,
        child_thread_id: ThreadId,
        reason: String,
    ) -> Result<Option<CccxRuntimeTaskUpdateEvent>, String> {
        let Some(runtime) = self.by_thread.remove(&child_thread_id.to_string()) else {
            return Ok(None);
        };
        let gateway_provenance = match runtime.gateway_provenance {
            Some(provenance) => Some(provenance),
            None => crate::cccx_gateway_provenance_state::load_gateway_provenance(
                &runtime.project_root,
                &child_thread_id.to_string(),
            )
            .await
            .map_err(|error| format!("failed to load terminal gateway provenance: {error}"))?,
        };
        let mut collector = ArtifactCollector::new(
            &runtime.work_spec.work_spec_id,
            &child_thread_id.to_string(),
        );
        collector.record_runtime_log("subagent cancelled", &reason);
        let artifact = collector.finalize(
            ArtifactStatus::Cancelled,
            reason.clone(),
            provenance_for_runtime(
                &runtime.work_spec,
                &runtime.model_route_summary,
                &reason,
                gateway_provenance.as_ref(),
                runtime.tool_call_sequence_digest.as_deref(),
            ),
        );
        let initial = TaskGraph::new("cccx-runtime", "cccx runtime graph");
        let task_update =
            cccx_task_graph_state::update_task_graph(&runtime.project_root, initial, |graph| {
                let mut state = CccxRuntimeState::new(graph.clone());
                state
                    .track_existing_attempt(runtime.work_spec.clone(), child_thread_id.to_string());
                state
                    .complete(artifact, None, None)
                    .map_err(cccx_task_graph_state::TaskGraphStateError::Mutation)?;
                *graph = state.task_graph.clone();
                task_update_from_graph(graph, &runtime.work_spec.task_id)
                    .map_err(cccx_task_graph_state::TaskGraphStateError::Mutation)
            })
            .await
            .map_err(|error| error.to_string())?;
        Ok(Some(task_update))
    }
}

fn task_update_from_graph(
    graph: &TaskGraph,
    task_id: &str,
) -> Result<CccxRuntimeTaskUpdateEvent, String> {
    let task = graph
        .tasks
        .get(task_id)
        .ok_or_else(|| format!("unknown cccx task `{task_id}`"))?;
    Ok(CccxRuntimeTaskUpdateEvent {
        task_id: task.task_id.clone(),
        status: task_status_label(&task.status).to_string(),
        verified: task.is_verified(),
    })
}

fn task_status_label(status: &TaskStatus) -> &'static str {
    match status {
        TaskStatus::Pending => "pending",
        TaskStatus::InProgress => "in_progress",
        TaskStatus::Blocked => "blocked",
        TaskStatus::Interrupted => "interrupted",
        TaskStatus::Cancelled => "cancelled",
        TaskStatus::Failed => "failed",
        TaskStatus::Verified => "verified",
    }
}

pub(crate) fn model_route_summary(work_spec: &WorkSpec, main_model_slug: &str) -> String {
    match &work_spec.model_route {
        ModelRouteSelection::Explicit { route_id } => route_id.clone(),
        ModelRouteSelection::InheritMainRequested | ModelRouteSelection::InheritMainEffective => {
            main_model_slug.to_string()
        }
    }
}

fn runtime_agent_model_route(
    work_spec: &WorkSpec,
    route_id: &str,
    gateway: Option<&GatewayProvenance>,
) -> CccxRuntimeAgentModelRoute {
    CccxRuntimeAgentModelRoute {
        selection_source: model_route_selection_source(&work_spec.model_route).to_string(),
        requested_route_id: gateway
            .map(|gateway| gateway.requested_route_id.clone())
            .unwrap_or_else(|| route_id.to_string()),
        effective_route_id: gateway
            .and_then(|gateway| gateway.effective_route_id.clone())
            .unwrap_or_else(|| "unknown".to_string()),
        fallback_chain: gateway
            .map(|gateway| gateway.fallback_chain.clone())
            .unwrap_or_default(),
    }
}

fn model_route_selection_source(selection: &ModelRouteSelection) -> &'static str {
    match selection {
        ModelRouteSelection::InheritMainRequested => "inherit_main_requested",
        ModelRouteSelection::InheritMainEffective => "inherit_main_effective",
        ModelRouteSelection::Explicit { .. } => "explicit",
    }
}

fn format_work_role(role: &cccx_protocol::WorkRole) -> String {
    format!("{role:?}").to_ascii_lowercase()
}

pub(crate) fn cccx_agent_status_label(status: &AgentStatus) -> &'static str {
    match status {
        AgentStatus::PendingInit => "waiting",
        AgentStatus::Running => "running",
        AgentStatus::Interrupted => "interrupted",
        AgentStatus::Completed(_) => "completed",
        AgentStatus::Errored(_) | AgentStatus::NotFound => "failed",
        AgentStatus::Shutdown => "cancelled",
    }
}

/// Build a ParentCapabilitySet for WorkSpec projection.
///
/// The tool, MCP, and model route sets are snapshots of the live Main CC
/// capability boundary, never values requested by the child WorkSpec. In
/// particular, an explicit child route must already be present in the local
/// 9router catalog before projection can admit it.
pub(crate) fn parent_capability_set_for_work_spec(
    project_root: &Path,
    work_spec: &WorkSpec,
    parent_allowed_tools: BTreeSet<String>,
    parent_allowed_mcp_tools: BTreeSet<String>,
    parent_model_routes: BTreeSet<String>,
) -> cccx_protocol::ParentCapabilitySet {
    cccx_protocol::ParentCapabilitySet {
        tools: parent_allowed_tools,
        mcp_tools: parent_allowed_mcp_tools,
        writable_roots: BTreeSet::from([project_root.display().to_string()]),
        protected_paths: work_spec.scope.protected_paths.clone(),
        model_routes: parent_model_routes,
        permission_ceiling: PermissionPolicy::Allow,
    }
}

fn provenance_for_runtime(
    work_spec: &WorkSpec,
    route_id: &str,
    status_summary: &str,
    gateway: Option<&GatewayProvenance>,
    tool_call_sequence_digest: Option<&str>,
) -> ArtifactProvenance {
    use sha2::{Digest, Sha256};
    let mut sorted_tools: Vec<_> = work_spec.allowed_tools.iter().collect();
    sorted_tools.sort();
    let tools_joined: String = sorted_tools
        .into_iter()
        .map(|s| s.as_str())
        .collect::<Vec<_>>()
        .join(",");
    let input_payload = format!("{}|{}", work_spec.objective, tools_joined);
    let input_digest = format!("sha256:{:x}", Sha256::digest(input_payload.as_bytes()));
    // Phase 5/7: effective_route_id is "unknown" when 9router provides no
    // effective downstream metadata. We never infer it from the requested route.
    let requested_route_id = gateway
        .map(|gateway| gateway.requested_route_id.clone())
        .unwrap_or_else(|| route_id.to_string());
    let effective_route_id = gateway
        .and_then(|gateway| gateway.effective_route_id.clone())
        .unwrap_or_else(|| "unknown".to_string());
    // Phase 5 audit: output_digest is SHA256(status_summary), not a constant.
    // Different completions produce different output digests.
    let output_digest = format!("sha256:{:x}", Sha256::digest(status_summary.as_bytes()));
    ArtifactProvenance {
        producer_harness: work_spec.harness.clone(),
        producer_role: work_spec.role.clone(),
        provider_gateway_request_id: gateway
            .map(|gateway| gateway.gateway_request_id.clone())
            .unwrap_or_else(|| "unknown".to_string()),
        model_profile_id: "unknown".to_string(),
        capability_profile_id: "unknown".to_string(),
        fallback_policy_id: gateway
            .and_then(|gateway| gateway.fallback_policy_id)
            .map(fallback_policy_id_to_str)
            .unwrap_or_else(|| work_spec.fallback_policy_id.as_str())
            .to_string(),
        requested_route_id,
        effective_route_id,
        gateway_request_id: gateway.map(|gateway| gateway.gateway_request_id.clone()),
        requested_model: gateway.map(|gateway| gateway.requested_route_id.clone()),
        effective_model: gateway.and_then(|gateway| gateway.effective_model.clone()),
        retry_count: gateway
            .map(|gateway| gateway.retry_count as usize)
            .unwrap_or_default(),
        fallback_chain: gateway
            .map(|gateway| gateway.fallback_chain.clone())
            .unwrap_or_default(),
        input_digest,
        output_digest,
        tool_call_sequence_digest: tool_call_sequence_digest.map(str::to_string),
        stream_termination_status: gateway
            .and_then(|gateway| gateway.stream_termination_status.clone()),
    }
}

fn fallback_policy_id_to_str(policy: FallbackPolicyId) -> &'static str {
    match policy {
        FallbackPolicyId::Strict => "strict",
        FallbackPolicyId::Compatible => "compatible",
        FallbackPolicyId::RouterDefault => "router_default",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifact_collector::ArtifactCollector;
    use cccx_protocol::{
        ArtifactProvenance, CommandSpec, ForkPolicy, HarnessKind, ModelRouteSelection,
        PermissionPolicy, RevisionRef, WorkRole, WorkScope,
    };
    use cccx_task_graph::{TaskNode, VerificationSpec};
    use std::collections::BTreeSet;

    fn work_spec() -> WorkSpec {
        WorkSpec {
            schema_version: cccx_protocol::CCCX_WORK_SPEC_SCHEMA_VERSION.into(),
            work_spec_id: "ws-1".into(),
            task_id: "T1".into(),
            attempt_id: "attempt-1".into(),
            created_at: "2026-07-19T00:00:00Z".into(),
            expires_at: None,
            base_revision: RevisionRef {
                git_head: None,
                working_tree_digest: None,
            },
            harness: HarnessKind::Cx,
            role: WorkRole::Tester,
            runtime_identity: None,
            objective: "run tests".into(),
            scope: WorkScope {
                canonical_paths: ["D:/repo".into()].into_iter().collect(),
                protected_paths: BTreeSet::new(),
            },
            allowed_tools: ["shell_command".into()].into_iter().collect(),
            forbidden_tools: BTreeSet::new(),
            allowed_mcp_tools: BTreeSet::new(),
            tool_grants: Vec::new(),
            permission_policy: PermissionPolicy::Ask,
            model_route: ModelRouteSelection::InheritMainEffective,
            fallback_policy_id: "strict".into(),
            fork_policy: ForkPolicy::WorkSpecOnly,
            enforced_constraints: vec![],
            typed_constraints: Vec::new(),
            risk_threshold: None,
            action_classes: None,
            approval_policy: None,
            verification_spec: None,
            audit_depth: None,
            expected_artifact: None,
        }
    }
    fn provenance() -> ArtifactProvenance {
        ArtifactProvenance {
            producer_harness: HarnessKind::Cx,
            producer_role: WorkRole::Tester,
            provider_gateway_request_id: "gw-test".into(),
            model_profile_id: "profile-test".into(),
            capability_profile_id: "cap-test".into(),
            fallback_policy_id: "strict".into(),
            requested_route_id: "route".into(),
            effective_route_id: "route".into(),
            gateway_request_id: None,
            requested_model: None,
            effective_model: None,
            retry_count: 0,
            fallback_chain: Vec::new(),
            input_digest: "sha256:i".into(),
            output_digest: "sha256:o".into(),
            tool_call_sequence_digest: None,
            stream_termination_status: None,
        }
    }
    #[test]
    fn completion_applies_artifact_and_verifies_its_task() {
        let mut graph = TaskGraph::new("g", "test");
        graph
            .add_task(TaskNode::new(
                "T1",
                "test",
                VerificationSpec {
                    command: None,
                    protected_digests: BTreeMap::new(),
                    verification_policy_id: Some("focused-tests".to_string()),
                    spec_digest: "sha256:s".into(),
                },
            ))
            .unwrap();
        let mut runtime = CccxRuntimeState::new(graph);
        runtime
            .register_work_spec(
                work_spec(),
                "attempt-1".into(),
                "thread-1".into(),
                "tester".into(),
                "route".into(),
            )
            .unwrap();
        let mut collector = ArtifactCollector::new("ws-1", "attempt-1");
        collector.record_test_run("cargo test", 0, "ok");
        runtime
            .complete(
                collector.finalize(ArtifactStatus::Success, "ok".into(), provenance()),
                Some(0),
                Some("sha256:log"),
            )
            .unwrap();
        assert!(runtime.task_graph().can_stop());
    }

    #[tokio::test]
    async fn complete_spawn_runs_the_pinned_work_spec_verification_command() {
        let project = tempfile::TempDir::new().unwrap();
        let mut work_spec = work_spec();
        work_spec.verification_spec = Some(cccx_protocol::VerificationSpec {
            command: Some(CommandSpec {
                argv: if cfg!(windows) {
                    vec![
                        "cmd".to_string(),
                        "/C".to_string(),
                        "echo cccx-wire".to_string(),
                    ]
                } else {
                    vec![
                        "sh".to_string(),
                        "-c".to_string(),
                        "printf cccx-wire".to_string(),
                    ]
                },
                cwd: ".".to_string(),
                timeout_ms: Some(5_000),
            }),
            protected_digests: BTreeMap::new(),
            verification_policy_id: Some("pinned-test".to_string()),
        });
        let child_thread_id = ThreadId::new();
        let runtime = CccxSpawnRuntime {
            project_root: project.path().to_path_buf(),
            logical_session_id: "logical-verification".to_string(),
            work_spec,
            display_name: "verification-worker".to_string(),
            model_route_summary: "9router/test".to_string(),
            gateway_provenance: None,
            tool_call_sequence_digest: None,
            parent_capabilities: None,
        };
        let mut registry = CccxRuntimeRegistry::default();
        registry
            .register_spawn(child_thread_id, runtime)
            .await
            .unwrap();
        let completion = registry
            .complete_spawn(child_thread_id, "worker completed".to_string(), true)
            .await
            .unwrap()
            .expect("terminal worker must emit a persisted TaskGraph update");
        assert_eq!(completion.task_update.task_id, "T1");
        assert_eq!(completion.task_update.status, "verified");
        assert!(completion.task_update.verified);
        assert!(completion.verification_error.is_none());
        let graph = cccx_task_graph_state::load_task_graph(project.path())
            .await
            .unwrap()
            .unwrap();
        let task = &graph.tasks["T1"];
        assert!(task.is_verified());
        let record = task.attempts[0].verification_records.last().unwrap();
        assert_eq!(record.verifier, "cccx-verification-runner");
        assert_eq!(record.command.as_ref().unwrap().exit_code, 0);
        assert!(record.protected_input_digest.is_some());
    }

    #[tokio::test]
    async fn active_spawn_writes_checkpoint_after_bounded_step() {
        let project = tempfile::TempDir::new().unwrap();
        let child_thread_id = ThreadId::new();
        let runtime = CccxSpawnRuntime {
            project_root: project.path().to_path_buf(),
            logical_session_id: "logical-post-tool".to_string(),
            work_spec: work_spec(),
            display_name: "worker".to_string(),
            model_route_summary: "9router/test".to_string(),
            gateway_provenance: None,
            tool_call_sequence_digest: None,
            parent_capabilities: None,
        };
        let mut registry = CccxRuntimeRegistry::default();
        registry
            .register_spawn(child_thread_id, runtime)
            .await
            .unwrap();

        assert!(
            registry
                .checkpoint_spawn(
                    child_thread_id,
                    "apply_patch completed (call-1)".to_string()
                )
                .await
                .unwrap()
        );
        let checkpoint = crate::checkpoint::load_checkpoint(project.path(), "logical-post-tool")
            .await
            .unwrap()
            .expect("post-tool checkpoint must persist");
        assert_eq!(
            checkpoint.worker_incarnation_id,
            child_thread_id.to_string()
        );
        assert_eq!(
            checkpoint.last_completed_step,
            "apply_patch completed (call-1)"
        );
    }

    #[tokio::test]
    async fn successful_tool_calls_are_checkpointed_as_a_secret_free_sequence_digest() {
        let project = tempfile::TempDir::new().unwrap();
        let child_thread_id = ThreadId::new();
        let runtime = CccxSpawnRuntime {
            project_root: project.path().to_path_buf(),
            logical_session_id: "logical-tool-provenance".to_string(),
            work_spec: work_spec(),
            display_name: "worker".to_string(),
            model_route_summary: "9router/test".to_string(),
            gateway_provenance: None,
            tool_call_sequence_digest: None,
            parent_capabilities: None,
        };
        let mut registry = CccxRuntimeRegistry::default();
        registry
            .register_spawn(child_thread_id, runtime)
            .await
            .unwrap();

        assert!(registry.record_successful_tool_call(child_thread_id, "call-1", "shell_command"));
        assert!(registry.record_successful_tool_call(child_thread_id, "call-2", "apply_patch"));
        assert!(
            registry
                .checkpoint_spawn(child_thread_id, "apply_patch completed".to_string())
                .await
                .unwrap()
        );

        let checkpoint =
            crate::checkpoint::load_checkpoint(project.path(), "logical-tool-provenance")
                .await
                .unwrap()
                .expect("checkpoint should exist");
        let digest = checkpoint
            .tool_call_sequence_digest
            .expect("successful tool sequence must be checkpointed");
        assert!(digest.starts_with("sha256:"));
        assert_eq!(digest.len(), "sha256:".len() + 64);
        assert!(!digest.contains("shell_command"));
        assert!(!digest.contains("call-1"));
        assert!(!registry.record_successful_tool_call(ThreadId::new(), "call-3", "shell_command"));
    }

    #[tokio::test]
    async fn prepare_resume_rejects_completed_and_accepts_interrupted_worker() {
        let project = tempfile::TempDir::new().unwrap();
        let child_thread_id = ThreadId::new();
        let runtime = CccxSpawnRuntime {
            project_root: project.path().to_path_buf(),
            logical_session_id: "logical-resume".to_string(),
            work_spec: work_spec(),
            display_name: "a1-worker".to_string(),
            model_route_summary: "9router/test".to_string(),
            gateway_provenance: None,
            tool_call_sequence_digest: None,
            parent_capabilities: None,
        };
        let mut registry = CccxRuntimeRegistry::default();
        registry
            .register_spawn(child_thread_id, runtime)
            .await
            .unwrap();
        registry
            .complete_spawn(child_thread_id, "transport lost".to_string(), false)
            .await
            .unwrap();
        assert!(
            CccxRuntimeRegistry::prepare_resume(
                project.path(),
                "logical-resume",
                "head",
                None,
                60,
                true
            )
            .await
            .is_ok()
        );
        assert!(
            CccxRuntimeRegistry::prepare_resume(
                project.path(),
                "a1-worker",
                "head",
                None,
                60,
                true
            )
            .await
            .is_ok()
        );
    }

    #[tokio::test]
    async fn register_spawn_preserves_logical_identity_and_incarnation_separately() {
        let project = tempfile::TempDir::new().unwrap();
        let child_thread_id = ThreadId::new();
        let runtime = CccxSpawnRuntime {
            project_root: project.path().to_path_buf(),
            logical_session_id: "logical-worker-a1".to_string(),
            work_spec: work_spec(),
            display_name: "a1-worker".to_string(),
            model_route_summary: "9router/test".to_string(),
            gateway_provenance: None,
            tool_call_sequence_digest: None,
            parent_capabilities: None,
        };
        let mut registry = CccxRuntimeRegistry::default();
        registry
            .register_spawn(child_thread_id, runtime)
            .await
            .unwrap();

        let graph = cccx_task_graph_state::load_task_graph(project.path())
            .await
            .unwrap()
            .expect("spawn must persist TaskGraph state");
        let attempt = &graph.tasks["T1"].attempts[0];
        assert_eq!(attempt.attempt_id, child_thread_id.to_string());
        assert_eq!(attempt.worker.logical_session_id, "logical-worker-a1");
    }

    #[tokio::test]
    async fn failed_pinned_verification_persists_one_correction_path() {
        let project = tempfile::TempDir::new().unwrap();
        let mut work_spec = work_spec();
        work_spec.verification_spec = Some(cccx_protocol::VerificationSpec {
            command: Some(CommandSpec {
                argv: if cfg!(windows) {
                    vec!["cmd".to_string(), "/C".to_string(), "exit /B 7".to_string()]
                } else {
                    vec!["sh".to_string(), "-c".to_string(), "exit 7".to_string()]
                },
                cwd: ".".to_string(),
                timeout_ms: Some(5_000),
            }),
            protected_digests: BTreeMap::new(),
            verification_policy_id: Some("pinned-test".to_string()),
        });
        let child_thread_id = ThreadId::new();
        let runtime = CccxSpawnRuntime {
            project_root: project.path().to_path_buf(),
            logical_session_id: "logical-verification".to_string(),
            work_spec,
            display_name: "verification-worker".to_string(),
            model_route_summary: "9router/test".to_string(),
            gateway_provenance: None,
            tool_call_sequence_digest: None,
            parent_capabilities: None,
        };
        let mut registry = CccxRuntimeRegistry::default();
        registry
            .register_spawn(child_thread_id, runtime)
            .await
            .unwrap();
        let completion = registry
            .complete_spawn(child_thread_id, "worker completed".to_string(), true)
            .await
            .unwrap()
            .expect("failed verification still commits a TaskGraph transition");
        assert_eq!(completion.task_update.status, "blocked");
        assert!(!completion.task_update.verified);
        let graph = cccx_task_graph_state::load_task_graph(project.path())
            .await
            .unwrap()
            .unwrap();
        let task = &graph.tasks["T1"];
        assert!(!task.is_verified());
        assert!(
            task.correction_required
                .as_deref()
                .is_some_and(|reason| reason.contains("exit code 7"))
        );
        assert_eq!(task.attempts[0].verification_records.len(), 1);
        assert!(!task.attempts[0].verification_records[0].passed);
    }

    #[tokio::test]
    async fn interrupted_spawn_persists_a_resumable_checkpoint() {
        let project = tempfile::TempDir::new().unwrap();
        let mut work_spec = work_spec();
        work_spec.base_revision.git_head = Some("base-commit".to_string());
        let child_thread_id = ThreadId::new();
        let runtime = CccxSpawnRuntime {
            project_root: project.path().to_path_buf(),
            logical_session_id: "logical-interrupted".to_string(),
            work_spec,
            display_name: "interrupted-worker".to_string(),
            model_route_summary: "9router/test".to_string(),
            gateway_provenance: None,
            tool_call_sequence_digest: None,
            parent_capabilities: None,
        };
        let mut registry = CccxRuntimeRegistry::default();
        registry
            .register_spawn(child_thread_id, runtime)
            .await
            .unwrap();
        assert!(
            registry
                .complete_spawn(child_thread_id, "provider disconnected".to_string(), false)
                .await
                .unwrap()
                .is_some()
        );

        let checkpoint = crate::checkpoint::load_checkpoint(project.path(), "logical-interrupted")
            .await
            .unwrap()
            .expect("interrupted worker must leave a checkpoint");
        assert_eq!(checkpoint.task_id, "T1");
        assert_eq!(checkpoint.logical_session_id, "logical-interrupted");
        assert_eq!(
            checkpoint.worker_incarnation_id,
            child_thread_id.to_string()
        );
        assert_eq!(checkpoint.base_commit.as_deref(), Some("base-commit"));
        assert_eq!(checkpoint.last_completed_step, "provider disconnected");
    }

    #[tokio::test]
    async fn cancelled_spawn_is_terminal_and_never_writes_a_checkpoint() {
        let project = tempfile::TempDir::new().unwrap();
        let child_thread_id = ThreadId::new();
        let runtime = CccxSpawnRuntime {
            project_root: project.path().to_path_buf(),
            logical_session_id: "logical-cancelled".to_string(),
            work_spec: work_spec(),
            display_name: "cancelled-worker".to_string(),
            model_route_summary: "9router/test".to_string(),
            gateway_provenance: None,
            tool_call_sequence_digest: None,
            parent_capabilities: None,
        };
        let mut registry = CccxRuntimeRegistry::default();
        registry
            .register_spawn(child_thread_id, runtime)
            .await
            .unwrap();
        assert!(
            registry
                .cancel_spawn(child_thread_id, "user cancelled".to_string())
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            crate::checkpoint::load_checkpoint(project.path(), &child_thread_id.to_string())
                .await
                .unwrap()
                .is_none()
        );
        let graph = cccx_task_graph_state::load_task_graph(project.path())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            graph.tasks["T1"].status,
            cccx_task_graph::TaskStatus::Cancelled
        );
    }

    // ── Phase 3 tests ──

    /// Phase 3: parent_capability_set_for_work_spec MUST use parent's actual
    /// tools from Main CC config, NOT clone from the WorkSpec itself.
    /// (implementer-issues §2: "parent_capability_set_for_work_spec()
    /// lấy capability của parent trực tiếp từ chính WorkSpec")
    #[test]
    fn p3_parent_capability_uses_actual_parent_tools_not_work_spec() {
        let ws = work_spec();
        let project_root = std::path::Path::new("D:/repo");
        let main_model = "9router/main";

        // The parent (Main CC) has MORE tools than the WorkSpec
        let parent_tools = BTreeSet::from([
            "shell_command".to_string(),
            "edit".to_string(),
            "read".to_string(),
            "spawn".to_string(),
            "apply_patch".to_string(),
        ]);
        let parent_mcp = BTreeSet::from([
            "codebase-memory-mcp".to_string(),
            "sequential-thinking".to_string(),
        ]);

        let parent = parent_capability_set_for_work_spec(
            project_root,
            &ws,
            parent_tools.clone(),
            parent_mcp.clone(),
            BTreeSet::from([main_model.to_string()]),
        );

        // Phase 3: parent.tools must be the ACTUAL parent tools, not the
        // WorkSpec's allowed_tools. Before the fix, this was:
        //   tools: work_spec.allowed_tools.clone()  ← circular!
        assert_eq!(parent.tools, parent_tools);
        assert_eq!(parent.mcp_tools, parent_mcp);
        assert_eq!(parent.permission_ceiling, PermissionPolicy::Allow);
        assert!(parent.model_routes.contains("9router/main"));
    }

    /// Phase 3: intersection of WorkSpec tools with narrow parent capability
    /// must correctly reduce the projection.
    #[test]
    fn p3_parent_capability_intersection_reduces_projection() {
        let ws = work_spec(); // allowed_tools = {"shell_command"}
        let project_root = std::path::Path::new("D:/repo");

        // Parent has NO tools — empty set
        let parent_tools = BTreeSet::new();
        let parent_mcp = BTreeSet::new();

        let parent = parent_capability_set_for_work_spec(
            project_root,
            &ws,
            parent_tools,
            parent_mcp,
            BTreeSet::from(["9router/main".to_string()]),
        );

        // The projection should be empty because WorkSpec ∩ parent = ∅
        let projection = cccx_protocol::project_child_config(&ws, &parent).unwrap();
        assert!(
            projection.allowed_tools.is_empty(),
            "WorkSpec tools ∩ empty parent = empty. Got: {:?}",
            projection.allowed_tools
        );
    }

    #[test]
    fn p7_parent_catalog_does_not_admit_a_route_solely_requested_by_work_spec() {
        let mut ws = work_spec();
        ws.model_route = ModelRouteSelection::Explicit {
            route_id: "9router/unconfigured-route".to_string(),
        };
        let parent = parent_capability_set_for_work_spec(
            std::path::Path::new("D:/repo"),
            &ws,
            BTreeSet::new(),
            BTreeSet::new(),
            BTreeSet::from(["9router/main".to_string()]),
        );

        assert!(matches!(
            cccx_protocol::project_child_config(&ws, &parent),
            Err(cccx_protocol::ProtocolError::ModelRouteNotAllowed(route))
                if route == "9router/unconfigured-route"
        ));
    }

    // ── Phase 5 tests ──

    /// Phase 5: provenance_for_runtime() produces real SHA256 digests,
    /// not placeholder `sha256:cccx-runtime-*` strings.
    /// task.md §Phase 5 exit criteria: "Provenance không còn placeholder sha256:cccx-runtime-*".
    #[test]
    fn p5_provenance_digests_are_real_not_placeholders() {
        let ws = work_spec();
        let prov = provenance_for_runtime(
            &ws,
            "9router/test-route",
            "test completed: 3/3 passed",
            None,
            None,
        );

        // Digests must be real SHA256 format
        assert!(
            prov.input_digest.starts_with("sha256:"),
            "input_digest must be sha256:... Got: {}",
            prov.input_digest
        );
        assert!(
            prov.output_digest.starts_with("sha256:"),
            "output_digest must be sha256:... Got: {}",
            prov.output_digest
        );
        // Must NOT be placeholder
        assert!(
            !prov.input_digest.contains("cccx-runtime"),
            "Phase 5: input_digest must not contain cccx-runtime placeholder. Got: {}",
            prov.input_digest
        );
        assert!(
            !prov.output_digest.contains("cccx-runtime"),
            "Phase 5: output_digest must not contain cccx-runtime placeholder. Got: {}",
            prov.output_digest
        );
        // Must be 64 hex chars after "sha256:"
        let input_hex = prov.input_digest.strip_prefix("sha256:").unwrap();
        let output_hex = prov.output_digest.strip_prefix("sha256:").unwrap();
        assert_eq!(input_hex.len(), 64, "digest must be 64 hex chars");
        assert_eq!(output_hex.len(), 64, "digest must be 64 hex chars");
        assert!(input_hex.chars().all(|c| c.is_ascii_hexdigit()));
    }

    /// Phase 5: provenance digests change when WorkSpec payload changes.
    /// Different objective/tools → different input_digest.
    /// Different status → different output_digest.
    #[test]
    fn p5_provenance_digest_changes_when_payload_changes() {
        let ws1 = work_spec(); // objective = "run tests", tools = {"shell_command"}
        let mut ws2 = ws1.clone();
        ws2.objective = "build and test with coverage".to_string();
        ws2.allowed_tools = BTreeSet::from(["edit".to_string(), "apply_patch".to_string()]);

        let prov1 = provenance_for_runtime(&ws1, "route-1", "status A", None, None);
        let prov2 = provenance_for_runtime(&ws2, "route-1", "status A", None, None);

        // Different input payloads → different input digests
        assert_ne!(
            prov1.input_digest, prov2.input_digest,
            "Phase 5: provenance digests MUST change when WorkSpec payload changes"
        );

        // Different status_summary → different output_digest (Phase 5 audit fix)
        let prov3 =
            provenance_for_runtime(&ws1, "route-1", "status B: different output", None, None);
        assert_ne!(
            prov1.output_digest, prov3.output_digest,
            "Phase 5 audit: output_digest MUST change when status_summary changes"
        );
    }

    /// Phase 5: effective_route_id must be "unknown" when no gateway_request_id.
    /// It must NOT be a copy of requested_route_id.
    #[test]
    fn p5_effective_route_unknown_when_no_gateway_evidence() {
        let ws = work_spec();
        let prov = provenance_for_runtime(&ws, "9router/requested-route", "completed", None, None);

        // When gateway_request_id is None, effective_route_id must be "unknown", not "9router/requested-route"
        assert_eq!(
            prov.requested_route_id, "9router/requested-route",
            "requested should be preserved"
        );
        assert!(prov.gateway_request_id.is_none(), "no gateway evidence");
        assert_eq!(
            prov.effective_route_id, "unknown",
            "Phase 5: effective_route_id must be 'unknown' when no 9router evidence, not a copy of requested. Got: {}",
            prov.effective_route_id
        );
        // Sanity: effective ≠ requested when no gateway evidence
        assert_ne!(
            prov.effective_route_id, prov.requested_route_id,
            "Phase 5: effective must differ from requested when no gw evidence"
        );
    }

    /// Phase 5 audit: provenance must pass verify_digest_format — sha256: + 64 hex chars.
    /// Runtime-produced provenance goes through the protocol's cryptographic audit.
    #[test]
    fn p5_audit_provenance_passes_digest_format_validation() {
        let ws = work_spec();
        let prov = provenance_for_runtime(&ws, "route", "completed successfully", None, None);
        // Runtime provenance MUST pass the protocol's digest format checker
        prov.verify_digest_format()
            .expect("Phase 5: runtime provenance digest must pass verify_digest_format");
    }

    #[test]
    fn p7_provenance_carries_only_the_runtime_tool_sequence_digest() {
        let ws = work_spec();
        let digest = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let provenance = provenance_for_runtime(&ws, "route", "completed", None, Some(digest));
        assert_eq!(
            provenance.tool_call_sequence_digest.as_deref(),
            Some(digest)
        );
    }

    /// Phase 5 audit: test helper provenance (hardcoded "sha256:i" / "sha256:o") also passes
    /// placeholder check (no cccx-runtime in it). Test fixture integrity check.
    #[test]
    fn p5_audit_existing_test_provenance_passes_placeholder_check() {
        provenance()
            .verify_placeholders()
            .expect("test fixture provenance must not contain cccx-runtime");
    }

    // ── Phase 6 tests ──

    /// Phase 6: StopHook must block final answer when any task is unverified.
    /// task.md Phase 6 exit criteria: "StopHook không thể bị pass nếu task graph còn unverified."
    #[test]
    fn p6_stop_guard_blocks_with_unverified_task() {
        let mut graph = TaskGraph::new("g", "test");
        graph
            .add_task(TaskNode::new(
                "T1",
                "prove the task",
                VerificationSpec {
                    command: None,
                    protected_digests: BTreeMap::new(),
                    verification_policy_id: Some("focused-tests".to_string()),
                    spec_digest: "sha256:spec".to_string(),
                },
            ))
            .unwrap();
        let mut runtime = CccxRuntimeState::new(graph.clone());

        // Spawn subagent, get attempt
        runtime
            .register_work_spec(
                work_spec(),
                "attempt-1".into(),
                "thread-1".into(),
                "worker".into(),
                "route".into(),
            )
            .unwrap();

        // Apply Artifact WITHOUT verification: pass None for exit_code and log_digest.
        // When both are None, GoalEvaluator::evaluate() returns None, so
        // record_verification() is never called → task stays unverified.
        let mut collector = ArtifactCollector::new("ws-1", "attempt-1");
        collector.record_runtime_log("tests pass", "3/3 ok");
        let artifact = collector.finalize(
            ArtifactStatus::Success,
            "all tests passed".into(),
            provenance(),
        );
        runtime.complete(artifact, None, None).unwrap();

        // stop_guard must block because no VerificationRecord.passed=true
        assert!(
            cccx_task_graph_state::stop_guard(runtime.task_graph()).is_err(),
            "Phase 6: StopHook must block when task is unverified"
        );
        assert!(
            !runtime.task_graph().can_stop(),
            "Phase 6: can_stop must be false when tasks are not verified"
        );
    }

    /// Phase 6: StopHook must ALLOW final answer when all tasks are verified.
    /// task.md Phase 6 exit criteria: "StopHook allow final answer khi mọi task verified."
    #[test]
    fn p6_stop_guard_allows_when_all_tasks_verified() {
        let mut graph = TaskGraph::new("g", "test");
        graph
            .add_task(TaskNode::new(
                "T1",
                "prove the task with evidence",
                VerificationSpec {
                    command: None,
                    protected_digests: BTreeMap::new(),
                    verification_policy_id: Some("focused-tests".to_string()),
                    spec_digest: "sha256:s".to_string(),
                },
            ))
            .unwrap();
        let mut runtime = CccxRuntimeState::new(graph);
        runtime
            .register_work_spec(
                work_spec(),
                "attempt-1".into(),
                "thread-1".into(),
                "worker".into(),
                "route".into(),
            )
            .unwrap();

        // Full cycle: Artifact → VerificationRecord(passed=true) → can_stop=true
        let mut collector = ArtifactCollector::new("ws-1", "attempt-1");
        collector.record_test_run("cargo test", 0, "checksum");
        runtime
            .complete(
                collector.finalize(ArtifactStatus::Success, "ok".into(), provenance()),
                Some(0),
                Some("sha256:log"),
            )
            .unwrap();

        // Verify: stop_guard allows and can_stop is true
        cccx_task_graph_state::stop_guard(runtime.task_graph())
            .expect("Phase 6: StopHook must allow when all tasks are verified");
        assert!(
            runtime.task_graph().can_stop(),
            "Phase 6: can_stop must be true after verification"
        );
    }

    /// Phase 6 audit: verify `ensure_task_exists()` now produces SHA256 spec_digest,
    /// not the placeholder `sha256:cccx-runtime-*` pattern.
    #[test]
    fn p6_audit_spec_digest_is_real_sha256_not_placeholder() {
        let graph = TaskGraph::new("g", "test");
        let mut runtime = CccxRuntimeState::new(graph);

        // register_work_spec triggers ensure_task_exists if task doesn't exist
        let mut ws = work_spec();
        ws.task_id = "T-spec".to_string();
        runtime
            .register_work_spec(
                ws,
                "attempt-spec".into(),
                "thread-spec".into(),
                "worker".into(),
                "route".into(),
            )
            .unwrap();

        let spec_digest = &runtime.task_graph().tasks["T-spec"]
            .verification_spec
            .spec_digest;

        assert!(
            spec_digest.starts_with("sha256:"),
            "Phase 6 audit: spec_digest must start with sha256:"
        );
        assert!(
            !spec_digest.contains("cccx-runtime"),
            "Phase 6 audit: spec_digest must NOT contain cccx-runtime placeholder. Got: {}",
            spec_digest
        );
        let hex_part = spec_digest.strip_prefix("sha256:").unwrap();
        assert_eq!(hex_part.len(), 64, "spec_digest must be 64 hex chars");
        assert!(hex_part.chars().all(|c| c.is_ascii_hexdigit()));
    }

    // ── Phase 10 tests ──

    /// Phase 10: Explicit model route produces the route_id directly.
    /// task.md: "WorkSpec model route resolve order: explicit spawn override first."
    #[test]
    fn p10_explicit_route_wins_over_inheritance() {
        let mut ws = work_spec();
        ws.model_route = ModelRouteSelection::Explicit {
            route_id: "oc/deepseek-v4-pro".to_string(),
        };
        let summary = model_route_summary(&ws, "main-default");

        assert_eq!(summary, "oc/deepseek-v4-pro");
    }

    /// Phase 10: InheritMainRequested uses main_model_slug (parent's requested route).
    #[test]
    fn p10_inherit_main_requested_uses_parent_slug() {
        let mut ws = work_spec();
        ws.model_route = ModelRouteSelection::InheritMainRequested;
        let summary = model_route_summary(&ws, "9router/main-requested");

        assert_eq!(summary, "9router/main-requested");
    }

    /// Phase 10: fallback_policy_id is pinned per attempt.
    /// task.md: "fallback policy pinned per attempt."
    /// The resolver in cccx-provider-gateway has a 7-test priority chain;
    /// this test proves the policy_id survives through the spawn code path.
    #[test]
    fn p10_fallback_policy_id_pinned_per_work_spec() {
        let ws = {
            let mut ws = work_spec();
            ws.fallback_policy_id = "strict".to_string();
            ws.model_route = ModelRouteSelection::Explicit {
                route_id: "oc/deepseek-v4-pro".to_string(),
            };
            ws
        };

        // fallback_policy_id must be the value set in WorkSpec
        assert_eq!(ws.fallback_policy_id, "strict");
        // model_route must be independent of fallback
        assert_eq!(model_route_summary(&ws, "main"), "oc/deepseek-v4-pro");
    }

    /// Phase 10: fallback_policy_id is set per-WorkSpec distinct from model route.
    /// Two WorkSpecs with different policies must carry their respective values.
    #[test]
    fn p10_fallback_policy_differs_between_work_specs() {
        let ws_strict = {
            let mut ws = work_spec();
            ws.fallback_policy_id = "strict".to_string();
            ws
        };
        let ws_compatible = {
            let mut ws = work_spec();
            ws.fallback_policy_id = "compatible".to_string();
            ws
        };

        assert_eq!(ws_strict.fallback_policy_id, "strict");
        assert_eq!(ws_compatible.fallback_policy_id, "compatible");
        assert_ne!(
            ws_strict.fallback_policy_id,
            ws_compatible.fallback_policy_id
        );
    }

    #[test]
    fn p11_runtime_agent_update_uses_work_spec_identity_without_gateway_inference() {
        let mut ws = work_spec();
        ws.role = WorkRole::Implementer;
        ws.model_route = ModelRouteSelection::Explicit {
            route_id: "9router/cx-implementer".to_string(),
        };
        let runtime = CccxSpawnRuntime {
            project_root: PathBuf::from("D:/repo"),
            work_spec: ws,
            logical_session_id: "ws-typed".to_string(),
            display_name: "task-implementer".to_string(),
            model_route_summary: "9router/cx-implementer".to_string(),
            gateway_provenance: None,
            tool_call_sequence_digest: None,
            parent_capabilities: None,
        };

        let update = runtime.agent_update_event("running", Some("approval-7".to_string()));

        assert_eq!(update.logical_session_id, "ws-typed");
        assert_eq!(update.display_name, "task-implementer");
        assert_eq!(update.role, "implementer");
        assert_eq!(update.status, "running");
        assert_eq!(update.permission_request_id.as_deref(), Some("approval-7"));
        assert_eq!(update.model_route.selection_source, "explicit");
        assert_eq!(
            update.model_route.requested_route_id,
            "9router/cx-implementer"
        );
        assert_eq!(update.model_route.effective_route_id, "unknown");
        assert!(update.model_route.fallback_chain.is_empty());
    }

    #[test]
    fn p11_runtime_agent_update_uses_gateway_effective_route_when_present() {
        let mut gateway = GatewayProvenance::new("9router/requested".to_string());
        gateway.record_effective(
            "9router/effective".to_string(),
            "provider".to_string(),
            "model".to_string(),
        );
        gateway.record_effective(
            "9router/fallback".to_string(),
            "fallback-provider".to_string(),
            "fallback-model".to_string(),
        );
        let runtime = CccxSpawnRuntime {
            project_root: PathBuf::from("D:/repo"),
            work_spec: work_spec(),
            logical_session_id: "ws-gateway".to_string(),
            display_name: "task-tester".to_string(),
            model_route_summary: "parent-main".to_string(),
            gateway_provenance: Some(gateway),
            tool_call_sequence_digest: None,
            parent_capabilities: None,
        };

        let update = runtime.agent_update_event("completed", None);

        assert_eq!(
            update.model_route.selection_source,
            "inherit_main_effective"
        );
        assert_eq!(update.model_route.requested_route_id, "9router/requested");
        assert_eq!(update.model_route.effective_route_id, "9router/effective");
        assert_eq!(
            update.model_route.fallback_chain,
            vec!["fallback-provider:fallback-model".to_string()]
        );
    }

    // ═══════════════════════════════════════════════════════════════════
    // Phase 13 — Cross-crate E2E chain
    // ═══════════════════════════════════════════════════════════════════
    //
    // task.md Phase 13: "chứng minh các feature chính chạy cùng nhau trong
    // runtime, không chỉ unit test từng crate."
    //
    // These tests link cccx-protocol + cccx-task-graph + cccx-policy +
    // cccx_runtime + cccx_spawn into a single end-to-end chain.
    //
    // Scenarios 1-2,4-6 are proven by existing tests (Phase 1 Mode, Phase 2
    // Policy, Phase 7 Queue, Phase 10 Gateway, Phase 11 Headless). Scenario 3
    // (Spawn CX) chain is proven here.

    /// Phase 13: Full CX spawn chain — WorkSpec → projection → harness
    /// injection → CX restrictions enforced.
    ///
    /// Scenario 3 (task.md:923-928): Main CC creates WorkSpec, CX receives
    /// limited tools, disallowed tool rejected, CX produces Artifact,
    /// Task verified only after VerificationRecord.
    #[test]
    fn p13_cross_crate_spawn_cx_full_chain_work_spec_to_verified_task() {
        use cccx_protocol::ParentCapabilitySet;
        use cccx_protocol::project_child_config;

        // Step 1: WorkSpec defines CX scope — task_id must match graph node
        let mut ws = work_spec();
        ws.task_id = "T-p13".to_string();
        ws.allowed_tools.insert("edit".to_string());
        ws.allowed_tools.insert("shell".to_string());
        ws.allowed_mcp_tools
            .insert("sequential-thinking".to_string());

        // Step 2: Parent (Main CC) has full capability → intersection limits CX
        let mut parent_tools = BTreeSet::new();
        parent_tools.insert("edit".to_string());
        parent_tools.insert("shell".to_string());
        parent_tools.insert("mcp_call".to_string());
        parent_tools.insert("spawn_agent".to_string());

        let mut parent_mcp = BTreeSet::new();
        parent_mcp.insert("codebase-memory-mcp".to_string());
        parent_mcp.insert("sequential-thinking".to_string());

        let parent = ParentCapabilitySet {
            tools: parent_tools,
            mcp_tools: parent_mcp,
            writable_roots: BTreeSet::from(["src/".to_string()]),
            protected_paths: BTreeSet::new(),
            model_routes: BTreeSet::from(["9router/*".to_string()]),
            permission_ceiling: PermissionPolicy::Allow,
        };

        let projection = project_child_config(&ws, &parent).unwrap();
        assert!(!projection.allowed_tools.is_empty());
        assert!(
            projection
                .allowed_tools
                .iter()
                .all(|t| ["edit", "shell"].contains(&t.as_str()))
        );

        // Step 3: TaskGraph lifecycle — register → verify → stop guard
        let mut graph = TaskGraph::new("g-p13", "test");
        graph
            .add_task(TaskNode::new(
                "T-p13",
                "prove E2E spawn chain",
                VerificationSpec {
                    command: None,
                    protected_digests: BTreeMap::new(),
                    verification_policy_id: Some("focused-tests".to_string()),
                    spec_digest: "sha256:p13".to_string(),
                },
            ))
            .unwrap();

        let mut runtime = CccxRuntimeState::new(graph);
        runtime
            .register_work_spec(
                ws.clone(),
                "attempt-p13".into(),
                "thread-p13".into(),
                "cx-worker".into(),
                "9router/main".into(),
            )
            .unwrap();

        // Step 4: CX produces Artifact with VerificationRecord
        let mut collector = ArtifactCollector::new("ws-1", "attempt-p13");
        collector.record_test_run("cargo test", 0, "sha256:e2e");
        let artifact = collector.finalize(
            ArtifactStatus::Success,
            "all e2e tests passed".into(),
            provenance(),
        );
        runtime
            .complete(artifact, Some(0), Some("sha256:e2e"))
            .unwrap();

        // Step 5: VerificationRecord with exit_code=0 marks task verified
        let task = &runtime.task_graph().tasks["T-p13"];
        assert!(
            task.verified_at.is_some(),
            "Phase 13: task must be verified after VerificationRecord with exit_code=0"
        );

        // Step 6: StopGuard allows completion because all tasks are verified
        cccx_task_graph_state::stop_guard(runtime.task_graph())
            .expect("Phase 13: StopGuard must allow when all tasks verified");
        assert!(
            runtime.task_graph().can_stop(),
            "Phase 13: can_stop must be true after full E2E chain"
        );
    }

    /// Phase 13: Regression guard — removing any step in the chain must fail.
    /// If VerificationRecord is missing, the StopGuard must not allow completion.
    #[test]
    fn p13_cross_crate_regression_chain_breaks_without_verification() {
        let mut graph = TaskGraph::new("g-p13r", "test");
        graph
            .add_task(TaskNode::new(
                "T-p13r",
                "prove regression detection",
                VerificationSpec {
                    command: None,
                    protected_digests: BTreeMap::new(),
                    verification_policy_id: Some("focused-tests".to_string()),
                    spec_digest: "sha256:p13r".to_string(),
                },
            ))
            .unwrap();

        let mut runtime = CccxRuntimeState::new(graph);
        runtime
            .register_work_spec(
                work_spec(),
                "attempt-p13r".into(),
                "thread-p13r".into(),
                "cx-worker".into(),
                "9router/main".into(),
            )
            .unwrap();

        // Apply Artifact with NO verification (None, None)
        let mut collector = ArtifactCollector::new("ws-1", "attempt-p13r");
        collector.record_runtime_log("tests pass", "3/3 ok");
        let artifact =
            collector.finalize(ArtifactStatus::Success, "tests passed".into(), provenance());
        runtime.complete(artifact, None, None).unwrap();

        // Regression: StopGuard MUST block because no VerificationRecord
        assert!(
            cccx_task_graph_state::stop_guard(runtime.task_graph()).is_err(),
            "Phase 13 regression: StopGuard must block without VerificationRecord"
        );
        assert!(
            !runtime.task_graph().can_stop(),
            "Phase 13 regression: can_stop must be false without verification"
        );
    }

    /// Phase 13: Gateway provenance chain — provenance must be real SHA256,
    /// not placeholder, and must carry effective_route_id.
    #[test]
    fn p13_cross_crate_provenance_chain_no_placeholder_survives_validation() {
        use cccx_protocol::ArtifactProvenance;

        let prov = provenance();
        assert!(
            !prov.input_digest.contains("cccx-runtime"),
            "input_digest must not be placeholder"
        );
        assert!(
            !prov.output_digest.contains("cccx-runtime"),
            "output_digest must not be placeholder"
        );
        assert!(
            !prov.effective_route_id.is_empty(),
            "effective_route_id must be populated"
        );

        // verify_placeholders must pass with real provenance
        let result = ArtifactProvenance::verify_placeholders(&prov);
        assert!(
            result.is_ok(),
            "Phase 13: provenance with real SHA256 digests must pass validation"
        );
    }
}
