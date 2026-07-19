//! cccx Task Graph persistence and deterministic stop guard.
//!
//! This module is the core-side wiring point for `cccx-task-graph`. Session
//! runtime hooks call into this module instead of reimplementing persistence or
//! completion semantics.

use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;

use cccx_mode::CccxModeState;
use cccx_task_graph::ActiveAttemptMetadata;
use cccx_task_graph::TaskGraph;
use thiserror::Error;
use tokio::fs;

const CCCX_STATE_DIR: &str = ".cccx/state";
const TASK_GRAPH_FILE: &str = "task-graph.json";
static STATE_WRITE_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static PROJECT_WRITERS: OnceLock<Mutex<BTreeMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>> =
    OnceLock::new();

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
    #[error("task graph mutation rejected: {0}")]
    Mutation(String),
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
    atomic_replace(&path, &bytes).await
}

/// Execute one complete TaskGraph read-modify-write transaction under the
/// project-local writer lease. Callers must use this for lifecycle mutations;
/// a standalone `load` followed by `save` can otherwise lose a sibling update.
pub async fn update_task_graph<T, F>(
    project_root: &Path,
    initial: TaskGraph,
    mutate: F,
) -> Result<T, TaskGraphStateError>
where
    F: FnOnce(&mut TaskGraph) -> Result<T, TaskGraphStateError>,
{
    let writer = project_writer(project_root);
    let _lease = writer.lock().await;
    let mut graph = load_task_graph(project_root).await?.unwrap_or(initial);
    let result = mutate(&mut graph)?;
    save_task_graph(project_root, &graph).await?;
    Ok(result)
}

/// Persist a user-declared completion condition for one concrete cccx task.
/// This does not verify the task; only `record_verification` can do that.
pub async fn set_task_goal_condition(
    project_root: &Path,
    task_id: &str,
    condition: String,
) -> Result<(), TaskGraphStateError> {
    let initial = TaskGraph::new("cccx-runtime", "cccx runtime graph");
    update_task_graph(project_root, initial, |graph| {
        graph
            .set_goal_condition(task_id, condition)
            .map_err(|error| TaskGraphStateError::Mutation(error.to_string()))
    })
    .await
}

/// Read active worker metadata from the same persisted TaskGraph transaction
/// source rather than duplicating it in an independently drifting state file.
pub async fn load_active_attempts(
    project_root: &Path,
) -> Result<Vec<ActiveAttemptMetadata>, TaskGraphStateError> {
    Ok(load_task_graph(project_root)
        .await?
        .map(|graph| graph.active_attempts())
        .unwrap_or_default())
}

fn project_writer(project_root: &Path) -> Arc<tokio::sync::Mutex<()>> {
    let key = project_root.to_path_buf();
    let writers = PROJECT_WRITERS.get_or_init(|| Mutex::new(BTreeMap::new()));
    let mut writers = writers.lock().expect("cccx project writer map poisoned");
    writers
        .entry(key)
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone()
}

