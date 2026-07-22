//! cccx Task Graph persistence and deterministic stop guard.
//!
//! This module is the core-side wiring point for `cccx-task-graph`. Session
//! runtime hooks call into this module instead of reimplementing persistence or
//! completion semantics.

use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::path::Path;
use std::path::PathBuf;
use std::process;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;

use cccx_mode::CccxModeState;
use cccx_task_graph::ActiveAttemptMetadata;
use cccx_task_graph::TaskGraph;
use cccx_task_graph::TaskNode;
use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;
use thiserror::Error;
use tokio::fs;

const CCCX_STATE_DIR: &str = ".cccx/state";
const TASK_GRAPH_FILE: &str = "task-graph.json";
const STATE_WRITER_LOCK_FILE: &str = ".writer.lock";
const STATE_WRITER_WAIT_TIMEOUT: Duration = Duration::from_secs(5);
const STATE_WRITER_RETRY_INTERVAL: Duration = Duration::from_millis(20);
const MAX_TASK_GRAPH_BYTES: u64 = 4 * 1024 * 1024;
static STATE_WRITE_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static PROJECT_WRITERS: OnceLock<Mutex<BTreeMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>> =
    OnceLock::new();
static VERIFIED_GRAPH_SEALS: OnceLock<Mutex<BTreeMap<PathBuf, String>>> = OnceLock::new();
static VERIFIED_TASK_ATTESTATIONS: OnceLock<Mutex<BTreeMap<(PathBuf, String), String>>> =
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
    #[error("task graph at {path} exceeds the {max_bytes}-byte safety limit")]
    TooLarge { path: PathBuf, max_bytes: u64 },
    #[error("failed to serialize task graph: {0}")]
    Serialize(serde_json::Error),
    #[error("invalid task graph: {0}")]
    InvalidGraph(String),
    #[error("task graph is not complete; unverified tasks: {0:?}")]
    StopBlocked(Vec<String>),
    #[error("task graph mutation rejected: {0}")]
    Mutation(String),
    #[error("state writer lease for {path} is already held by live process {pid}")]
    WriterLeaseContended { path: PathBuf, pid: u32 },
}

pub fn task_graph_path(project_root: &Path) -> PathBuf {
    project_root.join(CCCX_STATE_DIR).join(TASK_GRAPH_FILE)
}

