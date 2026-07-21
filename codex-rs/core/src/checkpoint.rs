use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::fs;

const CHECKPOINT_FILE: &str = "checkpoint.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentCheckpoint {
    pub logical_session_id: String,
    pub worker_incarnation_id: String,
    pub task_id: String,
    pub attempt_id: String,
    /// Pinned cross-harness contract required to create a fresh incarnation.
    /// Old checkpoints intentionally deserialize without it but cannot resume.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_spec: Option<cccx_protocol::WorkSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub last_completed_step: String,
    pub partial_diff: String,
    pub pending_steps: Vec<String>,
    pub checkpointed_at: String,
    /// Git commit at the time the checkpoint was saved. Used to
    /// detect resume-from-checkpoint on a different tree state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_commit: Option<String>,
    /// Working tree digest pinned by WorkSpec.base_revision. When present,
    /// resume must revalidate if the current tree digest differs or cannot be
    /// computed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_working_tree_digest: Option<String>,
    /// Live Main CC capability ceiling captured at the original spawn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_capabilities: Option<cccx_protocol::ParentCapabilitySet>,
    /// Scheduler-side route selection provenance for the pinned WorkSpec.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_route_selection_source: Option<String>,
    /// Rolling digest of successful tool calls completed before this snapshot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_sequence_digest: Option<String>,
}

#[derive(Debug)]
pub enum CheckpointValidation {
    Valid,
    NeedsRevalidation {
        base_commit: String,
        current_head: String,
    },
    Stale {
        base_commit: String,
        reason: String,
    },
}

fn checkpoint_path(project_root: &Path, logical_session_id: &str) -> PathBuf {
    project_root
        .join(".cccx")
        .join("state")
        .join("checkpoints")
        .join(format!("{logical_session_id}_{CHECKPOINT_FILE}"))
}

pub async fn save_checkpoint(
    project_root: &Path,
    checkpoint: &SubagentCheckpoint,
) -> Result<(), std::io::Error> {
    let path = checkpoint_path(project_root, &checkpoint.logical_session_id);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).await?;
    }
    let bytes = serde_json::to_vec_pretty(checkpoint)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err))?;
    fs::write(&path, bytes).await
}

pub async fn load_checkpoint(
    project_root: &Path,
    logical_session_id: &str,
) -> Result<Option<SubagentCheckpoint>, std::io::Error> {
    let path = checkpoint_path(project_root, logical_session_id);
    let bytes = match fs::read(&path).await {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err),
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err))
}

