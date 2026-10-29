//! Read-only Git membership classification for filesystem workers.
use crate::VcsError;
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

fn repository(root: &Path) -> Result<Option<git2::Repository>, VcsError> {
    match git2::Repository::discover(root) {
        Ok(repo) if !repo.is_bare() => Ok(Some(repo)),
        Ok(_) => Ok(None),
        Err(error) if error.code() == git2::ErrorCode::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}
/// Classifies displayed paths using actual Git ignores.
///
/// Tracked files and directories containing tracked descendants are never ignored. No repository returns `None`.
/// Call only from a background worker; no repository handle crosses this call.
/// # Errors
/// Reports discovery, index and ignore evaluation errors without changing any files.
pub fn ignored_paths(
    root: &Path,
    paths: &[PathBuf],
) -> Result<Option<BTreeSet<PathBuf>>, VcsError> {
    let Some(repo) = repository(root)? else {
        return Ok(None);
    };
    let index = repo.index()?;
    let canonical_root = root.canonicalize()?;
    let workdir = repo
        .workdir()
        .ok_or_else(|| VcsError::Invalid("Missing Git worktree".into()))?
        .canonicalize()?;
    let mut ignored = BTreeSet::new();
    for path in paths {
        let project_relative = path
            .strip_prefix(root)
            .map_err(|error| VcsError::Invalid(error.to_string()))?;
        let canonical_path = canonical_root.join(project_relative);
        let relative = canonical_path
            .strip_prefix(&workdir)
            .map_err(|error| VcsError::Invalid(error.to_string()))?;
        let mut prefix = relative.as_os_str().to_os_string();
        prefix.push("/");
        let contains_tracked = match index.find_prefix(Path::new(&prefix)) {
            Ok(_) => true,
            Err(error) if error.code() == git2::ErrorCode::NotFound => false,
            Err(error) => return Err(error.into()),
        };
        let tracked = (0..=3).any(|stage| index.get_path(relative, stage).is_some());
        if !tracked && !contains_tracked && repo.status_should_ignore(relative)? {
            ignored.insert(path.clone());
        }
    }
    Ok(Some(ignored))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tracked_descendants_override_ignored_ancestors() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let root = dir.path().canonicalize()?;
        let repo = git2::Repository::init(&root)?;
        std::fs::create_dir_all(root.join("vendor/local"))?;
        std::fs::create_dir_all(root.join("vendor/ignored"))?;
        let tracked = root.join("vendor/local/Source.php");
        std::fs::write(&tracked, "<?php class Source {}")?;
        let mut index = repo.index()?;
        index.add_path(Path::new("vendor/local/Source.php"))?;
        index.write()?;
        std::fs::write(root.join(".gitignore"), "vendor/\n")?;
        let paths = vec![
            tracked,
            root.join("vendor"),
            root.join("vendor/local"),
            root.join("vendor/ignored"),
        ];
        assert_eq!(
            ignored_paths(&root, &paths)?,
            Some(BTreeSet::from([root.join("vendor/ignored")]))
        );
        Ok(())
    }
}