pub async fn load_task_graph(
    project_root: &Path,
) -> Result<Option<TaskGraph>, TaskGraphStateError> {
    let path = task_graph_path(project_root);
    match fs::metadata(&path).await {
        Ok(metadata) if metadata.len() > MAX_TASK_GRAPH_BYTES => {
            return Err(TaskGraphStateError::TooLarge {
                path,
                max_bytes: MAX_TASK_GRAPH_BYTES,
            });
        }
        Ok(_) => {}
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(TaskGraphStateError::Read { path, source }),
    }
    let bytes = match fs::read(&path).await {
        Ok(bytes) => bytes,
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
    clear_verified_graph_seal(project_root);
    clear_verified_task_attestations(project_root);
    let writer = project_writer(project_root);
    let _process_lease = writer.lock().await;
    let _state_lease = acquire_state_writer_lease(project_root).await?;
    write_task_graph(project_root, task_graph).await
}

async fn write_task_graph(
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
    clear_verified_graph_seal(project_root);
    let writer = project_writer(project_root);
    let _process_lease = writer.lock().await;
    let _state_lease = acquire_state_writer_lease(project_root).await?;
    let mut graph = load_task_graph(project_root).await?.unwrap_or(initial);
    let result = mutate(&mut graph)?;
    write_task_graph(project_root, &graph).await?;
    retain_matching_task_attestations(project_root, &graph)?;
    Ok(result)
}

/// Record that the independent verification runner produced the exact task
/// state supplied by the in-process transaction. A later workspace edit cannot
/// manufacture this attestation or reuse it after changing the task payload.
pub(crate) fn attest_verified_task(
    project_root: &Path,
    task: &TaskNode,
) -> Result<(), TaskGraphStateError> {
    if !task.is_verified() {
        return Err(TaskGraphStateError::InvalidGraph(format!(
            "cannot attest unverified task `{}`",
            task.task_id
        )));
    }
    verified_task_attestations()
        .lock()
        .expect("verified task attestation map poisoned")
        .insert(
            (canonical_project_root(project_root), task.task_id.clone()),
            task_node_digest(task)?,
        );
    Ok(())
}

/// Seal one all-verified graph in process memory after the independent
/// verification runner has persisted it. Workspace JSON alone is never enough
/// to authorize an ADR write; any later graph mutation clears this seal.
pub(crate) fn seal_verified_graph(
    project_root: &Path,
    task_graph: &TaskGraph,
) -> Result<(), TaskGraphStateError> {
    task_graph
        .validate()
        .map_err(|error| TaskGraphStateError::InvalidGraph(error.to_string()))?;
    if !task_graph.can_stop() {
        return Err(TaskGraphStateError::InvalidGraph(
            "cannot seal an unverified task graph".to_string(),
        ));
    }
    let root = canonical_project_root(project_root);
    let attestations = verified_task_attestations()
        .lock()
        .expect("verified task attestation map poisoned");
    for (task_id, task) in &task_graph.tasks {
        let expected = task_node_digest(task)?;
        if attestations.get(&(root.clone(), task_id.clone())) != Some(&expected) {
            return Err(TaskGraphStateError::InvalidGraph(format!(
                "task `{task_id}` lacks an in-process verification attestation"
            )));
        }
    }
    drop(attestations);
    verified_graph_seals()
        .lock()
        .expect("verified graph seal map poisoned")
        .insert(
            canonical_project_root(project_root),
            task_graph_digest(task_graph)?,
        );
    Ok(())
}

pub(crate) fn matches_verified_graph_seal(project_root: &Path, task_graph: &TaskGraph) -> bool {
    let Ok(digest) = task_graph_digest(task_graph) else {
        return false;
    };
    verified_graph_seals()
        .lock()
        .expect("verified graph seal map poisoned")
        .get(&canonical_project_root(project_root))
        == Some(&digest)
}

fn clear_verified_graph_seal(project_root: &Path) {
    verified_graph_seals()
        .lock()
        .expect("verified graph seal map poisoned")
        .remove(&canonical_project_root(project_root));
}

fn verified_graph_seals() -> &'static Mutex<BTreeMap<PathBuf, String>> {
    VERIFIED_GRAPH_SEALS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn verified_task_attestations() -> &'static Mutex<BTreeMap<(PathBuf, String), String>> {
    VERIFIED_TASK_ATTESTATIONS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn clear_verified_task_attestations(project_root: &Path) {
    let root = canonical_project_root(project_root);
    verified_task_attestations()
        .lock()
        .expect("verified task attestation map poisoned")
        .retain(|(candidate, _), _| candidate != &root);
}

fn retain_matching_task_attestations(
    project_root: &Path,
    task_graph: &TaskGraph,
) -> Result<(), TaskGraphStateError> {
    let root = canonical_project_root(project_root);
    let current = task_graph
        .tasks
        .iter()
        .map(|(task_id, task)| Ok((task_id.clone(), task_node_digest(task)?)))
        .collect::<Result<BTreeMap<_, _>, TaskGraphStateError>>()?;
    verified_task_attestations()
        .lock()
        .expect("verified task attestation map poisoned")
        .retain(|(candidate, task_id), digest| {
            candidate != &root || current.get(task_id) == Some(digest)
        });
    Ok(())
}

fn task_graph_digest(task_graph: &TaskGraph) -> Result<String, TaskGraphStateError> {
    use sha2::Digest;
    use sha2::Sha256;
    let bytes = serde_json::to_vec(task_graph).map_err(TaskGraphStateError::Serialize)?;
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}

fn task_node_digest(task: &TaskNode) -> Result<String, TaskGraphStateError> {
    use sha2::Digest;
    use sha2::Sha256;
    let bytes = serde_json::to_vec(task).map_err(TaskGraphStateError::Serialize)?;
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}

fn canonical_project_root(path: &Path) -> PathBuf {
    dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
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

fn state_dir_path(project_root: &Path) -> PathBuf {
    project_root.join(CCCX_STATE_DIR)
}

fn state_writer_lock_path(project_root: &Path) -> PathBuf {
    state_dir_path(project_root).join(STATE_WRITER_LOCK_FILE)
}

async fn acquire_state_writer_lease(
    project_root: &Path,
) -> Result<StateWriterLease, TaskGraphStateError> {
    let started = Instant::now();
    loop {
        match StateWriterLease::acquire(project_root).await {
            Ok(lease) => return Ok(lease),
            Err(TaskGraphStateError::WriterLeaseContended { .. })
                if started.elapsed() < STATE_WRITER_WAIT_TIMEOUT =>
            {
                tokio::time::sleep(STATE_WRITER_RETRY_INTERVAL).await;
            }
            Err(TaskGraphStateError::Read { source, .. })
                if source.kind() == ErrorKind::NotFound
                    && started.elapsed() < STATE_WRITER_WAIT_TIMEOUT =>
            {
                // The previous holder can release the lock between our
                // create-new failure and lease-record read. Retry creation.
                tokio::time::sleep(STATE_WRITER_RETRY_INTERVAL).await;
            }
            Err(TaskGraphStateError::Write { source, .. })
                if source.kind() == ErrorKind::NotFound
                    && started.elapsed() < STATE_WRITER_WAIT_TIMEOUT =>
            {
                // A stale holder can disappear between liveness inspection
                // and cleanup. Retry rather than treating that race as I/O
                // corruption.
                tokio::time::sleep(STATE_WRITER_RETRY_INTERVAL).await;
            }
            Err(error) => return Err(error),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StateWriterLeaseRecord {
    pid: u32,
    token: String,
    acquired_at: String,
}

struct StateWriterLease {
    path: PathBuf,
    token: String,
}

impl StateWriterLease {
    async fn acquire(project_root: &Path) -> Result<Self, TaskGraphStateError> {
        let state_dir = state_dir_path(project_root);
        fs::create_dir_all(&state_dir)
            .await
            .map_err(|source| TaskGraphStateError::CreateDir {
                path: state_dir.clone(),
                source,
            })?;

        let path = state_writer_lock_path(project_root);
        let token = format!(
            "{}-{}",
            process::id(),
            STATE_WRITE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        );
        let record = StateWriterLeaseRecord {
            pid: process::id(),
            token: token.clone(),
            acquired_at: Utc::now().to_rfc3339(),
        };
        let bytes = serde_json::to_vec(&record).map_err(TaskGraphStateError::Serialize)?;

        match Self::try_create(&path, &bytes).await {
            Ok(()) => {
                return Ok(Self { path, token });
            }
            Err(source) if source.kind() == ErrorKind::AlreadyExists => {}
            Err(source) => {
                return Err(TaskGraphStateError::Write { path, source });
            }
        }

        let live_pid = live_lease_pid(&path).await?;
        if process_is_live(live_pid) {
            return Err(TaskGraphStateError::WriterLeaseContended {
                path,
                pid: live_pid,
            });
        }

        fs::remove_file(&path)
            .await
            .map_err(|source| TaskGraphStateError::Write {
                path: path.clone(),
                source,
            })?;
        match Self::try_create(&path, &bytes).await {
            Ok(()) => Ok(Self { path, token }),
            Err(source) if source.kind() == ErrorKind::AlreadyExists => {
                let live_pid = live_lease_pid(&path).await?;
                Err(TaskGraphStateError::WriterLeaseContended {
                    path,
                    pid: live_pid,
                })
            }
            Err(source) => Err(TaskGraphStateError::Write { path, source }),
        }
    }

    async fn try_create(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .await?;
        use tokio::io::AsyncWriteExt;
        file.write_all(bytes).await?;
        file.sync_all().await
    }
}

impl Drop for StateWriterLease {
    fn drop(&mut self) {
        let Ok(bytes) = std::fs::read(&self.path) else {
            return;
        };
        let Ok(record) = serde_json::from_slice::<StateWriterLeaseRecord>(&bytes) else {
            return;
        };
        if record.token == self.token {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

async fn live_lease_pid(path: &Path) -> Result<u32, TaskGraphStateError> {
    let bytes = fs::read(path)
        .await
        .map_err(|source| TaskGraphStateError::Read {
            path: path.to_path_buf(),
            source,
        })?;
    let record: StateWriterLeaseRecord =
        serde_json::from_slice(&bytes).map_err(|source| TaskGraphStateError::Parse {
            path: path.to_path_buf(),
            source,
        })?;
    Ok(record.pid)
}

fn process_is_live(pid: u32) -> bool {
    if pid == process::id() {
        return true;
    }
    platform_process_is_live(pid)
}

#[cfg(windows)]
fn platform_process_is_live(pid: u32) -> bool {
    let Ok(output) = std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
        .output()
    else {
        return true;
    };
    if !output.status.success() {
        return true;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let pid_field = format!("\",\"{pid}\",");
    stdout
        .lines()
        .any(|line| line.starts_with('"') && line.contains(&pid_field))
}

#[cfg(unix)]
fn platform_process_is_live(pid: u32) -> bool {
    let pid = pid as libc::pid_t;
    // SAFETY: kill(pid, 0) does not send a signal; it only asks the OS whether
    // a process with this pid is visible to the current process.
    let result = unsafe { libc::kill(pid, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(not(any(unix, windows)))]
fn platform_process_is_live(_pid: u32) -> bool {
    true
}

async fn atomic_replace(path: &Path, bytes: &[u8]) -> Result<(), TaskGraphStateError> {
    let sequence = STATE_WRITE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = path.with_file_name(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("state"),
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
    let _process_lease = writer.lock().await;
    let _state_lease = acquire_state_writer_lease(project_root).await?;
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

    use cccx_policy::MainMode;
    use cccx_task_graph::TaskNode;
    use cccx_task_graph::TaskStatus;
    use cccx_task_graph::VerificationSpec;
    use pretty_assertions::assert_eq;
    use tempfile::TempDir;

    use super::*;

    fn verification_spec() -> VerificationSpec {
        VerificationSpec {
            command: None,
            commands: Vec::new(),
            protected_paths: std::collections::BTreeSet::new(),
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

    #[test]
    fn verified_graph_seal_rejects_workspace_state_tampering() {
        let temp_dir = TempDir::new().expect("temp dir");
        let mut graph = TaskGraph::new("graph-1", "verified");
        let mut task = TaskNode::new("T1", "verified", verification_spec());
        task.status = TaskStatus::Verified;
        task.verified_at = Some("2026-07-22T00:00:00Z".to_string());
        graph.add_task(task).expect("add task");
        attest_verified_task(temp_dir.path(), &graph.tasks["T1"]).expect("attest task");
        seal_verified_graph(temp_dir.path(), &graph).expect("seal verified graph");
        assert!(matches_verified_graph_seal(temp_dir.path(), &graph));

        graph.title = "forged workspace state".to_string();

        assert!(!matches_verified_graph_seal(temp_dir.path(), &graph));
    }

    #[test]
    fn forged_verified_task_cannot_be_sealed_without_runner_attestation() {
        let temp_dir = TempDir::new().expect("temp dir");
        let mut graph = TaskGraph::new("graph-1", "forged");
        let mut task = TaskNode::new("T1", "forged", verification_spec());
        task.status = TaskStatus::Verified;
        task.verified_at = Some("2026-07-22T00:00:00Z".to_string());
        graph.add_task(task).expect("add task");

        let error = seal_verified_graph(temp_dir.path(), &graph)
            .expect_err("workspace JSON alone must not grant ADR authority");

        assert!(error.to_string().contains("verification attestation"));
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
        save_task_graph(temp_dir.path(), &graph)
            .await
            .expect("save graph");
        let path = task_graph_path(temp_dir.path());
        let temporary = path.with_file_name(".task-graph.json.crash.tmp");
        fs::write(&temporary, b"{ incomplete")
            .await
            .expect("write interrupted temp");

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
    async fn live_state_writer_lease_fails_closed_on_contention() {
        let temp_dir = TempDir::new().expect("temp dir");
        let _held = StateWriterLease::acquire(temp_dir.path())
            .await
            .expect("first lease");

        assert!(matches!(
            StateWriterLease::acquire(temp_dir.path()).await,
            Err(TaskGraphStateError::WriterLeaseContended { pid, .. })
                if pid == std::process::id()
        ));
    }

    #[tokio::test]
    async fn stale_state_writer_lease_from_dead_process_is_recovered() {
        let temp_dir = TempDir::new().expect("temp dir");
        let state_dir = state_dir_path(temp_dir.path());
        fs::create_dir_all(&state_dir).await.expect("state dir");
        let lock_path = state_writer_lock_path(temp_dir.path());
        let stale_record = StateWriterLeaseRecord {
            pid: 999_999,
            token: "dead-owner".to_string(),
            acquired_at: "2026-07-21T00:00:00Z".to_string(),
        };
        fs::write(
            &lock_path,
            serde_json::to_vec(&stale_record).expect("lease json"),
        )
        .await
        .expect("stale lock");

        let graph = TaskGraph::new("graph-1", "stale lease recovery");
        save_task_graph(temp_dir.path(), &graph)
            .await
            .expect("save after stale lock");

        assert_eq!(
            load_task_graph(temp_dir.path())
                .await
                .expect("load graph")
                .expect("graph exists"),
            graph
        );
        assert!(
            !lock_path.exists(),
            "the recovered writer lease must be released after the write"
        );
    }

    #[tokio::test]
    async fn interprocess_state_writer_lease_fails_closed_on_contention() {
        let temp_dir = TempDir::new().expect("temp dir");
        let ready_path = temp_dir.path().join("child-ready");
        let release_path = temp_dir.path().join("child-release");
        let mut child = std::process::Command::new(std::env::current_exe().expect("current exe"))
            .arg("state_writer_lease_child_process_entrypoint")
            .env("CCCX_STATE_LEASE_CHILD_ROOT", temp_dir.path())
            .env("CCCX_STATE_LEASE_CHILD_READY", &ready_path)
            .env("CCCX_STATE_LEASE_CHILD_RELEASE", &release_path)
            .spawn()
            .expect("spawn lease holder child");

        wait_for_path(&ready_path);
        let child_pid = child.id();
        assert!(matches!(
            StateWriterLease::acquire(temp_dir.path()).await,
            Err(TaskGraphStateError::WriterLeaseContended { pid, .. }) if pid == child_pid
        ));

        std::fs::write(&release_path, b"release").expect("release child");
        let status = child.wait().expect("wait child");
        assert!(status.success(), "lease holder child failed: {status}");
    }

    #[tokio::test]
    async fn concurrent_process_transactions_retain_both_updates() {
        let temp_dir = TempDir::new().expect("temp dir");
        let go_path = temp_dir.path().join("children-go");
        let first_ready = temp_dir.path().join("first-ready");
        let second_ready = temp_dir.path().join("second-ready");
        let executable = std::env::current_exe().expect("current test executable");
        let mut first = std::process::Command::new(&executable)
            .arg("state_writer_transaction_child_process_entrypoint")
            .env("CCCX_STATE_TX_ROOT", temp_dir.path())
            .env("CCCX_STATE_TX_READY", &first_ready)
            .env("CCCX_STATE_TX_GO", &go_path)
            .env("CCCX_STATE_TX_TASK", "T1")
            .spawn()
            .expect("spawn first transaction child");
        let mut second = std::process::Command::new(&executable)
            .arg("state_writer_transaction_child_process_entrypoint")
            .env("CCCX_STATE_TX_ROOT", temp_dir.path())
            .env("CCCX_STATE_TX_READY", &second_ready)
            .env("CCCX_STATE_TX_GO", &go_path)
            .env("CCCX_STATE_TX_TASK", "T2")
            .spawn()
            .expect("spawn second transaction child");

        wait_for_path(&first_ready);
        wait_for_path(&second_ready);
        std::fs::write(&go_path, b"go").expect("release transaction children");
        let first_status = first.wait().expect("wait first transaction child");
        let second_status = second.wait().expect("wait second transaction child");
        assert!(first_status.success(), "first child failed: {first_status}");
        assert!(
            second_status.success(),
            "second child failed: {second_status}"
        );

        let graph = load_task_graph(temp_dir.path())
            .await
            .expect("load merged graph")
            .expect("graph exists");
        assert_eq!(
            graph.tasks.keys().cloned().collect::<Vec<_>>(),
            vec!["T1".to_string(), "T2".to_string()]
        );
        assert!(
            !state_writer_lock_path(temp_dir.path()).exists(),
            "the process-safe writer lease must be released after both transactions"
        );
    }

    #[tokio::test]
    async fn state_writer_transaction_child_process_entrypoint() {
        let Some(root) = std::env::var_os("CCCX_STATE_TX_ROOT") else {
            return;
        };
        let ready_path =
            PathBuf::from(std::env::var_os("CCCX_STATE_TX_READY").expect("ready path"));
        let go_path = PathBuf::from(std::env::var_os("CCCX_STATE_TX_GO").expect("go path"));
        let task_id = std::env::var("CCCX_STATE_TX_TASK").expect("task id");
        std::fs::write(&ready_path, b"ready").expect("signal transaction ready");
        wait_for_path(&go_path);

        let initial = TaskGraph::new("graph-1", "interprocess transactions");
        update_task_graph(Path::new(&root), initial, |graph| {
            graph
                .add_task(TaskNode::new(
                    task_id.clone(),
                    format!("task {task_id}"),
                    verification_spec(),
                ))
                .map_err(|error| TaskGraphStateError::Mutation(error.to_string()))
        })
        .await
        .expect("transaction child update");
    }

    #[tokio::test]
    async fn state_writer_lease_child_process_entrypoint() {
        let Some(root) = std::env::var_os("CCCX_STATE_LEASE_CHILD_ROOT") else {
            return;
        };
        let ready_path =
            PathBuf::from(std::env::var_os("CCCX_STATE_LEASE_CHILD_READY").expect("ready path"));
        let release_path = PathBuf::from(
            std::env::var_os("CCCX_STATE_LEASE_CHILD_RELEASE").expect("release path"),
        );
        let root = PathBuf::from(root);
        let _lease = StateWriterLease::acquire(&root).await.expect("child lease");
        std::fs::write(&ready_path, b"ready").expect("signal ready");
        for _ in 0..100 {
            if release_path.exists() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        panic!("timed out waiting for release marker");
    }

    fn wait_for_path(path: &Path) {
        for _ in 0..100 {
            if path.exists() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        panic!("timed out waiting for {}", path.display());
    }

    #[tokio::test]
    async fn task_goal_condition_is_persisted_without_verifying_the_task() {
        let temp_dir = TempDir::new().expect("temp dir");
        let mut graph = TaskGraph::new("graph-1", "goal");
        graph
            .add_task(TaskNode::new("T1", "first", verification_spec()))
            .expect("add task");
        save_task_graph(temp_dir.path(), &graph)
            .await
            .expect("save graph");
        set_task_goal_condition(temp_dir.path(), "T1", "focused tests must pass".to_string())
            .await
            .expect("set goal");
        let restored = load_task_graph(temp_dir.path()).await.unwrap().unwrap();
        let task = &restored.tasks["T1"];
        assert_eq!(
            task.goal_condition.as_deref(),
            Some("focused tests must pass")
        );
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
                        worker_incarnation_id: "inc-1".to_string(),
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
        save_task_graph(temp_dir.path(), &graph)
            .await
            .expect("save graph");
        let active = load_active_attempts(temp_dir.path())
            .await
            .expect("load active");
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
        assert!(
            load_task_graph(temp_dir.path())
                .await
                .expect("load graph")
                .expect("graph exists")
                .tasks
                .contains_key("T1")
        );
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
