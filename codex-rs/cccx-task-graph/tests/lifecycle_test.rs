//! Phase 10: E2E integration test for cccx TaskGraph lifecycle.
//!
//! Proves: Scenario 4 (Spawn CX — WorkSpec required, Artifact returned,
//! TaskGraph updated), Scenario 5 (CX denied — blockedBy enforces order),
//! Scenario 8 (StopHook — can_stop only when ALL verified).

use cccx_protocol::{
    Artifact, ArtifactProvenance, ArtifactStatus, CCCX_ARTIFACT_SCHEMA_VERSION,
    CCCX_VERIFICATION_RECORD_SCHEMA_VERSION, Evidence, EvidenceKind, HarnessKind, RevisionRef,
    VerificationRecord, WorkRole,
};
use cccx_task_graph::{
    AttemptRecord, TaskGraph, TaskNode, TaskStatus, VerificationSpec, WorkerRecord, WorkerStatus,
};
use std::collections::BTreeMap;
use std::collections::BTreeSet;

fn verification_spec(command: &str, digest: &str) -> VerificationSpec {
    VerificationSpec {
        command: Some(cccx_protocol::CommandSpec {
            argv: command.split_whitespace().map(str::to_string).collect(),
            cwd: ".".to_string(),
            timeout_ms: Some(60_000),
        }),
        protected_digests: BTreeMap::new(),
        verification_policy_id: Some("integration".to_string()),
        spec_digest: digest.to_string(),
    }
}

fn worker(role: &str, display_name: &str) -> WorkerRecord {
    WorkerRecord {
        logical_session_id: format!("ls-{display_name}"),
        display_name: display_name.to_string(),
        role: role.to_string(),
        status: WorkerStatus::Running,
        model_route_summary: "9router/main".to_string(),
    }
}

fn attempt(work_spec_id: &str) -> AttemptRecord {
    AttemptRecord {
        attempt_id: format!("attempt-{work_spec_id}"),
        work_spec_id: work_spec_id.to_string(),
        worker: worker("cx:implementer", "worker"),
        artifact_ids: Vec::new(),
        verification_records: Vec::new(),
    }
}

fn success_artifact(art_id: &str, work_spec_id: &str, attempt_id: &str) -> Artifact {
    Artifact {
        schema_version: CCCX_ARTIFACT_SCHEMA_VERSION.to_string(),
        artifact_id: art_id.to_string(),
        task_id: "T1".to_string(),
        work_spec_id: work_spec_id.to_string(),
        attempt_id: attempt_id.to_string(),
        status: ArtifactStatus::Success,
        summary: format!("{art_id} completed"),
        changed_paths: BTreeSet::from(["src/lib.rs".to_string()]),
        changed_files: BTreeSet::from(["src/lib.rs".to_string()]),
        tests_run: Vec::new(),
        residual_risks: Vec::new(),
        follow_up: Vec::new(),
        evidence: vec![Evidence {
            kind: EvidenceKind::TestLog,
            summary: "tests pass".to_string(),
            digest: "sha256:log".to_string(),
        }],
        provenance: Some(ArtifactProvenance {
            producer_harness: HarnessKind::Cx,
            producer_role: WorkRole::Implementer,
            provider_gateway_request_id: "gw-1".to_string(),
            model_profile_id: "9router/main".to_string(),
            capability_profile_id: "coding-tools-v1".to_string(),
            fallback_policy_id: "strict".to_string(),
            requested_route_id: "9router/main".to_string(),
            effective_route_id: "9router/main".to_string(),
            gateway_request_id: Some("gw-1".to_string()),
            requested_model: Some("9router/main".to_string()),
            effective_model: Some("9router/main".to_string()),
            retry_count: 0,
            fallback_chain: Vec::new(),
            input_digest: "sha256:in".to_string(),
            output_digest: "sha256:out".to_string(),
            tool_call_sequence_digest: None,
        }),
        verification: None,
    }
}

