use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::io::AsyncWriteExt;

const MEMORY_FILE: &str = "MEMORY.md";
const MAX_LINES: usize = 200;

pub struct ProjectMemory {
    base_path: PathBuf,
}

impl ProjectMemory {
    pub fn new(project_root: &Path) -> Self {
        Self {
            base_path: project_root.join(".cccx").join("memory"),
        }
    }

    pub async fn load_context(&self) -> Result<String, std::io::Error> {
        let path = self.base_path.join(MEMORY_FILE);
        match fs::read_to_string(&path).await {
            Ok(content) => {
                let lines: Vec<&str> = content.lines().take(MAX_LINES).collect();
                Ok(lines.join("\n"))
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
            Err(err) => Err(err),
        }
    }

    pub async fn remember(&self, note: &str) -> Result<(), std::io::Error> {
        fs::create_dir_all(&self.base_path).await?;
        let path = self.base_path.join(MEMORY_FILE);
        let timestamp = chrono::Utc::now().to_rfc3339();
        let entry = format!("\n## {timestamp}\n\n{note}\n");
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn missing_project_memory_loads_as_empty_context() {
        let dir = TempDir::new().unwrap();
        let memory = ProjectMemory::new(dir.path());

        assert_eq!(memory.load_context().await.unwrap(), "");
    }

    #[tokio::test]
    async fn remember_appends_explicit_notes_under_project_cccx_memory() {
        let dir = TempDir::new().unwrap();
        let memory = ProjectMemory::new(dir.path());

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
        assert!(
            dir.path()
                .join(".cccx")
                .join("memory")
                .join(MEMORY_FILE)
                .exists()
        );
    }

    #[tokio::test]
    async fn loaded_memory_context_is_bounded_to_prevent_raw_context_dump() {
        let dir = TempDir::new().unwrap();
        let memory = ProjectMemory::new(dir.path());
        let path = dir.path().join(".cccx").join("memory").join(MEMORY_FILE);
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
}
