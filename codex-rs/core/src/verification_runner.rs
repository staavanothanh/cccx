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
    pub(crate) commands: Vec<CommandExecution>,
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
    #[error("protected verification path `{path}` is invalid: {reason}")]
    ProtectedInputPath { path: String, reason: String },
    #[error("protected verification path `{path}` is outside project root `{project_root}`")]
    ProtectedInputOutsideProject {
        path: PathBuf,
        project_root: PathBuf,
    },
    #[error("protected path `{path}` has no pinned digest")]
    MissingProtectedDigest { path: String },
    #[error("failed to read protected verification input `{path}`: {source}")]
    ProtectedInputRead {
        path: PathBuf,
        source: std::io::Error,
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
    let command_specs = if spec.commands.is_empty() {
        spec.command.iter().collect::<Vec<_>>()
    } else {
        spec.commands.iter().collect::<Vec<_>>()
    };
    if command_specs.is_empty() {
        return Ok(None);
    }
    let protected_input_digest =
        verify_protected_inputs(project_root, &spec.protected_paths, &spec.protected_digests)
            .await?;
    let mut commands = Vec::with_capacity(command_specs.len());
    for command_spec in command_specs {
        let cwd = resolve_cwd(project_root, &command_spec.cwd)?;
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
        commands.push(CommandExecution {
            argv: command_spec.argv.clone(),
            cwd: cwd.to_string_lossy().into_owned(),
            exit_code: output.status.code().unwrap_or(-1),
            started_at,
            finished_at: chrono::Utc::now().to_rfc3339(),
            log_digest: sha256_digest(&log),
        });
    }
    // Verification commands are untrusted executables. Re-check the pinned
    // inputs after the entire sequence so a command cannot make its own
    // acceptance condition pass and then mutate the protected fixture.
    verify_protected_inputs(project_root, &spec.protected_paths, &spec.protected_digests).await?;
    Ok(Some(VerificationRun {
        commands,
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
    protected_paths: &std::collections::BTreeSet<String>,
    protected_digests: &std::collections::BTreeMap<String, String>,
) -> Result<String, VerificationRunnerError> {
    let canonical_root = crate::cccx_paths::resolve_physical_path(project_root, project_root)
        .map_err(|reason| VerificationRunnerError::ProtectedInputPath {
            path: project_root.display().to_string(),
            reason,
        })?;
    let mut resolved_digests = Vec::with_capacity(protected_digests.len());
    for (path, expected) in protected_digests {
        let resolved = resolve_protected_input(&canonical_root, path)?;
        resolved_digests.push((path, resolved, expected));
    }
    for protected_path in protected_paths {
        let resolved = resolve_protected_input(&canonical_root, protected_path)?;
        if !resolved_digests
            .iter()
            .any(|(_, digest_path, _)| digest_path == &resolved)
        {
            return Err(VerificationRunnerError::MissingProtectedDigest {
                path: protected_path.clone(),
            });
        }
    }
    let mut aggregate = Sha256::new();
    for (path, resolved, expected) in resolved_digests {
        let bytes = tokio::fs::read(&resolved).await.map_err(|source| {
            VerificationRunnerError::ProtectedInputRead {
                path: resolved.clone(),
                source,
            }
        })?;
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

fn resolve_protected_input(
    canonical_root: &Path,
    raw_path: &str,
) -> Result<PathBuf, VerificationRunnerError> {
    let resolved = crate::cccx_paths::resolve_physical_path(Path::new(raw_path), canonical_root)
        .map_err(|reason| VerificationRunnerError::ProtectedInputPath {
            path: raw_path.to_string(),
            reason,
        })?;
    if !crate::cccx_paths::path_is_within(&resolved, canonical_root) {
        return Err(VerificationRunnerError::ProtectedInputOutsideProject {
            path: resolved,
            project_root: canonical_root.to_path_buf(),
        });
    }
    Ok(resolved)
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
            commands: Vec::new(),
            protected_paths: std::collections::BTreeSet::new(),
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
        assert_eq!(run.commands.len(), 1);
        assert_eq!(run.commands[0].exit_code, 0);
        assert_eq!(run.commands[0].argv, spec.command.unwrap().argv);
        assert_eq!(
            std::fs::canonicalize(&run.commands[0].cwd).unwrap(),
            std::fs::canonicalize(root.path()).unwrap()
        );
        assert_ne!(run.commands[0].log_digest, "sha256:");
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

    #[tokio::test]
    async fn protected_input_cannot_escape_the_project_root() {
        let parent = TempDir::new().unwrap();
        let project = parent.path().join("project");
        tokio::fs::create_dir(&project).await.unwrap();
        tokio::fs::write(parent.path().join("outside.txt"), "secret")
            .await
            .unwrap();
        let mut spec = command_spec(vec!["definitely-not-run".to_string()]);
        spec.protected_digests
            .insert("../outside.txt".to_string(), sha256_digest(b"secret"));

        assert!(matches!(
            run_verification(&project, &spec).await,
            Err(VerificationRunnerError::ProtectedInputOutsideProject { .. })
        ));
    }

    #[tokio::test]
    async fn every_declared_protected_path_requires_a_pinned_digest() {
        let root = TempDir::new().unwrap();
        tokio::fs::write(root.path().join("golden.txt"), "original")
            .await
            .unwrap();
        let mut spec = command_spec(vec!["definitely-not-run".to_string()]);
        spec.protected_paths.insert("golden.txt".to_string());

        assert!(matches!(
            run_verification(root.path(), &spec).await,
            Err(VerificationRunnerError::MissingProtectedDigest { .. })
        ));
    }

    #[tokio::test]
    async fn verification_command_cannot_mutate_a_protected_input_after_precheck() {
        let root = TempDir::new().unwrap();
        tokio::fs::write(root.path().join("golden.txt"), "original")
            .await
            .unwrap();
        let mut spec = if cfg!(windows) {
            command_spec(vec![
                "cmd".to_string(),
                "/C".to_string(),
                "echo changed>golden.txt".to_string(),
            ])
        } else {
            command_spec(vec![
                "sh".to_string(),
                "-c".to_string(),
                "printf changed > golden.txt".to_string(),
            ])
        };
        spec.protected_paths.insert("golden.txt".to_string());
        spec.protected_digests
            .insert("golden.txt".to_string(), sha256_digest(b"original"));

        assert!(matches!(
            run_verification(root.path(), &spec).await,
            Err(VerificationRunnerError::ProtectedInputChanged { .. })
        ));
    }

    #[tokio::test]
    async fn runs_every_pinned_command_in_order() {
        let root = TempDir::new().unwrap();
        let argv = if cfg!(windows) {
            vec!["cmd".to_string(), "/C".to_string(), "exit /B 0".to_string()]
        } else {
            vec!["sh".to_string(), "-c".to_string(), "exit 0".to_string()]
        };
        let mut spec = command_spec(argv.clone());
        spec.command = None;
        spec.commands = vec![
            CommandSpec {
                argv: argv.clone(),
                cwd: ".".to_string(),
                timeout_ms: Some(5_000),
            },
            CommandSpec {
                argv,
                cwd: ".".to_string(),
                timeout_ms: Some(5_000),
            },
        ];

        let run = run_verification(root.path(), &spec).await.unwrap().unwrap();
        assert_eq!(run.commands.len(), 2);
        assert!(run.commands.iter().all(|command| command.exit_code == 0));
    }
}
