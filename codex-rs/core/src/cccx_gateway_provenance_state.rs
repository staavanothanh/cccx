//! cccx provider-gateway provenance persistence.
//!
//! Model streaming code observes the real transport attempt, while the cccx
//! runtime finalizes WorkSpec/Artifact lifecycle later. This small store is the
//! bridge between those two call-sites: provenance is keyed by the thread /
//! attempt id and consumed when the matching worker Artifact is finalized.

use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use cccx_provider_gateway::provenance::GatewayProvenance;
use serde::Deserialize;
use serde::Serialize;
use thiserror::Error;
use tokio::fs;

const CCCX_STATE_DIR: &str = ".cccx/state";
const GATEWAY_PROVENANCE_DIR: &str = "gateway-provenance";
static GATEWAY_PROVENANCE_WRITE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PersistedGatewayProvenance {
    pub(crate) thread_id: String,
    pub(crate) recorded_at: String,
    pub(crate) provenance: GatewayProvenance,
}

#[derive(Debug, Error)]
pub(crate) enum GatewayProvenanceStateError {
    #[error("failed to read gateway provenance at {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to write gateway provenance at {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to create gateway provenance directory at {path}: {source}")]
    CreateDir {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to parse gateway provenance at {path}: {source}")]
    Parse {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("failed to serialize gateway provenance: {0}")]
    Serialize(serde_json::Error),
}

pub(crate) fn gateway_provenance_path(project_root: &Path, thread_id: &str) -> PathBuf {
    let safe_thread_id = thread_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    project_root
        .join(CCCX_STATE_DIR)
        .join(GATEWAY_PROVENANCE_DIR)
        .join(format!("{safe_thread_id}.json"))
}

pub(crate) async fn save_gateway_provenance(
    project_root: &Path,
    thread_id: &str,
    provenance: &GatewayProvenance,
) -> Result<(), GatewayProvenanceStateError> {
    let path = gateway_provenance_path(project_root, thread_id);
    let Some(parent) = path.parent() else {
        return Err(GatewayProvenanceStateError::Write {
            path,
            source: std::io::Error::other("gateway provenance path has no parent"),
        });
    };
    fs::create_dir_all(parent)
        .await
        .map_err(|source| GatewayProvenanceStateError::CreateDir {
            path: parent.to_path_buf(),
            source,
        })?;
    let record = PersistedGatewayProvenance {
        thread_id: thread_id.to_string(),
        recorded_at: chrono::Utc::now().to_rfc3339(),
        provenance: provenance.clone(),
    };
    let bytes =
        serde_json::to_vec_pretty(&record).map_err(GatewayProvenanceStateError::Serialize)?;
    atomic_replace(&path, &bytes).await
}

pub(crate) async fn load_gateway_provenance(
    project_root: &Path,
    thread_id: &str,
) -> Result<Option<GatewayProvenance>, GatewayProvenanceStateError> {
    let path = gateway_provenance_path(project_root, thread_id);
    let bytes = match fs::read(&path).await {
        Ok(bytes) => bytes,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(GatewayProvenanceStateError::Read { path, source }),
    };
    let record: PersistedGatewayProvenance =
        serde_json::from_slice(&bytes).map_err(|source| GatewayProvenanceStateError::Parse {
            path,
            source,
        })?;
    Ok(Some(record.provenance))
}

async fn atomic_replace(path: &Path, bytes: &[u8]) -> Result<(), GatewayProvenanceStateError> {
    let sequence = GATEWAY_PROVENANCE_WRITE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = path.with_file_name(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("gateway-provenance"),
        sequence
    ));
    fs::write(&temporary, bytes)
        .await
        .map_err(|source| GatewayProvenanceStateError::Write {
            path: temporary.clone(),
            source,
        })?;
    fs::OpenOptions::new()
        .write(true)
        .open(&temporary)
        .await
        .map_err(|source| GatewayProvenanceStateError::Write {
            path: temporary.clone(),
            source,
        })?
        .sync_all()
        .await
        .map_err(|source| GatewayProvenanceStateError::Write {
            path: temporary.clone(),
            source,
        })?;
    fs::rename(&temporary, path)
        .await
        .map_err(|source| GatewayProvenanceStateError::Write {
            path: path.to_path_buf(),
            source,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn gateway_provenance_round_trip_by_thread_id() {
        let project = tempfile::tempdir().expect("temp project");
        let mut provenance = GatewayProvenance::new("oc/deepseek-v4-flash-free".to_string());
        provenance.record_wire_api("chat_completions");
        provenance.record_stream_termination("completed");
        provenance.record_retry();

        save_gateway_provenance(project.path(), "thread-1", &provenance)
            .await
            .expect("save provenance");

        let loaded = load_gateway_provenance(project.path(), "thread-1")
            .await
            .expect("load provenance")
            .expect("record exists");

        assert_eq!(loaded.requested_route_id, "oc/deepseek-v4-flash-free");
        assert_eq!(loaded.wire_api.as_deref(), Some("chat_completions"));
        assert_eq!(
            loaded.stream_termination_status.as_deref(),
            Some("completed")
        );
        assert_eq!(loaded.retry_count, 1);
    }
}
