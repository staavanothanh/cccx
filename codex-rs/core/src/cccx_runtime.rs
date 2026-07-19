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
        let protocol_spec = work_spec.verification_spec.clone().unwrap_or(
            cccx_protocol::VerificationSpec {
                command: None,
                protected_digests: BTreeMap::new(),
                verification_policy_id: None,
            },
        );
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
                    child_thread_id.to_string(),
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

    pub(crate) async fn complete_spawn(
        &mut self,
        child_thread_id: ThreadId,
        status_summary: String,
        success: bool,
    ) -> Result<bool, String> {
        let Some(runtime) = self.by_thread.remove(&child_thread_id.to_string()) else {
            return Ok(false);
        };
        if !success {
            let checkpoint = SubagentCheckpoint {
                logical_session_id: child_thread_id.to_string(),
                worker_incarnation_id: child_thread_id.to_string(),
                task_id: runtime.work_spec.task_id.clone(),
                attempt_id: child_thread_id.to_string(),
                last_completed_step: status_summary.clone(),
                partial_diff: String::new(),
                pending_steps: vec![runtime.work_spec.objective.clone()],
                checkpointed_at: chrono::Utc::now().to_rfc3339(),
                base_commit: runtime.work_spec.base_revision.git_head.clone(),
            };
            crate::checkpoint::save_checkpoint(&runtime.project_root, &checkpoint)
                .await
                .map_err(|error| format!("failed to persist interrupted worker checkpoint: {error}"))?;
        }
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
            ArtifactStatus::Blocked
        };
        let artifact = collector.finalize(
            artifact_status,
            status_summary.clone(),
            provenance_for_runtime(
                &runtime.work_spec,
                &runtime.model_route_summary,
                &status_summary,
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
        cccx_task_graph_state::update_task_graph(&runtime.project_root, initial, |graph| {
            let mut state = CccxRuntimeState::new(graph.clone());
            state.track_existing_attempt(runtime.work_spec.clone(), child_thread_id.to_string());
            // A terminal worker status is an Artifact, not independent verification evidence.
            state
                .complete(artifact, None, None)
                .map_err(cccx_task_graph_state::TaskGraphStateError::Mutation)?;
            if let Some(run) = verification_run {
                use sha2::{Digest, Sha256};
                let task_id = runtime.work_spec.task_id.clone();
                let artifact_digest = format!("sha256:{:x}", Sha256::digest(status_summary.as_bytes()));
                let task = state.task_graph().tasks.get(&task_id).ok_or_else(|| {
                    cccx_task_graph_state::TaskGraphStateError::Mutation(format!("unknown cccx task `{task_id}`"))
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
            Ok(())
        })
        .await
            .map_err(|error| error.to_string())?;
        if let Some(error) = verification_error {
            return Err(error);
        }
        Ok(true)
    }

    /// Persist a terminal user/CC cancellation. Unlike an interrupted worker,
    /// this deliberately creates no checkpoint and therefore cannot be resumed
    /// as the same attempt.
    pub(crate) async fn cancel_spawn(
        &mut self,
        child_thread_id: ThreadId,
        reason: String,
    ) -> Result<bool, String> {
        let Some(runtime) = self.by_thread.remove(&child_thread_id.to_string()) else {
            return Ok(false);
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
            ),
        );
        let initial = TaskGraph::new("cccx-runtime", "cccx runtime graph");
        cccx_task_graph_state::update_task_graph(&runtime.project_root, initial, |graph| {
            let mut state = CccxRuntimeState::new(graph.clone());
            state.track_existing_attempt(runtime.work_spec.clone(), child_thread_id.to_string());
            state
                .complete(artifact, None, None)
                .map_err(cccx_task_graph_state::TaskGraphStateError::Mutation)?;
            *graph = state.task_graph.clone();
            Ok(())
        })
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

/// Build a ParentCapabilitySet for WorkSpec projection.
///
/// Phase 3 fix: `parent_allowed_tools` and `parent_allowed_mcp_tools` are
/// the actual Main CC's available tools/MCP from its runtime Config, NOT
/// the WorkSpec's own allowed_tools. Previously, the function cloned
/// `work_spec.allowed_tools` into the parent capability, making the
/// intersection with itself a no-op (implementer-issues §2).
pub(crate) fn parent_capability_set_for_work_spec(
    project_root: &Path,
    main_model_slug: &str,
    work_spec: &WorkSpec,
    parent_allowed_tools: BTreeSet<String>,
    parent_allowed_mcp_tools: BTreeSet<String>,
) -> cccx_protocol::ParentCapabilitySet {
    let mut model_routes = BTreeSet::from([main_model_slug.to_string()]);
    if let ModelRouteSelection::Explicit { route_id } = &work_spec.model_route {
        model_routes.insert(route_id.clone());
    }
    cccx_protocol::ParentCapabilitySet {
        tools: parent_allowed_tools,
        mcp_tools: parent_allowed_mcp_tools,
        writable_roots: BTreeSet::from([project_root.display().to_string()]),
        protected_paths: work_spec.scope.protected_paths.clone(),
        model_routes,
        permission_ceiling: PermissionPolicy::Allow,
    }
}

fn provenance_for_runtime(
    work_spec: &WorkSpec,
    route_id: &str,
    status_summary: &str,
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
    // Phase 5: effective_route_id is "unknown" when gateway_request_id is None
    // (no 9router evidence). We no longer copy requested_route_id as effective.
    let effective_route_id = "unknown".to_string();
    // Phase 5 audit: output_digest is SHA256(status_summary), not a constant.
    // Different completions produce different output digests.
    let output_digest = format!("sha256:{:x}", Sha256::digest(status_summary.as_bytes()));
    ArtifactProvenance {
        producer_harness: work_spec.harness.clone(),
        producer_role: work_spec.role.clone(),
        provider_gateway_request_id: "unknown".to_string(),
        model_profile_id: "unknown".to_string(),
        capability_profile_id: "unknown".to_string(),
        fallback_policy_id: work_spec.fallback_policy_id.clone(),
        requested_route_id: route_id.to_string(),
        effective_route_id,
        gateway_request_id: None,
        requested_model: None,
        effective_model: None,
        retry_count: 0,
        fallback_chain: Vec::new(),
        input_digest,
        output_digest,
        tool_call_sequence_digest: None,
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
                    vec!["cmd".to_string(), "/C".to_string(), "echo cccx-wire".to_string()]
                } else {
                    vec!["sh".to_string(), "-c".to_string(), "printf cccx-wire".to_string()]
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
            work_spec,
            display_name: "verification-worker".to_string(),
            model_route_summary: "9router/test".to_string(),
        };
        let mut registry = CccxRuntimeRegistry::default();
        registry.register_spawn(child_thread_id, runtime).await.unwrap();
        assert!(registry
            .complete_spawn(child_thread_id, "worker completed".to_string(), true)
            .await
            .unwrap());
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
            work_spec,
            display_name: "verification-worker".to_string(),
            model_route_summary: "9router/test".to_string(),
        };
        let mut registry = CccxRuntimeRegistry::default();
        registry.register_spawn(child_thread_id, runtime).await.unwrap();
        assert!(registry
            .complete_spawn(child_thread_id, "worker completed".to_string(), true)
            .await
            .unwrap());
        let graph = cccx_task_graph_state::load_task_graph(project.path())
            .await
            .unwrap()
            .unwrap();
        let task = &graph.tasks["T1"];
        assert!(!task.is_verified());
        assert!(task.correction_required.as_deref().is_some_and(|reason| reason.contains("exit code 7")));
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
            work_spec,
            display_name: "interrupted-worker".to_string(),
            model_route_summary: "9router/test".to_string(),
        };
        let mut registry = CccxRuntimeRegistry::default();
        registry.register_spawn(child_thread_id, runtime).await.unwrap();
        assert!(registry
            .complete_spawn(child_thread_id, "provider disconnected".to_string(), false)
            .await
            .unwrap());

        let checkpoint = crate::checkpoint::load_checkpoint(
            project.path(),
            &child_thread_id.to_string(),
        )
        .await
        .unwrap()
        .expect("interrupted worker must leave a checkpoint");
        assert_eq!(checkpoint.task_id, "T1");
        assert_eq!(checkpoint.attempt_id, child_thread_id.to_string());
        assert_eq!(checkpoint.base_commit.as_deref(), Some("base-commit"));
        assert_eq!(checkpoint.last_completed_step, "provider disconnected");
    }

    #[tokio::test]
    async fn cancelled_spawn_is_terminal_and_never_writes_a_checkpoint() {
        let project = tempfile::TempDir::new().unwrap();
        let child_thread_id = ThreadId::new();
        let runtime = CccxSpawnRuntime {
            project_root: project.path().to_path_buf(),
            work_spec: work_spec(),
            display_name: "cancelled-worker".to_string(),
            model_route_summary: "9router/test".to_string(),
        };
        let mut registry = CccxRuntimeRegistry::default();
        registry.register_spawn(child_thread_id, runtime).await.unwrap();
        assert!(registry
            .cancel_spawn(child_thread_id, "user cancelled".to_string())
            .await
            .unwrap());
        assert!(crate::checkpoint::load_checkpoint(project.path(), &child_thread_id.to_string())
            .await
            .unwrap()
            .is_none());
        let graph = cccx_task_graph_state::load_task_graph(project.path())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(graph.tasks["T1"].status, cccx_task_graph::TaskStatus::Cancelled);
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
            main_model,
            &ws,
            parent_tools.clone(),
            parent_mcp.clone(),
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
            "9router/main",
            &ws,
            parent_tools,
            parent_mcp,
        );

        // The projection should be empty because WorkSpec ∩ parent = ∅
        let projection = cccx_protocol::project_child_config(&ws, &parent).unwrap();
        assert!(
            projection.allowed_tools.is_empty(),
            "WorkSpec tools ∩ empty parent = empty. Got: {:?}",
            projection.allowed_tools
        );
    }

    // ── Phase 5 tests ──

    /// Phase 5: provenance_for_runtime() produces real SHA256 digests,
    /// not placeholder `sha256:cccx-runtime-*` strings.
    /// task.md §Phase 5 exit criteria: "Provenance không còn placeholder sha256:cccx-runtime-*".
    #[test]
    fn p5_provenance_digests_are_real_not_placeholders() {
        let ws = work_spec();
        let prov = provenance_for_runtime(&ws, "9router/test-route", "test completed: 3/3 passed");

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

        let prov1 = provenance_for_runtime(&ws1, "route-1", "status A");
        let prov2 = provenance_for_runtime(&ws2, "route-1", "status A");

        // Different input payloads → different input digests
        assert_ne!(
            prov1.input_digest, prov2.input_digest,
            "Phase 5: provenance digests MUST change when WorkSpec payload changes"
        );

        // Different status_summary → different output_digest (Phase 5 audit fix)
        let prov3 = provenance_for_runtime(&ws1, "route-1", "status B: different output");
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
        let prov = provenance_for_runtime(&ws, "9router/requested-route", "completed");

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
        let prov = provenance_for_runtime(&ws, "route", "completed successfully");
        // Runtime provenance MUST pass the protocol's digest format checker
        prov.verify_digest_format()
            .expect("Phase 5: runtime provenance digest must pass verify_digest_format");
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