pub async fn delete_checkpoint(
    project_root: &Path,
    logical_session_id: &str,
) -> Result<(), std::io::Error> {
    let path = checkpoint_path(project_root, logical_session_id);
    match fs::remove_file(&path).await {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}

pub fn validate_checkpoint(
    checkpoint: &SubagentCheckpoint,
    current_git_head: &str,
    current_working_tree_digest: Option<&str>,
    max_age_minutes: u32,
) -> CheckpointValidation {
    // Old-format checkpoints (no base_commit) cannot be automatically
    // validated — a resume must be explicitly confirmed.
    // New-format checkpoints with a different base commit also require
    // revalidation because the tree state has changed.
    match &checkpoint.base_commit {
        None => {
            return CheckpointValidation::NeedsRevalidation {
                base_commit: "<missing>".to_string(),
                current_head: current_git_head.to_string(),
            };
        }
        Some(base_commit) if base_commit != current_git_head => {
            return CheckpointValidation::NeedsRevalidation {
                base_commit: base_commit.clone(),
                current_head: current_git_head.to_string(),
            };
        }
        _ => {}
    }
    if let Some(base_digest) = &checkpoint.base_working_tree_digest {
        match current_working_tree_digest {
            Some(current_digest) if current_digest == base_digest => {}
            Some(current_digest) => {
                return CheckpointValidation::NeedsRevalidation {
                    base_commit: format!("working-tree:{base_digest}"),
                    current_head: format!("working-tree:{current_digest}"),
                };
            }
            None => {
                return CheckpointValidation::NeedsRevalidation {
                    base_commit: format!("working-tree:{base_digest}"),
                    current_head: "working-tree:<unavailable>".to_string(),
                };
            }
        }
    }
    let checkpoint_time = chrono::DateTime::parse_from_rfc3339(&checkpoint.checkpointed_at)
        .ok()
        .map(|dt| dt.with_timezone(&chrono::Utc));

    let now = chrono::Utc::now();
    if let Some(ct) = checkpoint_time {
        let age = now.signed_duration_since(ct);
        if age.num_minutes() > max_age_minutes as i64 {
            return CheckpointValidation::Stale {
                base_commit: "unknown".to_string(),
                reason: format!(
                    "checkpoint is {} minutes old (max {})",
                    age.num_minutes(),
                    max_age_minutes
                ),
            };
        }
    }

    if checkpoint.last_completed_step.is_empty() {
        return CheckpointValidation::Stale {
            base_commit: "unknown".to_string(),
            reason: "no steps completed — nothing to resume".to_string(),
        };
    }

    CheckpointValidation::Valid
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn checkpoint_round_trip() {
        let dir = TempDir::new().unwrap();
        let ck = SubagentCheckpoint {
            logical_session_id: "a1-sess".into(),
            worker_incarnation_id: "inc-1".into(),
            task_id: "T1".into(),
            attempt_id: "att-1".into(),
            work_spec: None,
            display_name: None,
            last_completed_step: "applied patch to auth.rs".into(),
            partial_diff: "--- a/auth.rs\n+++ b/auth.rs\n...".into(),
            pending_steps: vec!["run tests".into(), "review diff".into()],
            checkpointed_at: "2026-07-18T12:00:00Z".into(),
            base_commit: None,
            base_working_tree_digest: None,
            parent_capabilities: None,
            model_route_selection_source: None,
            tool_call_sequence_digest: None,
        };
        save_checkpoint(dir.path(), &ck).await.unwrap();
        let loaded = load_checkpoint(dir.path(), "a1-sess")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded.logical_session_id, "a1-sess");
        assert_eq!(loaded.pending_steps, vec!["run tests", "review diff"]);
        delete_checkpoint(dir.path(), "a1-sess").await.unwrap();
        assert!(
            load_checkpoint(dir.path(), "a1-sess")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn checkpoint_with_empty_completed_step_is_not_resumable() {
        let ck = SubagentCheckpoint {
            logical_session_id: "a1-sess".into(),
            worker_incarnation_id: "inc-1".into(),
            task_id: "T1".into(),
            attempt_id: "att-1".into(),
            work_spec: None,
            display_name: None,
            last_completed_step: String::new(),
            partial_diff: String::new(),
            pending_steps: vec!["run tests".into()],
            checkpointed_at: chrono::Utc::now().to_rfc3339(),
            base_commit: Some("head-1".to_string()),
            base_working_tree_digest: None,
            parent_capabilities: None,
            model_route_selection_source: None,
            tool_call_sequence_digest: None,
        };

        assert!(matches!(
            validate_checkpoint(&ck, "head-1", None, 60),
            CheckpointValidation::Stale { reason, .. }
                if reason == "no steps completed — nothing to resume"
        ));
    }

    #[test]
    fn checkpoint_from_different_git_head_requires_revalidation() {
        let ck = SubagentCheckpoint {
            logical_session_id: "a1-sess".into(),
            worker_incarnation_id: "inc-1".into(),
            task_id: "T1".into(),
            attempt_id: "att-1".into(),
            work_spec: None,
            display_name: None,
            last_completed_step: "patched file".into(),
            partial_diff: "--- a/src/lib.rs\n+++ b/src/lib.rs".into(),
            pending_steps: vec!["run tests".into()],
            checkpointed_at: chrono::Utc::now().to_rfc3339(),
            base_commit: Some("old-head".to_string()),
            base_working_tree_digest: None,
            parent_capabilities: None,
            model_route_selection_source: None,
            tool_call_sequence_digest: None,
        };

        assert!(matches!(
            validate_checkpoint(&ck, "new-head", None, 60),
            CheckpointValidation::NeedsRevalidation { .. }
        ));
    }

    // ── Phase 8 tests ──

    /// Phase 8: checkpoint must persist all WorkSpec identity fields
    /// through save→load cycle. Proves the full persistence path works.
    /// task.md: "SubagentStop ghi checkpoint với logicalSessionId/attemptId/baseCommit."
    #[tokio::test]
    async fn p8_checkpoint_round_trip_preserves_workerspec_context() {
        let dir = TempDir::new().unwrap();
        let ck = SubagentCheckpoint {
            logical_session_id: "logical-sess-1".into(),
            worker_incarnation_id: "inc-v2".into(),
            task_id: "T-root".into(),
            attempt_id: "attempt-3".into(),
            work_spec: None,
            display_name: None,
            last_completed_step: "cargo test passes".into(),
            partial_diff: "--- a/src/lib.rs".into(),
            pending_steps: vec!["commit".into()],
            checkpointed_at: chrono::Utc::now().to_rfc3339(),
            base_commit: Some("abc1234".to_string()),
            base_working_tree_digest: Some("sha256:tree".to_string()),
            parent_capabilities: None,
            model_route_selection_source: None,
            tool_call_sequence_digest: None,
        };
        save_checkpoint(dir.path(), &ck).await.unwrap();

        let loaded = load_checkpoint(dir.path(), "logical-sess-1")
            .await
            .unwrap()
            .expect("Phase 8: checkpoint must survive save→load");

        assert_eq!(loaded.logical_session_id, "logical-sess-1");
        assert_eq!(loaded.worker_incarnation_id, "inc-v2");
        assert_eq!(loaded.task_id, "T-root");
        assert_eq!(loaded.attempt_id, "attempt-3");
        assert_eq!(loaded.base_commit.as_deref(), Some("abc1234"));
        assert_eq!(
            loaded.base_working_tree_digest.as_deref(),
            Some("sha256:tree")
        );
        assert_eq!(loaded.pending_steps, vec!["commit"]);
    }

    /// Phase 8: stale checkpoint due to age is rejected.
    /// task.md: "resume loads checkpoint, validates git HEAD drift" — age enforcement too.
    #[test]
    fn p8_checkpoint_stale_due_to_age() {
        let ck = SubagentCheckpoint {
            logical_session_id: "sess".into(),
            worker_incarnation_id: "inc".into(),
            task_id: "T1".into(),
            attempt_id: "att-1".into(),
            work_spec: None,
            display_name: None,
            last_completed_step: "done".into(),
            partial_diff: "diff".into(),
            pending_steps: vec![],
            checkpointed_at: "2026-01-01T00:00:00Z".into(), // months stale
            base_commit: Some("same-head".to_string()),
            base_working_tree_digest: None,
            parent_capabilities: None,
            model_route_selection_source: None,
            tool_call_sequence_digest: None,
        };

        // Even with matching base_commit, a 6+ month old checkpoint is stale
        assert!(matches!(
            validate_checkpoint(&ck, "same-head", None, 30),
            CheckpointValidation::Stale { .. }
        ));
    }

    /// Phase 8: matching base_commit + recent age → valid.
    /// task.md: "resume loads checkpoint, validates git HEAD drift."
    #[test]
    fn p8_checkpoint_matching_head_validates() {
        let ck = SubagentCheckpoint {
            logical_session_id: "sess".into(),
            worker_incarnation_id: "inc".into(),
            task_id: "T1".into(),
            attempt_id: "att-1".into(),
            work_spec: None,
            display_name: None,
            last_completed_step: "tests passing".into(),
            partial_diff: "diff".into(),
            pending_steps: vec!["review".into()],
            checkpointed_at: chrono::Utc::now().to_rfc3339(), // just now
            base_commit: Some("head-abc".to_string()),
            base_working_tree_digest: Some("sha256:tree-a".to_string()),
            parent_capabilities: None,
            model_route_selection_source: None,
            tool_call_sequence_digest: None,
        };

        assert!(matches!(
            validate_checkpoint(&ck, "head-abc", Some("sha256:tree-a"), 60),
            CheckpointValidation::Valid
        ));
    }

    #[test]
    fn p8_checkpoint_working_tree_digest_drift_requires_revalidation() {
        let ck = SubagentCheckpoint {
            logical_session_id: "sess".into(),
            worker_incarnation_id: "inc".into(),
            task_id: "T1".into(),
            attempt_id: "att-1".into(),
            work_spec: None,
            display_name: None,
            last_completed_step: "tests passing".into(),
            partial_diff: "diff".into(),
            pending_steps: vec!["review".into()],
            checkpointed_at: chrono::Utc::now().to_rfc3339(),
            base_commit: Some("head-abc".to_string()),
            base_working_tree_digest: Some("sha256:tree-a".to_string()),
            parent_capabilities: None,
            model_route_selection_source: None,
            tool_call_sequence_digest: None,
        };

        assert!(matches!(
            validate_checkpoint(&ck, "head-abc", Some("sha256:tree-b"), 60),
            CheckpointValidation::NeedsRevalidation { .. }
        ));
        assert!(matches!(
            validate_checkpoint(&ck, "head-abc", None, 60),
            CheckpointValidation::NeedsRevalidation { .. }
        ));
    }

    /// Phase 8: checkpoint without base_commit (old format) requires
    /// explicit revalidation — cannot auto-resume.
    #[test]
    fn p8_checkpoint_missing_base_commit_requires_explicit_revalidation() {
        let ck = SubagentCheckpoint {
            logical_session_id: "sess".into(),
            worker_incarnation_id: "inc".into(),
            task_id: "T1".into(),
            attempt_id: "att-1".into(),
            work_spec: None,
            display_name: None,
            last_completed_step: "done".into(),
            partial_diff: "diff".into(),
            pending_steps: vec![],
            checkpointed_at: chrono::Utc::now().to_rfc3339(),
            base_commit: None, // old format
            base_working_tree_digest: None,
            parent_capabilities: None,
            model_route_selection_source: None,
            tool_call_sequence_digest: None,
        };

        assert!(matches!(
            validate_checkpoint(&ck, "any-head", None, 60),
            CheckpointValidation::NeedsRevalidation { .. }
        ));
    }
}
