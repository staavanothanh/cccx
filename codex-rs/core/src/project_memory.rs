use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncSeekExt;
use tokio::io::AsyncWriteExt;

const MEMORY_FILE: &str = "MEMORY.md";
const MAX_LINES: usize = 200;
const MAX_CONTEXT_BYTES: u64 = 64 * 1024;
const MAX_MEMORY_FILE_BYTES: u64 = 128 * 1024;
const MAX_NOTE_CHARS: usize = 16 * 1024;

pub struct ProjectMemory {
    base_path: PathBuf,
}

impl ProjectMemory {
    /// Create the durable, project-scoped memory store. CCCX_HOME is the
    /// ownership boundary; the workspace only contributes a stable hash, so
    /// project memory is never written into the checked-out repository.
    pub fn new(cccx_home: &Path, project_root: &Path) -> Self {
        Self {
            base_path: project_storage_root(cccx_home, project_root).join("memory"),
        }
    }

    pub(crate) fn memory_file_path(&self) -> PathBuf {
        self.base_path.join(MEMORY_FILE)
    }

    pub async fn load_context(&self) -> Result<String, std::io::Error> {
        let path = self.memory_file_path();
        match fs::File::open(&path).await {
            Ok(file) => {
                let mut bytes = Vec::new();
                file.take(MAX_CONTEXT_BYTES).read_to_end(&mut bytes).await?;
                let content = String::from_utf8_lossy(&bytes);
                let lines: Vec<&str> = content.lines().take(MAX_LINES).collect();
                Ok(lines.join("\n"))
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
            Err(err) => Err(err),
        }
    }

    pub async fn remember(&self, note: &str) -> Result<(), std::io::Error> {
        fs::create_dir_all(&self.base_path).await?;
        let path = self.memory_file_path();
        let timestamp = chrono::Utc::now().to_rfc3339();
        let bounded_note = note.chars().take(MAX_NOTE_CHARS).collect::<String>();
        let entry = format!("\n## {timestamp}\n\n{bounded_note}\n");
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await?;
        file.write_all(entry.as_bytes()).await?;
        Self::truncate_if_needed(&path).await?;
        Ok(())
    }

    async fn truncate_if_needed(path: &Path) -> Result<(), std::io::Error> {
        if fs::metadata(path).await?.len() > MAX_MEMORY_FILE_BYTES {
            let mut file = fs::File::open(path).await?;
            let len = file.metadata().await?.len();
            file.seek(std::io::SeekFrom::Start(
                len.saturating_sub(MAX_CONTEXT_BYTES),
            ))
            .await?;
            let mut tail = Vec::new();
            file.read_to_end(&mut tail).await?;
            if len > MAX_CONTEXT_BYTES
                && let Some(newline) = tail.iter().position(|byte| *byte == b'\n')
            {
                tail.drain(..=newline);
            }
            fs::write(path, tail).await?;
        }
        let content = fs::read_to_string(path).await?;
        let lines: Vec<&str> = content.lines().collect();
        if lines.len() > MAX_LINES * 2 {
            let skip_count = lines.len().saturating_sub(MAX_LINES * 2 - MAX_LINES);
            let kept: Vec<&str> = lines.into_iter().skip(skip_count).collect();
            fs::write(path, kept.join("\n")).await?;
        }
        Ok(())
    }
}

pub(crate) fn project_storage_root(cccx_home: &Path, project_root: &Path) -> PathBuf {
    let canonical_root =
        dunce::canonicalize(project_root).unwrap_or_else(|_| project_root.to_path_buf());
    let project_hash = format!(
        "{:x}",
        Sha256::digest(canonical_root.as_os_str().as_encoded_bytes())
    );
    cccx_home.join("projects").join(project_hash)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn missing_project_memory_loads_as_empty_context() {
        let dir = TempDir::new().unwrap();
        let memory = ProjectMemory::new(dir.path(), dir.path());

        assert_eq!(memory.load_context().await.unwrap(), "");
    }

    #[tokio::test]
    async fn remember_appends_explicit_notes_under_cccx_home_project_memory() {
        let home = TempDir::new().unwrap();
        let project = TempDir::new().unwrap();
        let memory = ProjectMemory::new(home.path(), project.path());

        memory
            .remember("User prefers 9router-only routing.")
            .await
            .unwrap();
        memory
            .remember("Do not use direct provider credentials.")
            .await
            .unwrap();

        let context = memory.load_context().await.unwrap();
        assert!(context.contains("User prefers 9router-only routing."));
        assert!(context.contains("Do not use direct provider credentials."));
        assert!(memory.memory_file_path().exists());
    }

    #[tokio::test]
    async fn loaded_memory_context_is_bounded_to_prevent_raw_context_dump() {
        let home = TempDir::new().unwrap();
        let project = TempDir::new().unwrap();
        let memory = ProjectMemory::new(home.path(), project.path());
        let path = memory.memory_file_path();
        fs::create_dir_all(path.parent().unwrap()).await.unwrap();
        let content = (0..(MAX_LINES + 20))
            .map(|idx| format!("line-{idx}"))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(&path, content).await.unwrap();

        let context = memory.load_context().await.unwrap();

        assert_eq!(context.lines().count(), MAX_LINES);
        assert!(context.contains("line-0"));
        assert!(!context.contains(&format!("line-{MAX_LINES}")));
    }

    #[tokio::test]
    async fn loaded_memory_context_is_bounded_even_for_one_huge_line() {
        let home = TempDir::new().unwrap();
        let project = TempDir::new().unwrap();
        let memory = ProjectMemory::new(home.path(), project.path());
        let path = memory.memory_file_path();
        fs::create_dir_all(path.parent().unwrap()).await.unwrap();
        fs::write(&path, vec![b'x'; (MAX_CONTEXT_BYTES as usize) * 2])
            .await
            .unwrap();

        let context = memory.load_context().await.unwrap();

        assert!(context.len() <= MAX_CONTEXT_BYTES as usize);
    }

    // ── Phase 9 tests ──

    /// Phase 9: Project Memory survives "restart" — write with one instance,
    /// load with fresh instance (simulates session restart).
    /// task.md: "`/remember` survives restart and loads into Main CC context."
    #[tokio::test]
    async fn p9_project_memory_survives_restart() {
        let home = TempDir::new().unwrap();
        let project = TempDir::new().unwrap();

        // Session 1: remember two facts
        let memory1 = ProjectMemory::new(home.path(), project.path());
        memory1.remember("Use 9router-only routing.").await.unwrap();
        memory1
            .remember("Do not use direct provider keys.")
            .await
            .unwrap();
        drop(memory1);

        // Session 2 (fresh ProjectMemory for same project): load context
        let memory2 = ProjectMemory::new(home.path(), project.path());
        let context = memory2.load_context().await.unwrap();

        assert!(
            context.contains("Use 9router-only routing."),
            "Phase 9: memory must survive restart"
        );
        assert!(
            context.contains("Do not use direct provider keys."),
            "Phase 9: all remembered facts must persist"
        );
    }

    /// Phase 8: verify memory file is created under CCCX_HOME/projects/<hash>/memory/
    /// when remember() is called. Proves storage location contract.
    #[tokio::test]
    async fn p8_project_memory_creates_file_under_cccx_home() {
        let home = TempDir::new().unwrap();
        let project = TempDir::new().unwrap();
        let memory = ProjectMemory::new(home.path(), project.path());
        memory.remember("phase-9-test").await.unwrap();

        let expected_file = memory.memory_file_path();
        assert!(
            expected_file.exists(),
            "Phase 8: memory file must exist under CCCX_HOME after remember"
        );
        let content = tokio::fs::read_to_string(&expected_file).await.unwrap();
        assert!(
            content.contains("phase-9-test"),
            "Phase 9: content must be in file"
        );
    }
}
