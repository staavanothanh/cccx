use cccx_protocol::CCCX_VERIFICATION_RECORD_SCHEMA_VERSION;
use cccx_protocol::CommandExecution;
use cccx_protocol::RevisionRef;
use cccx_protocol::VerificationRecord;
use cccx_task_graph::TaskNode;

/// Minimal goal evaluator — architecture.md §12.
///
/// MVP behavior: only supports command-based verification (run test command, check exit code).
/// Prose-based evaluation (using a separate model) is deferred.
pub struct GoalEvaluator;

impl GoalEvaluator {
    /// Convert a harness-owned command execution into the only kind of
    /// verification evidence that can mark a task verified.
    pub fn evaluate_execution(
        task: &TaskNode,
        attempt_id: &str,
        command: CommandExecution,
        protected_input_digest: String,
        artifact_digest: String,
    ) -> VerificationRecord {
        let passed = command.exit_code == 0;
        let log_digest = command.log_digest.clone();
        let exit_code = command.exit_code;
        VerificationRecord {
            schema_version: CCCX_VERIFICATION_RECORD_SCHEMA_VERSION.to_string(),
            task_id: task.task_id.clone(),
            attempt_id: attempt_id.to_string(),
            verifier: "cccx-verification-runner".to_string(),
            policy_id: task.verification_spec.spec_digest.clone(),
            command: Some(command),
            artifact_digest,
            result_revision: RevisionRef {
                git_head: None,
                working_tree_digest: None,
            },
            passed,
            checked_at: chrono::Utc::now().to_rfc3339(),
            evidence_digest: log_digest,
            protected_input_digest: Some(protected_input_digest),
            findings: if passed {
                Vec::new()
            } else {
                vec![format!(
                    "verification command failed with exit code {exit_code}"
                )]
            },
            reason: if passed {
                "verification command exited 0".to_string()
            } else {
                format!("verification command failed with exit code {exit_code}")
            },
        }
    }

    /// Evaluate whether a task meets its goal condition.
    /// Returns a VerificationRecord that TaskGraph::record_verification() can use.
    pub fn evaluate(
        task: &TaskNode,
        attempt_id: &str,
        test_exit_code: Option<i32>,
        test_log_digest: Option<&str>,
    ) -> Option<VerificationRecord> {
        let _spec = &task.verification_spec;
        let exit_code = test_exit_code?;
        let log_digest = test_log_digest?;

        let passed = exit_code == 0;

        Some(VerificationRecord {
            schema_version: CCCX_VERIFICATION_RECORD_SCHEMA_VERSION.to_string(),
            task_id: task.task_id.clone(),
            attempt_id: attempt_id.to_string(),
            verifier: "goal-evaluator".to_string(),
            policy_id: task.verification_spec.spec_digest.clone(),
            command: None,
            artifact_digest: log_digest.to_string(),
            result_revision: RevisionRef {
                git_head: None,
                working_tree_digest: None,
            },
            passed,
            checked_at: chrono::Utc::now().to_rfc3339(),
            evidence_digest: log_digest.to_string(),
            protected_input_digest: None,
            findings: if passed {
                Vec::new()
            } else {
                vec![format!(
                    "verification command failed with exit code {exit_code}"
                )]
            },
            reason: if passed {
                "verification command exited 0".to_string()
            } else {
                format!("verification command failed with exit code {exit_code}")
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cccx_task_graph::TaskNode;
    use cccx_task_graph::VerificationSpec;
    use std::collections::BTreeMap;

    fn task() -> TaskNode {
        TaskNode::new(
            "T1",
            "prove the task with command evidence",
            VerificationSpec {
                command: None,
                protected_digests: BTreeMap::new(),
                verification_policy_id: Some("focused-tests".to_string()),
                spec_digest: "sha256:spec".to_string(),
            },
        )
    }

    #[test]
    fn evaluator_requires_command_exit_code_and_log_digest() {
        assert!(GoalEvaluator::evaluate(&task(), "attempt-1", None, Some("sha256:log")).is_none());
        assert!(GoalEvaluator::evaluate(&task(), "attempt-1", Some(0), None).is_none());
    }

    #[test]
    fn evaluator_marks_success_only_from_zero_exit_code() {
        let record = GoalEvaluator::evaluate(&task(), "attempt-1", Some(0), Some("sha256:log"))
            .expect("verification record");

        assert!(record.passed);
        assert_eq!(record.verifier, "goal-evaluator");
        assert_eq!(record.evidence_digest, "sha256:log");
        assert!(record.findings.is_empty());
    }

    #[test]
    fn evaluator_records_failure_as_verification_finding() {
        let record = GoalEvaluator::evaluate(&task(), "attempt-1", Some(101), Some("sha256:log"))
            .expect("verification record");

        assert!(!record.passed);
        assert_eq!(
            record.findings,
            vec!["verification command failed with exit code 101"]
        );
    }

    #[test]
    fn runner_execution_preserves_pinned_command_and_attempt() {
        let record = GoalEvaluator::evaluate_execution(
            &task(),
            "attempt-42",
            CommandExecution {
                argv: vec!["cargo".to_string(), "test".to_string()],
                cwd: "D:/repo".to_string(),
                exit_code: 0,
                started_at: "2026-07-20T00:00:00Z".to_string(),
                finished_at: "2026-07-20T00:00:01Z".to_string(),
                log_digest: "sha256:log".to_string(),
            },
            "sha256:protected".to_string(),
            "sha256:artifact".to_string(),
        );
        assert!(record.passed);
        assert_eq!(record.attempt_id, "attempt-42");
        assert_eq!(record.command.unwrap().argv, vec!["cargo", "test"]);
        assert_eq!(
            record.protected_input_digest.as_deref(),
            Some("sha256:protected")
        );
    }
}
