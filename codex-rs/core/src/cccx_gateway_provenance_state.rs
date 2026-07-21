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
    #[serde(default)]
    pub(crate) attempt_scope_id: String,
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
    attempt_scope_id: &str,
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
        attempt_scope_id: attempt_scope_id.to_string(),
        recorded_at: chrono::Utc::now().to_rfc3339(),
        provenance: provenance.clone(),
    };
    let bytes =
        serde_json::to_vec_pretty(&record).map_err(GatewayProvenanceStateError::Serialize)?;
    atomic_replace(&path, &bytes).await
}

/// Records a retry only when the persisted record belongs to the current
/// turn-scoped model-client session. A later prompt receives a new scope and
/// therefore can never inherit an earlier prompt's retry count.
pub(crate) async fn record_gateway_retry(
    project_root: &Path,
    thread_id: &str,
    attempt_scope_id: &str,
) -> Result<(), GatewayProvenanceStateError> {
    mutate_gateway_provenance(project_root, thread_id, attempt_scope_id, |provenance| {
        provenance.record_retry();
    })
    .await
}

/// Records a client transport fallback separately from router/provider
/// fallback provenance. The 9router route remains opaque unless the gateway
/// itself returns factual downstream metadata.
pub(crate) async fn record_gateway_transport_fallback(
    project_root: &Path,
    thread_id: &str,
    attempt_scope_id: &str,
    transition: &str,
) -> Result<(), GatewayProvenanceStateError> {
    mutate_gateway_provenance(project_root, thread_id, attempt_scope_id, |provenance| {
        provenance.record_transport_fallback(transition);
    })
    .await
}

/// Carries aggregate retry/transport fallback state from a failed attempt to
/// its next attempt. All route/provider fields remain owned by the live
/// attempt, preventing stale or inferred gateway identity from leaking across
/// retries.
pub(crate) async fn merge_attempt_aggregation(
    project_root: &Path,
    thread_id: &str,
    attempt_scope_id: &str,
    provenance: &mut GatewayProvenance,
) -> Result<(), GatewayProvenanceStateError> {
    let path = gateway_provenance_path(project_root, thread_id);
    let Some(record) = load_gateway_provenance_record(&path).await? else {
        return Ok(());
    };
    if record.attempt_scope_id != attempt_scope_id {
        return Ok(());
    }
    provenance.retry_count = record.provenance.retry_count;
    provenance.transport_fallbacks = record.provenance.transport_fallbacks;
    Ok(())
}

pub(crate) async fn load_gateway_provenance(
    project_root: &Path,
    thread_id: &str,
) -> Result<Option<GatewayProvenance>, GatewayProvenanceStateError> {
    let path = gateway_provenance_path(project_root, thread_id);
    let Some(record) = load_gateway_provenance_record(&path).await? else {
        return Ok(None);
    };
    Ok(Some(record.provenance))
}

async fn mutate_gateway_provenance(
    project_root: &Path,
    thread_id: &str,
    attempt_scope_id: &str,
    mutate: impl FnOnce(&mut GatewayProvenance),
) -> Result<(), GatewayProvenanceStateError> {
    let path = gateway_provenance_path(project_root, thread_id);
    let Some(mut record) = load_gateway_provenance_record(&path).await? else {
        return Ok(());
    };
    if record.attempt_scope_id != attempt_scope_id {
        return Ok(());
    }
    mutate(&mut record.provenance);
    save_gateway_provenance(
        project_root,
        thread_id,
        attempt_scope_id,
        &record.provenance,
    )
    .await
}

async fn load_gateway_provenance_record(
    path: &Path,
) -> Result<Option<PersistedGatewayProvenance>, GatewayProvenanceStateError> {
    let bytes = match fs::read(path).await {
        Ok(bytes) => bytes,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(GatewayProvenanceStateError::Read {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    let record =
        serde_json::from_slice(&bytes).map_err(|source| GatewayProvenanceStateError::Parse {
            path: path.to_path_buf(),
            source,
        })?;
    Ok(Some(record))
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

        save_gateway_provenance(project.path(), "thread-1", "scope-1", &provenance)
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

    #[tokio::test]
    async fn aggregation_merges_only_the_matching_turn_scope() {
        let project = tempfile::tempdir().expect("temp project");
        let mut prior = GatewayProvenance::new("route".to_string());
        prior.record_retry();
        prior.record_transport_fallback("responses_websocket->responses_http");
        save_gateway_provenance(project.path(), "thread-1", "scope-1", &prior)
            .await
            .expect("save prior provenance");

        let mut matching = GatewayProvenance::new("route".to_string());
        merge_attempt_aggregation(project.path(), "thread-1", "scope-1", &mut matching)
            .await
            .expect("merge matching scope");
        assert_eq!(matching.retry_count, 1);
        assert_eq!(
            matching.transport_fallbacks,
            vec!["responses_websocket->responses_http"]
        );

        let mut other_turn = GatewayProvenance::new("route".to_string());
        merge_attempt_aggregation(project.path(), "thread-1", "scope-2", &mut other_turn)
            .await
            .expect("merge other scope");
        assert_eq!(other_turn.retry_count, 0);
        assert!(other_turn.transport_fallbacks.is_empty());
    }

    #[tokio::test]
    async fn retry_and_transport_mutations_flow_into_the_next_attempt() {
        let project = tempfile::tempdir().expect("temp project");
        let initial = GatewayProvenance::new("route".to_string());
        save_gateway_provenance(project.path(), "thread-1", "scope-1", &initial)
            .await
            .expect("save initial provenance");

        record_gateway_retry(project.path(), "thread-1", "scope-1")
            .await
            .expect("record retry");
        record_gateway_transport_fallback(
            project.path(),
            "thread-1",
            "scope-1",
            "responses_websocket->responses_http",
        )
        .await
        .expect("record transport fallback");

        let mut retried_attempt = GatewayProvenance::new("route".to_string());
        merge_attempt_aggregation(project.path(), "thread-1", "scope-1", &mut retried_attempt)
            .await
            .expect("merge retry aggregation");

        assert_eq!(retried_attempt.retry_count, 1);
        assert_eq!(
            retried_attempt.transport_fallbacks,
            vec!["responses_websocket->responses_http"]
        );
    }
}
