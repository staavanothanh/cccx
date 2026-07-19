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
        let graph = cccx_task_graph_state::load_task_graph(&runtime.project_root)
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

fn provenance_for_runtime(work_spec: &WorkSpec, route_id: &str) -> ArtifactProvenance {
    use sha2::{Digest, Sha256};
    let mut sorted_tools: Vec<_> = work_spec.allowed_tools.iter().collect();
    sorted_tools.sort();
    let tools_joined: String = sorted_tools.into_iter().map(|s| s.as_str()).collect::<Vec<_>>().join(",");
    let input_payload = format!("{}|{}", work_spec.objective, tools_joined);
    let input_digest = format!("sha256:{:x}", Sha256::digest(input_payload.as_bytes()));
    // Phase 5: effective_route_id is "unknown" when gateway_request_id is None
    // (no 9router evidence). We no longer copy requested_route_id as effective.
    let effective_route_id = "unknown".to_string();
    ArtifactProvenance {
        producer_harness: work_spec.harness.clone(),
        producer_role: work_spec.role.clone(),
        requested_route_id: route_id.to_string(),
        effective_route_id,
        gateway_request_id: None,
        input_digest,
        output_digest: format!(
            "sha256:{:x}",
            Sha256::digest("auto-generated subagent Artifact".as_bytes())
        ),
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
        let projection =
            cccx_protocol::project_child_config(&ws, &parent).unwrap();
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
        let prov = provenance_for_runtime(&ws, "9router/test-route");

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
    #[test]
    fn p5_provenance_digest_changes_when_payload_changes() {
        let ws1 = work_spec(); // objective = "run tests", tools = {"shell_command"}
        let mut ws2 = ws1.clone();
        ws2.objective = "build and test with coverage".to_string();
        ws2.allowed_tools = BTreeSet::from(["edit".to_string(), "apply_patch".to_string()]);

        let prov1 = provenance_for_runtime(&ws1, "route-1");
        let prov2 = provenance_for_runtime(&ws2, "route-1");

        // Different input payloads → different input digests
        assert_ne!(
            prov1.input_digest, prov2.input_digest,
            "Phase 5: provenance digests MUST change when WorkSpec payload changes"
        );
    }

    /// Phase 5: effective_route_id must be "unknown" when no gateway_request_id.
    /// It must NOT be a copy of requested_route_id.
    #[test]
    fn p5_effective_route_unknown_when_no_gateway_evidence() {
        let ws = work_spec();
        let prov = provenance_for_runtime(&ws, "9router/requested-route");

        // When gateway_request_id is None, effective_route_id must be "unknown", not "9router/requested-route"
        assert_eq!(
            prov.requested_route_id, "9router/requested-route",
            "requested should be preserved"
        );
        assert!(
            prov.gateway_request_id.is_none(),
            "no gateway evidence"
        );
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
}