async fn atomic_replace(path: &Path, bytes: &[u8]) -> Result<(), TaskGraphStateError> {
    let sequence = STATE_WRITE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = path.with_file_name(format!(
        ".{}.{}.tmp",
        path.file_name().and_then(|name| name.to_str()).unwrap_or("state"),
        sequence
    ));
    fs::write(&temporary, bytes)
        .await
        .map_err(|source| TaskGraphStateError::Write {
            path: temporary.clone(),
            source,
        })?;
    fs::OpenOptions::new()
        .write(true)
        .open(&temporary)
        .await
        .map_err(|source| TaskGraphStateError::Write {
            path: temporary.clone(),
            source,
        })?
        .sync_all()
        .await
        .map_err(|source| TaskGraphStateError::Write {
            path: temporary.clone(),
            source,
        })?;
    fs::rename(&temporary, path)
        .await
        .map_err(|source| TaskGraphStateError::Write {
            path: path.to_path_buf(),
            source,
        })
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
    // Mode and TaskGraph make up one compaction-survivor state boundary. Use
    // the same per-project lease as graph mutations so a mode write cannot
    // interleave with a lifecycle transaction for the same project.
    let writer = project_writer(project_root);
    let _lease = writer.lock().await;
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
    atomic_replace(&path, &bytes).await
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use cccx_task_graph::TaskNode;
    use cccx_task_graph::TaskStatus;
    use cccx_task_graph::VerificationSpec;
    use cccx_policy::MainMode;
    use pretty_assertions::assert_eq;
    use tempfile::TempDir;

    use super::*;

    fn verification_spec() -> VerificationSpec {
        VerificationSpec {
            command: None,
            protected_digests: BTreeMap::new(),
            verification_policy_id: Some("focused-tests".to_string()),
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

    #[tokio::test]
    async fn incomplete_temporary_write_leaves_previous_task_graph_readable() {
        let temp_dir = TempDir::new().expect("temp dir");
        let graph = TaskGraph::new("graph-1", "persist");
        save_task_graph(temp_dir.path(), &graph).await.expect("save graph");
        let path = task_graph_path(temp_dir.path());
        let temporary = path.with_file_name(".task-graph.json.crash.tmp");
        fs::write(&temporary, b"{ incomplete").await.expect("write interrupted temp");

        assert_eq!(
            load_task_graph(temp_dir.path())
                .await
                .expect("load graph")
                .expect("existing graph"),
            graph,
            "an interrupted temporary write must not corrupt the previously committed graph"
        );
    }

    #[tokio::test]
    async fn concurrent_transactions_do_not_lose_sibling_tasks() {
        let temp_dir = TempDir::new().expect("temp dir");
        let initial = TaskGraph::new("graph-1", "concurrent state");
        let first = update_task_graph(temp_dir.path(), initial.clone(), |graph| {
            graph
                .add_task(TaskNode::new("T1", "first", verification_spec()))
                .map_err(|error| TaskGraphStateError::Mutation(error.to_string()))
        });
        let second = update_task_graph(temp_dir.path(), initial, |graph| {
            graph
                .add_task(TaskNode::new("T2", "second", verification_spec()))
                .map_err(|error| TaskGraphStateError::Mutation(error.to_string()))
        });
        let (first, second) = tokio::join!(first, second);
        first.expect("first transaction");
        second.expect("second transaction");
        let graph = load_task_graph(temp_dir.path()).await.unwrap().unwrap();
        assert!(graph.tasks.contains_key("T1"));
        assert!(graph.tasks.contains_key("T2"));
    }

    #[tokio::test]
    async fn task_goal_condition_is_persisted_without_verifying_the_task() {
        let temp_dir = TempDir::new().expect("temp dir");
        let mut graph = TaskGraph::new("graph-1", "goal");
        graph
            .add_task(TaskNode::new("T1", "first", verification_spec()))
            .expect("add task");
        save_task_graph(temp_dir.path(), &graph).await.expect("save graph");
        set_task_goal_condition(
            temp_dir.path(),
            "T1",
            "focused tests must pass".to_string(),
        )
        .await
        .expect("set goal");
        let restored = load_task_graph(temp_dir.path()).await.unwrap().unwrap();
        let task = &restored.tasks["T1"];
        assert_eq!(task.goal_condition.as_deref(), Some("focused tests must pass"));
        assert_eq!(task.goal_condition_met, None);
        assert!(!task.is_verified());
    }

    #[tokio::test]
    async fn active_attempts_are_loaded_from_the_task_graph_store() {
        let temp_dir = TempDir::new().expect("temp dir");
        let mut graph = TaskGraph::new("graph-1", "attempts");
        graph
            .add_task(TaskNode::new("T1", "first", verification_spec()))
            .expect("add task");
        graph
            .start_attempt(
                "T1",
                cccx_task_graph::AttemptRecord {
                    attempt_id: "attempt-1".to_string(),
                    work_spec_id: "ws-1".to_string(),
                    worker: cccx_task_graph::WorkerRecord {
                        logical_session_id: "logical-1".to_string(),
                        display_name: "worker-1".to_string(),
                        role: "Implementer".to_string(),
                        status: cccx_task_graph::WorkerStatus::Running,
                        model_route_summary: "9router/test".to_string(),
                    },
                    artifact_ids: Vec::new(),
                    verification_records: Vec::new(),
                },
            )
            .expect("start attempt");
        save_task_graph(temp_dir.path(), &graph).await.expect("save graph");
        let active = load_active_attempts(temp_dir.path()).await.expect("load active");
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].attempt_id, "attempt-1");
        assert_eq!(active[0].logical_session_id, "logical-1");
    }

    #[tokio::test]
    async fn mode_and_task_graph_share_a_safe_project_write_boundary() {
        let temp_dir = TempDir::new().expect("temp dir");
        let graph = TaskGraph::new("graph-1", "shared state");
        let graph_write = update_task_graph(temp_dir.path(), graph, |graph| {
            graph
                .add_task(TaskNode::new("T1", "first", verification_spec()))
                .map_err(|error| TaskGraphStateError::Mutation(error.to_string()))
        });
        let mode = CccxModeState::new(MainMode::Auto);
        let mode_write = save_mode_state(temp_dir.path(), &mode);

        let (graph_write, mode_write) = tokio::join!(graph_write, mode_write);
        graph_write.expect("graph transaction");
        mode_write.expect("mode transaction");
        assert!(load_task_graph(temp_dir.path())
            .await
            .expect("load graph")
            .expect("graph exists")
            .tasks
            .contains_key("T1"));
        assert!(matches!(
            load_mode_state(temp_dir.path())
                .await
                .expect("load mode")
                .expect("mode exists")
                .current,
            MainMode::Auto
        ));
    }
}
