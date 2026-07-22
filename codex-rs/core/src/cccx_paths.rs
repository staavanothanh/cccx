use codex_utils_absolute_path::AbsolutePathBuf;
use std::path::{Path, PathBuf};

/// Resolve a target through its nearest existing ancestor. This gives every
/// CCCX filesystem boundary the same physical symlink/junction semantics,
/// including targets that do not exist yet.
pub(crate) fn resolve_physical_path(path: &Path, cwd: &Path) -> Result<PathBuf, String> {
    let absolute =
        AbsolutePathBuf::resolve_path_against_base(path.to_path_buf(), cwd).into_path_buf();
    let mut existing = absolute.as_path();
    let mut missing_segments = Vec::new();
    while !existing.exists() {
        let Some(name) = existing.file_name() else {
            return Err(format!(
                "cannot resolve target path `{}`",
                absolute.display()
            ));
        };
        missing_segments.push(name.to_os_string());
        existing = existing
            .parent()
            .ok_or_else(|| format!("cannot resolve target path `{}`", absolute.display()))?;
    }

    let mut resolved = std::fs::canonicalize(existing).map_err(|error| {
        format!(
            "cannot canonicalize target ancestor `{}`: {error}",
            existing.display()
        )
    })?;
    for segment in missing_segments.iter().rev() {
        resolved.push(segment);
    }
    Ok(resolved)
}

pub(crate) fn resolve_physical_paths(
    paths: impl IntoIterator<Item = String>,
    cwd: &Path,
) -> Result<Vec<String>, String> {
    paths
        .into_iter()
        .map(|path| {
            resolve_physical_path(Path::new(&path), cwd)
                .map(|path| path.to_string_lossy().into_owned())
        })
        .collect()
}

pub(crate) fn path_is_within(path: &Path, root: &Path) -> bool {
    #[cfg(windows)]
    {
        let path = path.to_string_lossy().replace('\\', "/").to_lowercase();
        let root = root.to_string_lossy().replace('\\', "/").to_lowercase();
        path == root
            || (path.len() > root.len()
                && path.as_bytes().get(root.len()) == Some(&b'/')
                && path.starts_with(&root))
    }
    #[cfg(not(windows))]
    {
        path.starts_with(root)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_new_target_through_existing_ancestor() {
        let temp = tempfile::tempdir().expect("tempdir");
        let existing = temp.path().join("existing");
        std::fs::create_dir(&existing).expect("existing directory");

        let resolved = resolve_physical_path(Path::new("existing/new/file.txt"), temp.path())
            .expect("new target should resolve");

        assert_eq!(
            resolved,
            std::fs::canonicalize(existing)
                .expect("canonical existing")
                .join("new/file.txt")
        );
    }
}