fn passed_verification(task_id: &str, attempt_id: &str) -> VerificationRecord {
    VerificationRecord {
        schema_version: CCCX_VERIFICATION_RECORD_SCHEMA_VERSION.to_string(),
        task_id: task_id.to_string(),
        attempt_id: attempt_id.to_string(),
        verifier: "goal-evaluator".to_string(),
        policy_id: "integration".to_string(),
        command: None,
        artifact_digest: "sha256:artifact".to_string(),
        result_revision: RevisionRef {
            git_head: None,
            working_tree_digest: Some("sha256:result".to_string()),
        },
        passed: true,
        checked_at: "2026-07-19T00:00:00Z".to_string(),
        evidence_digest: "sha256:ev".to_string(),
        protected_input_digest: None,
        findings: Vec::new(),
        reason: "verification passed".to_string(),
    }
}

// ── Scenario 4 + 8 combined: Artifact without VerificationRecord does NOT verify ──

#[test]
fn artifact_success_without_verification_does_not_mark_task_verified() {
    let mut graph = TaskGraph::new("e2e-1", "integration test");
    let spec = verification_spec("cargo test", "sha256:spec");

    graph
        .add_task(TaskNode::new("T1", "implement auth fix", spec.clone()))
        .expect("add T1");
    graph
        .start_attempt("T1", attempt("ws-1"))
        .expect("start attempt");

    // Apply an Artifact with Success status — but no VerificationRecord.
    let artifact = success_artifact("art-1", "ws-1", "attempt-ws-1");
    graph
        .apply_artifact("T1", artifact)
        .expect("apply artifact");

    let task = graph.tasks.get("T1").expect("task exists");
    assert_eq!(task.status, TaskStatus::Blocked);
    assert!(!task.is_verified());
    assert!(!graph.can_stop());
}

// ── Scenario 8: verifiedAt is projection from VerificationRecord.passed=true ──

#[test]
fn verification_record_passed_marks_task_verified_and_allows_stop() {
    let mut graph = TaskGraph::new("e2e-2", "verification test");
    let spec = verification_spec("cargo test", "sha256:spec");

    graph
        .add_task(TaskNode::new("T1", "implement fix", spec.clone()))
        .expect("add T1");
    graph
        .start_attempt("T1", attempt("ws-1"))
        .expect("start attempt");

    // Complete: artifact + passed verification
    let artifact = success_artifact("art-1", "ws-1", "attempt-ws-1");
    graph
        .apply_artifact("T1", artifact)
        .expect("apply artifact");
    graph
        .record_verification("T1", "attempt-ws-1", passed_verification("T1", "attempt-ws-1"), "sha256:spec")
        .expect("record verification");

    let task = graph.tasks.get("T1").expect("task exists");
    assert!(task.is_verified());
    assert!(graph.can_stop());
}

// ── Scenario 5: blockedBy enforces dependency ordering ──

