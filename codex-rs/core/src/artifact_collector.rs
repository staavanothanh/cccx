use cccx_protocol::{Artifact, ArtifactProvenance, ArtifactStatus, Evidence, EvidenceKind};
use std::collections::BTreeSet;

/// Accumulates subagent execution evidence and produces an Artifact on completion.
pub struct ArtifactCollector {
    work_spec_id: String,
    attempt_id: String,
    changed_paths: BTreeSet<String>,
    evidence_entries: Vec<Evidence>,
}

impl ArtifactCollector {
    pub fn new(work_spec_id: &str, attempt_id: &str) -> Self {
        Self {
            work_spec_id: work_spec_id.to_string(),
            attempt_id: attempt_id.to_string(),
            changed_paths: BTreeSet::new(),
            evidence_entries: Vec::new(),
        }
    }

    pub fn record_file_change(&mut self, path: &str) {
        self.changed_paths.insert(path.to_string());
    }

    pub fn record_test_run(&mut self, command: &str, exit_code: i32, log: &str) {
        let digest = sha256_digest(log);
        self.evidence_entries.push(Evidence {
            kind: EvidenceKind::TestLog,
            summary: format!("`{command}` exited {exit_code}"),
            digest,
        });
    }

    pub fn record_build(&mut self, command: &str, exit_code: i32, log: &str) {
        let digest = sha256_digest(log);
        self.evidence_entries.push(Evidence {
            kind: EvidenceKind::BuildLog,
            summary: format!("`{command}` exited {exit_code}"),
            digest,
        });
    }

    pub fn record_diff_digest(&mut self, summary: &str, log: &str) {
        let digest = sha256_digest(log);
        self.evidence_entries.push(Evidence {
            kind: EvidenceKind::DiffDigest,
            summary: summary.to_string(),
            digest,
        });
    }

    pub fn record_finding(&mut self, detail: &str) {
        let digest = sha256_digest(detail);
        self.evidence_entries.push(Evidence {
            kind: EvidenceKind::ReviewFinding,
            summary: detail.to_string(),
            digest,
        });
    }

    pub fn record_runtime_log(&mut self, summary: &str, log: &str) {
        let digest = sha256_digest(log);
        self.evidence_entries.push(Evidence {
            kind: EvidenceKind::RuntimeLog,
            summary: summary.to_string(),
            digest,
        });
    }

    pub fn finalize(
        self,
        status: ArtifactStatus,
        summary: String,
        provenance: ArtifactProvenance,
    ) -> Artifact {
        let artifact_id = format!("art-{}-{}", self.work_spec_id, self.attempt_id);
        Artifact {
            schema_version: cccx_protocol::CCCX_PROTOCOL_SCHEMA_VERSION.to_string(),
            artifact_id,
            work_spec_id: self.work_spec_id,
            attempt_id: self.attempt_id,
            status,
            summary,
            changed_paths: self.changed_paths,
            evidence: self.evidence_entries,
            provenance: Some(provenance),
            verification: None,
        }
    }
}

fn sha256_digest(input: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("sha256:{:x}", Sha256::digest(input.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cccx_protocol::HarnessKind;
    use cccx_protocol::WorkRole;

    fn provenance() -> ArtifactProvenance {
        ArtifactProvenance {
            producer_harness: HarnessKind::Cx,
            producer_role: WorkRole::Implementer,
            requested_route_id: "oc/deepseek-v4-flash-free".to_string(),
            effective_route_id: "opencode/deepseek-v4-flash-free".to_string(),
            gateway_request_id: Some("gw-1".to_string()),
            input_digest: "sha256:input".to_string(),
            output_digest: "sha256:output".to_string(),
        }
    }

    #[test]
    fn finalized_success_artifact_contains_required_provenance_and_evidence() {
        let mut collector = ArtifactCollector::new("ws-1", "attempt-1");
        collector.record_file_change("D:/repo/src/lib.rs");
        collector.record_file_change("D:/repo/src/lib.rs");
        collector.record_test_run("cargo test -p cccx-protocol", 0, "ok");
        collector.record_diff_digest("changed lib.rs", "diff --git a/src/lib.rs b/src/lib.rs");

        let artifact = collector.finalize(
            ArtifactStatus::Success,
            "bounded task completed".to_string(),
            provenance(),
        );

        artifact
            .validate()
            .expect("collector must produce design-valid artifact");
        assert_eq!(artifact.artifact_id, "art-ws-1-attempt-1");
        assert_eq!(artifact.changed_paths.len(), 1);
        assert_eq!(artifact.evidence.len(), 2);
        assert!(
            artifact
                .evidence
                .iter()
                .all(|entry| entry.digest.starts_with("sha256:"))
        );
    }

    #[test]
    fn success_artifact_without_runtime_evidence_remains_invalid() {
        let collector = ArtifactCollector::new("ws-1", "attempt-1");

        let artifact = collector.finalize(
            ArtifactStatus::Success,
            "claim without proof".to_string(),
            provenance(),
        );

        assert!(artifact.validate().is_err());
    }
}
