use cccx_protocol::CCCX_VERIFICATION_RECORD_SCHEMA_VERSION;
use cccx_protocol::RevisionRef;
use cccx_protocol::VerificationRecord;
use cccx_task_graph::TaskNode;

/// Minimal goal evaluator — architecture.md §12.
///
/// MVP behavior: only supports command-based verification (run test command, check exit code).
/// Prose-based evaluation (using a separate model) is deferred.
pub struct GoalEvaluator;

impl GoalEvaluator {
    /// Evaluate whether a task meets its goal condition.
    /// Returns a VerificationRecord that TaskGraph::record_verification() can use.
    pub fn evaluate(
        task: &TaskNode,
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
            attempt_id: "unknown".to_string(),
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
    use std::collections::BTreeSet;

    fn task() -> TaskNode {
        TaskNode::new(
            "T1",
            "prove the task with command evidence",
            VerificationSpec {
                command: "cargo test -p cccx-task-graph".to_string(),
                protected_paths: BTreeSet::new(),
                spec_digest: "sha256:spec".to_string(),
            },
        )
    }

    #[test]
    fn evaluator_requires_command_exit_code_and_log_digest() {
        assert!(GoalEvaluator::evaluate(&task(), None, Some("sha256:log")).is_none());
        assert!(GoalEvaluator::evaluate(&task(), Some(0), None).is_none());
    }

    #[test]
    fn evaluator_marks_success_only_from_zero_exit_code() {
        let record = GoalEvaluator::evaluate(&task(), Some(0), Some("sha256:log"))
            .expect("verification record");

        assert!(record.passed);
        assert_eq!(record.verifier, "goal-evaluator");
        assert_eq!(record.evidence_digest, "sha256:log");
        assert!(record.findings.is_empty());
    }

    #[test]
    fn evaluator_records_failure_as_verification_finding() {
        let record = GoalEvaluator::evaluate(&task(), Some(101), Some("sha256:log"))
            .expect("verification record");

        assert!(!record.passed);
        assert_eq!(
            record.findings,
            vec!["verification command failed with exit code 101"]
        );
    }
}