#[test]
fn blocked_task_cannot_start_until_dependency_is_verified() {
    let mut graph = TaskGraph::new("e2e-3", "dependency test");
    let spec = verification_spec("cargo test", "sha256:spec");

    // T1 (no deps) + T2 (depends on T1) + T3 (depends on T1)
    let mut t2 = TaskNode::new("T2", "audit fix", spec.clone());
    t2.blocked_by.insert("T1".to_string());
    let mut t3 = TaskNode::new("T3", "release fix", spec.clone());
    t3.blocked_by.insert("T1".to_string());

    graph
        .add_task(TaskNode::new("T1", "impl", spec.clone()))
        .expect("add T1");
    graph.add_task(t2).expect("add T2");
    graph.add_task(t3).expect("add T3");

    // T2 cannot start (T1 unverified)
    assert_eq!(
        graph.start_attempt("T2", attempt("ws-2")),
        Err(cccx_task_graph::TaskGraphError::TaskBlocked(
            "T2".to_string()
        ))
    );

    // Complete T1: artifact + verify
    graph
        .start_attempt("T1", attempt("ws-1"))
        .expect("start T1");
    graph
        .apply_artifact("T1", success_artifact("art-1", "ws-1", "attempt-ws-1"))
        .expect("apply T1 artifact");
    graph
        .record_verification("T1", "attempt-ws-1", passed_verification("T1", "attempt-ws-1"), "sha256:spec")
        .expect("verify T1");

    // Now T2 and T3 can start
    graph
        .start_attempt("T2", attempt("ws-2"))
        .expect("start T2");
    graph
        .start_attempt("T3", attempt("ws-3"))
        .expect("start T3");

    // Complete T2 + T3
    graph
        .apply_artifact("T2", success_artifact("art-2", "ws-2", "attempt-ws-2"))
        .expect("apply T2");
    graph
        .record_verification("T2", "attempt-ws-2", passed_verification("T2", "attempt-ws-2"), "sha256:spec")
        .expect("verify T2");
    graph
        .apply_artifact("T3", success_artifact("art-3", "ws-3", "attempt-ws-3"))
        .expect("apply T3");
    graph
        .record_verification("T3", "attempt-ws-3", passed_verification("T3", "attempt-ws-3"), "sha256:spec")
        .expect("verify T3");

    assert!(graph.can_stop());
    let unverified = graph.unverified_tasks();
    assert!(unverified.is_empty(), "all tasks should be verified");
}

// ── Scenario 8: StopGuard — unverified tasks block completion ──

#[test]
fn stop_is_blocked_when_any_task_remains_unverified() {
    let mut graph = TaskGraph::new("e2e-4", "stop guard test");
    let spec = verification_spec("cargo test", "sha256:spec");

    graph
        .add_task(TaskNode::new("T1", "task one", spec.clone()))
        .expect("add T1");
    graph
        .add_task(TaskNode::new("T2", "task two", spec.clone()))
        .expect("add T2");

    graph
        .start_attempt("T1", attempt("ws-1"))
        .expect("start T1");
    graph
        .apply_artifact("T1", success_artifact("art-1", "ws-1", "attempt-ws-1"))
        .expect("apply T1");
    graph
        .record_verification("T1", "attempt-ws-1", passed_verification("T1", "attempt-ws-1"), "sha256:spec")
        .expect("verify T1");

    // T1 verified, T2 NOT started → can_stop must be false
    assert!(!graph.can_stop());
    assert_eq!(graph.unverified_tasks().len(), 1);

    // Complete T2
    graph
        .start_attempt("T2", attempt("ws-2"))
        .expect("start T2");
    graph
        .apply_artifact("T2", success_artifact("art-2", "ws-2", "attempt-ws-2"))
        .expect("apply T2");
    graph
        .record_verification("T2", "attempt-ws-2", passed_verification("T2", "attempt-ws-2"), "sha256:spec")
        .expect("verify T2");

    // Now can stop
    assert!(graph.can_stop());
    assert!(graph.unverified_tasks().is_empty());
}

// ── Specification change detection ──

#[test]
fn verification_spec_change_blocks_tampered_verification() {
    let mut graph = TaskGraph::new("e2e-5", "tamper test");
    let spec = verification_spec("cargo test", "sha256:original");

    graph
        .add_task(TaskNode::new("T1", "task", spec))
        .expect("add T1");
    graph
        .start_attempt("T1", attempt("ws-1"))
        .expect("start attempt");
    graph
        .apply_artifact("T1", success_artifact("art-1", "ws-1", "attempt-ws-1"))
        .expect("apply artifact");

    // Try verifying with a DIFFERENT spec digest (tampered)
    assert_eq!(
        graph.record_verification(
            "T1",
            "attempt-ws-1",
            passed_verification("T1", "attempt-ws-1"),
            "sha256:tampered"
        ),
        Err(cccx_task_graph::TaskGraphError::VerificationSpecChanged(
            "T1".to_string()
        ))
    );

    // Task remains Blocked (not Verified)
    assert_eq!(graph.tasks.get("T1").unwrap().status, TaskStatus::Blocked);
    assert!(!graph.can_stop());
}
