use fs2::FileExt;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::path::Path;

/// OS-backed exclusive writer lease keyed by canonical project root.
///
/// The lock file may persist, but the operating-system lock is released even
/// when a process crashes. This avoids stale-PID recovery races while keeping
/// independent cccx processes from writing the same project concurrently.
pub(crate) struct CrossProcessWriteLease {
    _file: File,
}

impl CrossProcessWriteLease {
    pub(crate) async fn acquire(project_root: &Path) -> Result<Self, String> {
        let canonical =
            std::fs::canonicalize(project_root).unwrap_or_else(|_| project_root.to_path_buf());
        let digest = format!(
            "{:x}",
            Sha256::digest(canonical.to_string_lossy().as_bytes())
        );
        let directory = std::env::temp_dir().join("cccx-write-leases");
        tokio::fs::create_dir_all(&directory)
            .await
            .map_err(|error| format!("cannot create write-lease directory: {error}"))?;
        let path = directory.join(format!("{digest}.lock"));

        let file = tokio::task::spawn_blocking(move || {
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .open(&path)
                .map_err(|error| format!("cannot open write lease: {error}"))?;
            file.lock_exclusive()
                .map_err(|error| format!("cannot acquire write lease: {error}"))?;
            Ok::<_, String>(file)
        })
        .await
        .map_err(|error| format!("write-lease task failed: {error}"))??;

        Ok(Self { _file: file })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    #[tokio::test]
    async fn independent_write_leases_serialize_for_the_same_project() {
        let project = tempfile::tempdir().expect("project");
        let first = CrossProcessWriteLease::acquire(project.path())
            .await
            .expect("first lease");
        let acquired = Arc::new(AtomicBool::new(false));
        let acquired_by_waiter = Arc::clone(&acquired);
        let project_path = project.path().to_path_buf();
        let waiter = tokio::spawn(async move {
            let lease = CrossProcessWriteLease::acquire(&project_path)
                .await
                .expect("second lease");
            acquired_by_waiter.store(true, Ordering::Release);
            lease
        });
        tokio::time::sleep(Duration::from_millis(75)).await;
        assert!(!acquired.load(Ordering::Acquire));
        drop(first);
        let second = tokio::time::timeout(Duration::from_secs(2), waiter)
            .await
            .expect("waiter timeout")
            .expect("waiter task");
        assert!(acquired.load(Ordering::Acquire));
        drop(second);
    }
}
