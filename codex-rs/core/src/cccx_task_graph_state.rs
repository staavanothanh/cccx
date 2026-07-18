//! cccx Task Graph persistence and deterministic stop guard.
//!
//! This module is the core-side wiring point for `cccx-task-graph`. Session
//! runtime hooks call into this module instead of reimplementing persistence or
//! completion semantics.
//!
//! TODO(cccx-phase6): Wire load/save/stop_guard into session lifecycle.
//! Currently tested through unit tests; runtime integration deferred to
//! the subagent/session wiring milestone.

#![allow(dead_code)]

use std::path::Path;
use std::path::PathBuf;

use cccx_mode::CccxModeState;
use cccx_task_graph::TaskGraph;
use thiserror::Error;
use tokio::fs;

const CCCX_STATE_DIR: &str = ".cccx/state";
const TASK_GRAPH_FILE: &str = "task-graph.json";

#[derive(Debug, Error)]
pub enum TaskGraphStateError {
    #[error("failed to read task graph at {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to write task graph at {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to create task graph state directory at {path}: {source}")]
    CreateDir {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to parse task graph at {path}: {source}")]
    Parse {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("failed to serialize task graph: {0}")]
    Serialize(serde_json::Error),
    #[error("invalid task graph: {0}")]
    InvalidGraph(String),
    #[error("task graph is not complete; unverified tasks: {0:?}")]
    StopBlocked(Vec<String>),
}

pub fn task_graph_path(project_root: &Path) -> PathBuf {
    project_root.join(CCCX_STATE_DIR).join(TASK_GRAPH_FILE)
}

pub async fn load_task_graph(
    project_root: &Path,
) -> Result<Option<TaskGraph>, TaskGraphStateError> {
    let path = task_graph_path(project_root);
    let bytes = match fs::read(&path).await {
        Ok(bytes) => bytes,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(TaskGraphStateError::Read { path, source }),
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|source| TaskGraphStateError::Parse { path, source })
}

pub async fn save_task_graph(
    project_root: &Path,
    task_graph: &TaskGraph,
) -> Result<(), TaskGraphStateError> {
    task_graph
        .validate()
        .map_err(|error| TaskGraphStateError::InvalidGraph(error.to_string()))?;

    let path = task_graph_path(project_root);
    let Some(parent) = path.parent() else {
        return Err(TaskGraphStateError::Write {
            path,
            source: std::io::Error::other("task graph path has no parent"),
        });
    };
    fs::create_dir_all(parent)
        .await
        .map_err(|source| TaskGraphStateError::CreateDir {
            path: parent.to_path_buf(),
            source,
        })?;
    let bytes = serde_json::to_vec_pretty(task_graph).map_err(TaskGraphStateError::Serialize)?;
    fs::write(&path, bytes)
        .await
        .map_err(|source| TaskGraphStateError::Write { path, source })
}

pub fn stop_guard(task_graph: &TaskGraph) -> Result<(), TaskGraphStateError> {
    if task_graph.can_stop() {
        Ok(())
    } else {
        Err(TaskGraphStateError::StopBlocked(
            task_graph
                .unverified_tasks()
                .into_iter()
                .map(|task| task.task_id.clone())
                .collect(),
        ))
    }
}

pub fn compaction_survivor_json(task_graph: &TaskGraph) -> Result<String, TaskGraphStateError> {
    serde_json::to_string(task_graph).map_err(TaskGraphStateError::Serialize)
}

// --- Mode State Persistence ---

const MODE_STATE_FILE: &str = "mode_state.json";

fn mode_state_path(project_root: &Path) -> PathBuf {
    project_root.join(CCCX_STATE_DIR).join(MODE_STATE_FILE)
}

pub async fn load_mode_state(
    project_root: &Path,
) -> Result<Option<CccxModeState>, TaskGraphStateError> {
    let path = mode_state_path(project_root);
    let bytes = match fs::read(&path).await {
        Ok(bytes) => bytes,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(TaskGraphStateError::Read { path, source }),
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|source| TaskGraphStateError::Parse { path, source })
}

pub async fn save_mode_state(
    project_root: &Path,
    mode: &CccxModeState,
) -> Result<(), TaskGraphStateError> {
    let path = mode_state_path(project_root);
    let Some(parent) = path.parent() else {
        return Err(TaskGraphStateError::Write {
            path,
            source: std::io::Error::other("mode state path has no parent"),
        });
    };
    fs::create_dir_all(parent)
        .await
        .map_err(|source| TaskGraphStateError::CreateDir {
            path: parent.to_path_buf(),
            source,
        })?;
    let bytes = serde_json::to_vec_pretty(mode).map_err(TaskGraphStateError::Serialize)?;
    fs::write(&path, bytes)
        .await
        .map_err(|source| TaskGraphStateError::Write { path, source })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use cccx_task_graph::TaskNode;
    use cccx_task_graph::TaskStatus;
    use cccx_task_graph::VerificationSpec;
    use pretty_assertions::assert_eq;
    use tempfile::TempDir;

    use super::*;

    fn verification_spec() -> VerificationSpec {
        VerificationSpec {
            command: "cargo test -p cccx-task-graph".to_string(),
            protected_paths: BTreeSet::new(),
            spec_digest: "sha256:spec".to_string(),
        }
    }

    #[test]
    fn stop_guard_blocks_unverified_tasks() {
        let mut graph = TaskGraph::new("graph-1", "test");
        graph
            .add_task(TaskNode::new("T1", "unverified", verification_spec()))
            .expect("add task");

        assert!(matches!(
            stop_guard(&graph),
            Err(TaskGraphStateError::StopBlocked(tasks)) if tasks == vec!["T1".to_string()]
        ));
    }

    #[test]
    fn stop_guard_allows_verified_graph() {
        let mut graph = TaskGraph::new("graph-1", "test");
        let mut task = TaskNode::new("T1", "verified", verification_spec());
        task.status = TaskStatus::Verified;
        task.verified_at = Some("2026-07-16T00:00:00Z".to_string());
        graph.add_task(task).expect("add task");

        assert!(stop_guard(&graph).is_ok());
    }

    #[tokio::test]
    async fn task_graph_persists_under_cccx_state_dir() {
        let temp_dir = TempDir::new().expect("temp dir");
        let graph = TaskGraph::new("graph-1", "persist");

        save_task_graph(temp_dir.path(), &graph)
            .await
            .expect("save graph");
        let restored = load_task_graph(temp_dir.path())
            .await
            .expect("load graph")
            .expect("graph exists");

        assert_eq!(restored, graph);
        assert!(task_graph_path(temp_dir.path()).ends_with(".cccx/state/task-graph.json"));
    }
}
