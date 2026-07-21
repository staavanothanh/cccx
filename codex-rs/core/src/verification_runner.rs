//! Harness-owned execution of a pinned verification command.
//!
//! The model never supplies a shell command at completion time.  The runner
//! consumes the structured command and protected digest contract frozen in the
//! WorkSpec/TaskGraph before the worker began.

use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use cccx_protocol::CommandExecution;
use cccx_task_graph::VerificationSpec;
use sha2::Digest;
use sha2::Sha256;
use thiserror::Error;
use tokio::process::Command;

const DEFAULT_TIMEOUT_MS: u64 = 60_000;

#[derive(Debug)]
pub(crate) struct VerificationRun {
    pub(crate) command: CommandExecution,
    pub(crate) protected_input_digest: String,
}

#[derive(Debug, Error)]
pub(crate) enum VerificationRunnerError {
    #[error("verification cwd `{path}` is outside project root `{project_root}")]
    CwdOutsideProject {
        path: PathBuf,
        project_root: PathBuf,
    },
    #[error("protected verification input changed: `{path}` expected {expected}, got {actual}")]
    ProtectedInputChanged {
        path: String,
        expected: String,
        actual: String,
    },
    #[error("failed to start verification command: {0}")]
    Spawn(#[source] std::io::Error),
    #[error("verification command exceeded its pinned timeout of {timeout_ms}ms")]
    TimedOut { timeout_ms: u64 },
}

pub(crate) async fn run_verification(
    project_root: &Path,
    spec: &VerificationSpec,
) -> Result<Option<VerificationRun>, VerificationRunnerError> {
    let Some(command_spec) = &spec.command else {
        return Ok(None);
    };
    let cwd = resolve_cwd(project_root, &command_spec.cwd)?;
    let protected_input_digest =
        verify_protected_inputs(project_root, &spec.protected_digests).await?;
    let started_at = chrono::Utc::now().to_rfc3339();
    let timeout_ms = command_spec.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS);
    let mut command = Command::new(&command_spec.argv[0]);
    command.args(&command_spec.argv[1..]).current_dir(&cwd);
    let output =
        match tokio::time::timeout(Duration::from_millis(timeout_ms), command.output()).await {
            Ok(result) => result.map_err(VerificationRunnerError::Spawn)?,
            Err(_) => return Err(VerificationRunnerError::TimedOut { timeout_ms }),
        };
    let mut log = output.stdout;
    log.extend_from_slice(&output.stderr);
    let log_digest = sha256_digest(&log);
    Ok(Some(VerificationRun {
        command: CommandExecution {
            argv: command_spec.argv.clone(),
            cwd: cwd.to_string_lossy().into_owned(),
            exit_code: output.status.code().unwrap_or(-1),
            started_at,
            finished_at: chrono::Utc::now().to_rfc3339(),
            log_digest,
        },
        protected_input_digest,
    }))
}

fn resolve_cwd(project_root: &Path, raw_cwd: &str) -> Result<PathBuf, VerificationRunnerError> {
    let root = std::fs::canonicalize(project_root).unwrap_or_else(|_| project_root.to_path_buf());
    let requested = PathBuf::from(raw_cwd);
    let candidate = if requested.is_absolute() {
        requested
    } else {
        root.join(requested)
    };
    let canonical = std::fs::canonicalize(&candidate).unwrap_or(candidate);
    if canonical.starts_with(&root) {
        Ok(canonical)
    } else {
        Err(VerificationRunnerError::CwdOutsideProject {
            path: canonical,
            project_root: root,
        })
    }
}

async fn verify_protected_inputs(
    project_root: &Path,
    protected_digests: &std::collections::BTreeMap<String, String>,
) -> Result<String, VerificationRunnerError> {
    let mut aggregate = Sha256::new();
    for (path, expected) in protected_digests {
        let resolved = project_root.join(path);
        let bytes = tokio::fs::read(&resolved).await.unwrap_or_default();
        let actual = sha256_digest(&bytes);
        if &actual != expected {
            return Err(VerificationRunnerError::ProtectedInputChanged {
                path: path.clone(),
                expected: expected.clone(),
                actual,
            });
        }
        aggregate.update(path.as_bytes());
        aggregate.update(expected.as_bytes());
    }
    Ok(format!("sha256:{:x}", aggregate.finalize()))
}

fn sha256_digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cccx_protocol::CommandSpec;
    use std::collections::BTreeMap;
    use tempfile::TempDir;

    fn command_spec(argv: Vec<String>) -> VerificationSpec {
        VerificationSpec {
            command: Some(CommandSpec {
                argv,
                cwd: ".".to_string(),
                timeout_ms: Some(5_000),
            }),
            protected_digests: BTreeMap::new(),
            verification_policy_id: Some("test".to_string()),
            spec_digest: "sha256:spec".to_string(),
        }
    }

    #[tokio::test]
    async fn runs_pinned_argv_cwd_and_captures_real_output_digest() {
        let root = TempDir::new().unwrap();
        let spec = if cfg!(windows) {
            command_spec(vec![
                "cmd".to_string(),
                "/C".to_string(),
                "echo cccx-verification".to_string(),
            ])
        } else {
            command_spec(vec![
                "sh".to_string(),
                "-c".to_string(),
                "printf cccx-verification".to_string(),
            ])
        };
        let run = run_verification(root.path(), &spec).await.unwrap().unwrap();
        assert_eq!(run.command.exit_code, 0);
        assert_eq!(run.command.argv, spec.command.unwrap().argv);
        assert_eq!(
            std::fs::canonicalize(&run.command.cwd).unwrap(),
            std::fs::canonicalize(root.path()).unwrap()
        );
        assert_ne!(run.command.log_digest, "sha256:");
    }

    #[tokio::test]
    async fn protected_digest_drift_blocks_before_command_runs() {
        let root = TempDir::new().unwrap();
        let protected = root.path().join("golden.txt");
        tokio::fs::write(&protected, "changed").await.unwrap();
        let mut spec = command_spec(vec!["definitely-not-run".to_string()]);
        spec.protected_digests
            .insert("golden.txt".to_string(), "sha256:expected".to_string());
        assert!(matches!(
            run_verification(root.path(), &spec).await,
            Err(VerificationRunnerError::ProtectedInputChanged { .. })
        ));
    }
}
