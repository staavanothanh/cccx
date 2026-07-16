//! Persistent cccx Task Graph projection.
//!
//! This crate keeps the Task Graph lifecycle independent from `codex-core` so
//! verification invariants can be tested quickly before runtime wiring.

use cccx_protocol::Artifact;
use cccx_protocol::ArtifactStatus;
use cccx_protocol::VerificationRecord;
use serde::Deserialize;
use serde::Serialize;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use thiserror::Error;

pub const CCCX_TASK_GRAPH_SCHEMA_VERSION: &str = "cccx.task_graph.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    InProgress,
    Blocked,
    Interrupted,
    Failed,
    Verified,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerStatus {
    Running,
    Waiting,
    Interrupted,
    Resumable,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerificationSpec {
    pub command: String,
    #[serde(default)]
    pub protected_paths: BTreeSet<String>,
    pub spec_digest: String,
}

impl VerificationSpec {
    pub fn digest_matches(&self, observed_digest: &str) -> bool {
        self.spec_digest == observed_digest
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerRecord {
    pub logical_session_id: String,
    pub display_name: String,
    pub role: String,
    pub status: WorkerStatus,
    pub model_route_summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttemptRecord {
    pub attempt_id: String,
    pub work_spec_id: String,
    pub worker: WorkerRecord,
    #[serde(default)]
    pub artifact_ids: Vec<String>,
    #[serde(default)]
    pub verification_records: Vec<VerificationRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskNode {
    pub task_id: String,
    pub objective: String,
    #[serde(default)]
    pub blocked_by: BTreeSet<String>,
    pub status: TaskStatus,
    pub verification_spec: VerificationSpec,
    #[serde(default)]
    pub attempts: Vec<AttemptRecord>,
    pub verified_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskGraph {
    pub schema_version: String,
    pub graph_id: String,
    pub title: String,
    #[serde(default)]
    pub tasks: BTreeMap<String, TaskNode>,
    #[serde(default)]
    pub artifacts: BTreeMap<String, Artifact>,
}

impl TaskGraph {
    pub fn new(graph_id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            schema_version: CCCX_TASK_GRAPH_SCHEMA_VERSION.to_string(),
            graph_id: graph_id.into(),
            title: title.into(),
            tasks: BTreeMap::new(),
            artifacts: BTreeMap::new(),
        }
    }

    pub fn add_task(&mut self, task: TaskNode) -> Result<(), TaskGraphError> {
        if self.tasks.contains_key(&task.task_id) {
            return Err(TaskGraphError::DuplicateTask(task.task_id));
        }
        for dependency in &task.blocked_by {
            if !self.tasks.contains_key(dependency) {
                return Err(TaskGraphError::MissingDependency {
                    task_id: task.task_id.clone(),
                    dependency: dependency.clone(),
                });
            }
        }
        self.tasks.insert(task.task_id.clone(), task);
        Ok(())
    }

    pub fn start_attempt(
        &mut self,
        task_id: &str,
        attempt: AttemptRecord,
    ) -> Result<(), TaskGraphError> {
        let blocked_by = self
            .tasks
            .get(task_id)
            .ok_or_else(|| TaskGraphError::UnknownTask(task_id.to_string()))?
            .blocked_by
            .clone();

        if !blocked_by.iter().all(|dependency| {
            self.tasks
                .get(dependency)
                .is_some_and(TaskNode::is_verified)
        }) {
            return Err(TaskGraphError::TaskBlocked(task_id.to_string()));
        }

        let task = self
            .tasks
            .get_mut(task_id)
            .ok_or_else(|| TaskGraphError::UnknownTask(task_id.to_string()))?;

        task.status = TaskStatus::InProgress;
        task.attempts.push(attempt);
        Ok(())
    }

    pub fn apply_artifact(
        &mut self,
        task_id: &str,
        artifact: Artifact,
    ) -> Result<(), TaskGraphError> {
        artifact
            .validate()
            .map_err(|error| TaskGraphError::InvalidArtifact(error.to_string()))?;
        let task = self
            .tasks
            .get_mut(task_id)
            .ok_or_else(|| TaskGraphError::UnknownTask(task_id.to_string()))?;

        let attempt_index = task
            .attempts
            .iter()
            .position(|attempt| attempt.attempt_id == artifact.attempt_id)
            .ok_or_else(|| TaskGraphError::UnknownAttempt(artifact.attempt_id.clone()))?;

        task.attempts[attempt_index]
            .artifact_ids
            .push(artifact.artifact_id.clone());

        task.status = match artifact.status {
            ArtifactStatus::Success => TaskStatus::Blocked,
            ArtifactStatus::Blocked => TaskStatus::Blocked,
            ArtifactStatus::Interrupted => TaskStatus::Interrupted,
            ArtifactStatus::Failed => TaskStatus::Failed,
        };

        self.artifacts
            .insert(artifact.artifact_id.clone(), artifact);
        Ok(())
    }

    pub fn record_verification(
        &mut self,
        task_id: &str,
        attempt_id: &str,
        record: VerificationRecord,
        observed_spec_digest: &str,
    ) -> Result<(), TaskGraphError> {
        record
            .validate()
            .map_err(|error| TaskGraphError::InvalidVerification(error.to_string()))?;

        let task = self
            .tasks
            .get_mut(task_id)
            .ok_or_else(|| TaskGraphError::UnknownTask(task_id.to_string()))?;

        if !task.verification_spec.digest_matches(observed_spec_digest) {
            task.status = TaskStatus::Blocked;
            return Err(TaskGraphError::VerificationSpecChanged(task_id.to_string()));
        }

        let attempt_index = task
            .attempts
            .iter()
            .position(|attempt| attempt.attempt_id == attempt_id)
            .ok_or_else(|| TaskGraphError::UnknownAttempt(attempt_id.to_string()))?;

        if record.passed {
            task.status = TaskStatus::Verified;
            task.verified_at = Some(record.checked_at.clone());
        } else {
            task.status = TaskStatus::Blocked;
        }

        task.attempts[attempt_index]
            .verification_records
            .push(record);
        Ok(())
    }

    pub fn can_stop(&self) -> bool {
        !self.tasks.is_empty() && self.tasks.values().all(TaskNode::is_verified)
    }

    pub fn unverified_tasks(&self) -> Vec<&TaskNode> {
        self.tasks
            .values()
            .filter(|task| !task.is_verified())
            .collect()
    }

    pub fn validate(&self) -> Result<(), TaskGraphError> {
        if self.schema_version != CCCX_TASK_GRAPH_SCHEMA_VERSION {
            return Err(TaskGraphError::UnsupportedSchemaVersion(
                self.schema_version.clone(),
            ));
        }
        for task in self.tasks.values() {
            for dependency in &task.blocked_by {
                if !self.tasks.contains_key(dependency) {
                    return Err(TaskGraphError::MissingDependency {
                        task_id: task.task_id.clone(),
                        dependency: dependency.clone(),
                    });
                }
            }
        }
        Ok(())
    }
}

impl TaskNode {
    pub fn new(
        task_id: impl Into<String>,
        objective: impl Into<String>,
        verification_spec: VerificationSpec,
    ) -> Self {
        Self {
            task_id: task_id.into(),
            objective: objective.into(),
            blocked_by: BTreeSet::new(),
            status: TaskStatus::Pending,
            verification_spec,
            attempts: Vec::new(),
            verified_at: None,
        }
    }

    pub fn is_verified(&self) -> bool {
        matches!(self.status, TaskStatus::Verified) && self.verified_at.is_some()
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TaskGraphError {
    #[error("unsupported task graph schema version `{0}`")]
    UnsupportedSchemaVersion(String),
    #[error("duplicate task `{0}`")]
    DuplicateTask(String),
    #[error("unknown task `{0}`")]
    UnknownTask(String),
    #[error("task `{task_id}` references missing dependency `{dependency}`")]
    MissingDependency { task_id: String, dependency: String },
    #[error("task `{0}` is blocked")]
    TaskBlocked(String),
    #[error("unknown attempt `{0}`")]
    UnknownAttempt(String),
    #[error("invalid artifact: {0}")]
    InvalidArtifact(String),
    #[error("invalid verification record: {0}")]
    InvalidVerification(String),
    #[error("verification spec changed for task `{0}`")]
    VerificationSpecChanged(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use cccx_protocol::ArtifactProvenance;
    use cccx_protocol::CCCX_PROTOCOL_SCHEMA_VERSION;
    use cccx_protocol::Evidence;
    use cccx_protocol::EvidenceKind;
    use cccx_protocol::HarnessKind;
    use cccx_protocol::WorkRole;
    use pretty_assertions::assert_eq;

    fn set(values: &[&str]) -> BTreeSet<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    fn verification_spec() -> VerificationSpec {
        VerificationSpec {
            command: "cargo test -p cccx-task-graph".to_string(),
            protected_paths: set(&["codex-rs/cccx-task-graph/src/lib.rs"]),
            spec_digest: "sha256:verification-spec".to_string(),
        }
    }

    fn worker() -> WorkerRecord {
        WorkerRecord {
            logical_session_id: "ls-a1".to_string(),
            display_name: "a1-task".to_string(),
            role: "cx:implementer".to_string(),
            status: WorkerStatus::Running,
            model_route_summary: "inherit-main-requested".to_string(),
        }
    }

    fn attempt() -> AttemptRecord {
        AttemptRecord {
            attempt_id: "attempt-1".to_string(),
            work_spec_id: "ws-1".to_string(),
            worker: worker(),
            artifact_ids: Vec::new(),
            verification_records: Vec::new(),
        }
    }

    fn success_artifact() -> Artifact {
        Artifact {
            schema_version: CCCX_PROTOCOL_SCHEMA_VERSION.to_string(),
            artifact_id: "artifact-1".to_string(),
            work_spec_id: "ws-1".to_string(),
            attempt_id: "attempt-1".to_string(),
            status: ArtifactStatus::Success,
            summary: "claimed done".to_string(),
            changed_paths: set(&["codex-rs/cccx-task-graph/src/lib.rs"]),
            evidence: vec![Evidence {
                kind: EvidenceKind::TestLog,
                summary: "test log".to_string(),
                digest: "sha256:test-log".to_string(),
            }],
            provenance: Some(ArtifactProvenance {
                producer_harness: HarnessKind::Cx,
                producer_role: WorkRole::Implementer,
                requested_route_id: "9router/main".to_string(),
                effective_route_id: "9router/main".to_string(),
                gateway_request_id: Some("gw-1".to_string()),
                input_digest: "sha256:input".to_string(),
                output_digest: "sha256:output".to_string(),
            }),
            verification: None,
        }
    }

    fn passed_verification() -> VerificationRecord {
        VerificationRecord {
            verifier: "cccx:self".to_string(),
            passed: true,
            checked_at: "2026-07-16T00:00:00Z".to_string(),
            evidence_digest: "sha256:test-log".to_string(),
            findings: Vec::new(),
        }
    }

    fn graph_with_one_started_task() -> TaskGraph {
        let mut graph = TaskGraph::new("graph-1", "M5");
        graph
            .add_task(TaskNode::new(
                "T1",
                "implement task graph",
                verification_spec(),
            ))
            .expect("add task");
        graph.start_attempt("T1", attempt()).expect("start attempt");
        graph
    }

    #[test]
    fn task_graph_round_trips_as_json_and_validates() {
        let graph = graph_with_one_started_task();

        let encoded = serde_json::to_string_pretty(&graph).expect("serialize");
        let decoded: TaskGraph = serde_json::from_str(&encoded).expect("deserialize");

        assert_eq!(decoded, graph);
        decoded.validate().expect("valid graph");
    }

    #[test]
    fn cx_success_artifact_without_verification_does_not_verify_task() {
        let mut graph = graph_with_one_started_task();

        graph
            .apply_artifact("T1", success_artifact())
            .expect("apply artifact");

        let task = graph.tasks.get("T1").expect("task exists");
        assert_eq!(task.status, TaskStatus::Blocked);
        assert!(!task.is_verified());
        assert!(!graph.can_stop());
    }

    #[test]
    fn passed_verification_marks_task_verified_and_allows_stop() {
        let mut graph = graph_with_one_started_task();
        graph
            .apply_artifact("T1", success_artifact())
            .expect("apply artifact");
        graph
            .record_verification(
                "T1",
                "attempt-1",
                passed_verification(),
                "sha256:verification-spec",
            )
            .expect("record verification");

        let task = graph.tasks.get("T1").expect("task exists");
        assert!(task.is_verified());
        assert!(graph.can_stop());
    }

    #[test]
    fn protected_verification_spec_change_blocks_verification() {
        let mut graph = graph_with_one_started_task();
        graph
            .apply_artifact("T1", success_artifact())
            .expect("apply artifact");

        assert_eq!(
            graph.record_verification(
                "T1",
                "attempt-1",
                passed_verification(),
                "sha256:tampered-verification-spec",
            ),
            Err(TaskGraphError::VerificationSpecChanged("T1".to_string()))
        );
        assert_eq!(
            graph.tasks.get("T1").map(|task| &task.status),
            Some(&TaskStatus::Blocked)
        );
        assert!(!graph.can_stop());
    }

    #[test]
    fn blocked_dependency_prevents_attempt_start() {
        let mut graph = TaskGraph::new("graph-1", "deps");
        graph
            .add_task(TaskNode::new("T1", "first", verification_spec()))
            .expect("add T1");
        let mut second = TaskNode::new("T2", "second", verification_spec());
        second.blocked_by.insert("T1".to_string());
        graph.add_task(second).expect("add T2");

        assert_eq!(
            graph.start_attempt("T2", attempt()),
            Err(TaskGraphError::TaskBlocked("T2".to_string()))
        );
    }

    #[test]
    fn compact_or_clear_context_does_not_lose_task_graph_when_json_is_preserved() {
        let mut graph = graph_with_one_started_task();
        graph
            .apply_artifact("T1", success_artifact())
            .expect("apply artifact");

        let persisted = serde_json::to_string(&graph).expect("persist graph");
        let restored: TaskGraph = serde_json::from_str(&persisted).expect("restore graph");

        assert_eq!(restored.unverified_tasks().len(), 1);
        assert_eq!(restored.artifacts.len(), 1);
        assert!(!restored.can_stop());
    }
}
