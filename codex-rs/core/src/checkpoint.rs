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
    pub last_completed_step: String,
    pub partial_diff: String,
    pub pending_steps: Vec<String>,
    pub checkpointed_at: String,
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
    _current_git_head: &str,
    max_age_minutes: u32,
) -> CheckpointValidation {
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
            last_completed_step: "applied patch to auth.rs".into(),
            partial_diff: "--- a/auth.rs\n+++ b/auth.rs\n...".into(),
            pending_steps: vec!["run tests".into(), "review diff".into()],
            checkpointed_at: "2026-07-18T12:00:00Z".into(),
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
            last_completed_step: String::new(),
            partial_diff: String::new(),
            pending_steps: vec!["run tests".into()],
            checkpointed_at: chrono::Utc::now().to_rfc3339(),
        };

        assert!(matches!(
            validate_checkpoint(&ck, "head-1", 60),
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
            last_completed_step: "patched file".into(),
            partial_diff: "--- a/src/lib.rs\n+++ b/src/lib.rs".into(),
            pending_steps: vec!["run tests".into()],
            checkpointed_at: chrono::Utc::now().to_rfc3339(),
        };

        assert!(matches!(
            validate_checkpoint(&ck, "new-head", 60),
            CheckpointValidation::NeedsRevalidation { .. }
        ));
    }
}
