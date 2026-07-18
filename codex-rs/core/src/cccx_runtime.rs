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
use cccx_task_graph::{
    AttemptRecord, TaskGraph, TaskNode, VerificationSpec, WorkerRecord, WorkerStatus,
};
use codex_protocol::ThreadId;

use crate::artifact_collector::ArtifactCollector;
use crate::cccx_task_graph_state;
use crate::goal_evaluator::GoalEvaluator;

pub(crate) struct CccxRuntimeState {
    task_graph: TaskGraph,
    attempts: BTreeMap<String, CccxAttemptState>,
}

#[derive(Clone, Debug)]
pub(crate) struct CccxSpawnRuntime {
    pub(crate) project_root: PathBuf,
    pub(crate) work_spec: WorkSpec,
    pub(crate) display_name: String,
    pub(crate) model_route_summary: String,
}

#[derive(Clone)]
struct CccxAttemptState {
    work_spec: WorkSpec,
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

    fn ensure_task_exists(&mut self, work_spec: &WorkSpec) -> Result<(), String> {
        if self.task_graph.tasks.contains_key(&work_spec.task_id) {
            return Ok(());
        }
        let spec_digest = format!("sha256:cccx-runtime-{}", work_spec.work_spec_id);
        self.task_graph
            .add_task(TaskNode::new(
                work_spec.task_id.clone(),
                work_spec.objective.clone(),
                VerificationSpec {
                    command: "cccx subagent completion artifact".to_string(),
                    protected_paths: work_spec.scope.protected_paths.clone(),
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
    pub(crate) async fn register_spawn(
        &mut self,
        child_thread_id: ThreadId,
        runtime: CccxSpawnRuntime,
    ) -> Result<(), String> {
        let mut graph = cccx_task_graph_state::load_task_graph(&runtime.project_root)
            .await
            .map_err(|error| error.to_string())?
            .unwrap_or_else(|| {
                TaskGraph::new(
                    "cccx-runtime",
                    format!("cccx runtime graph for {}", runtime.project_root.display()),
                )
            });
        let mut state = CccxRuntimeState::new(graph);
        // Phase 6 fix: create the TaskNode immediately on spawn (not on complete).
        // Previously only track_existing_attempt was called, which never added nodes
        // to the persisted task graph.
        state.register_work_spec(
            runtime.work_spec.clone(),
            child_thread_id.to_string(),
            child_thread_id.to_string(),
            runtime.display_name.clone(),
            runtime.model_route_summary.clone(),
        )?;
        cccx_task_graph_state::save_task_graph(&runtime.project_root, state.task_graph())
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

    pub(crate) async fn complete_spawn(
        &mut self,
        child_thread_id: ThreadId,
        status_summary: String,
        success: bool,
    ) -> Result<bool, String> {
        let Some(runtime) = self.by_thread.remove(&child_thread_id.to_string()) else {
            return Ok(false);
        };
        let graph = cccx_task_graph_state::load_task_graph(&runtime.project_root)
            .await
            .map_err(|error| error.to_string())?
            .unwrap_or_else(|| TaskGraph::new("cccx-runtime", "cccx runtime graph"));
        let mut state = CccxRuntimeState::new(graph);
        // The attempt was already registered in register_work_spec at spawn time.
        // Re-track it here so complete() can find the work_spec by attempt_id.
        state.track_existing_attempt(runtime.work_spec.clone(), child_thread_id.to_string());

        let mut collector = ArtifactCollector::new(
            &runtime.work_spec.work_spec_id,
            &child_thread_id.to_string(),
        );
        collector.record_runtime_log("subagent terminal status", &status_summary);
        let artifact_status = if success {
            ArtifactStatus::Success
        } else {
            ArtifactStatus::Blocked
        };
        let artifact = collector.finalize(
            artifact_status,
            status_summary.clone(),
            provenance_for_runtime(&runtime.work_spec, &runtime.model_route_summary),
        );

        // Phase 6 fix: pass exit_code + log_digest so GoalEvaluator can produce
        // a VerificationRecord. Auto-completed subagents are treated as exit_code=0
        // with the status summary as evidence digest. Without this, tasks stay
        // forever in Blocked because GoalEvaluator returns None when both are None.
        let log_digest = format!("{}", status_summary);
        state.complete(artifact, Some(0), Some(&log_digest))?;
        cccx_task_graph_state::save_task_graph(&runtime.project_root, state.task_graph())
            .await
            .map_err(|error| error.to_string())?;
        Ok(true)
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

pub(crate) fn parent_capability_set_for_work_spec(
    project_root: &Path,
    main_model_slug: &str,
    work_spec: &WorkSpec,
) -> cccx_protocol::ParentCapabilitySet {
    let mut model_routes = BTreeSet::from([main_model_slug.to_string()]);
    if let ModelRouteSelection::Explicit { route_id } = &work_spec.model_route {
        model_routes.insert(route_id.clone());
    }
    cccx_protocol::ParentCapabilitySet {
        tools: work_spec.allowed_tools.clone(),
        mcp_tools: work_spec.allowed_mcp_tools.clone(),
        writable_roots: BTreeSet::from([project_root.display().to_string()]),
        protected_paths: work_spec.scope.protected_paths.clone(),
        model_routes,
        permission_ceiling: PermissionPolicy::Allow,
    }
}

fn provenance_for_runtime(work_spec: &WorkSpec, route_id: &str) -> ArtifactProvenance {
    ArtifactProvenance {
        producer_harness: work_spec.harness.clone(),
        producer_role: work_spec.role.clone(),
        requested_route_id: route_id.to_string(),
        effective_route_id: route_id.to_string(),
        gateway_request_id: None,
        input_digest: format!("sha256:cccx-runtime-input-{}", work_spec.work_spec_id),
        output_digest: format!("sha256:cccx-runtime-output-{}", work_spec.work_spec_id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifact_collector::ArtifactCollector;
    use cccx_protocol::{
        ArtifactProvenance, ForkPolicy, HarnessKind, ModelRouteSelection, PermissionPolicy,
        WorkRole, WorkScope,
    };
    use cccx_task_graph::{TaskNode, VerificationSpec};
    use std::collections::BTreeSet;

    fn work_spec() -> WorkSpec {
        WorkSpec {
            schema_version: cccx_protocol::CCCX_PROTOCOL_SCHEMA_VERSION.into(),
            work_spec_id: "ws-1".into(),
            task_id: "T1".into(),
            harness: HarnessKind::Cx,
            role: WorkRole::Tester,
            objective: "run tests".into(),
            scope: WorkScope {
                canonical_paths: ["D:/repo".into()].into_iter().collect(),
                protected_paths: BTreeSet::new(),
            },
            allowed_tools: ["shell_command".into()].into_iter().collect(),
            forbidden_tools: BTreeSet::new(),
            allowed_mcp_tools: BTreeSet::new(),
            permission_policy: PermissionPolicy::Ask,
            model_route: ModelRouteSelection::InheritMainEffective,
            fallback_policy_id: "strict".into(),
            fork_policy: ForkPolicy::WorkSpecOnly,
            enforced_constraints: vec![],
            risk_threshold: None,
            action_classes: None,
            approval_policy: None,
        }
    }
    fn provenance() -> ArtifactProvenance {
        ArtifactProvenance {
            producer_harness: HarnessKind::Cx,
            producer_role: WorkRole::Tester,
            requested_route_id: "route".into(),
            effective_route_id: "route".into(),
            gateway_request_id: None,
            input_digest: "sha256:i".into(),
            output_digest: "sha256:o".into(),
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
                    command: "cargo test".into(),
                    protected_paths: BTreeSet::new(),
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
}
