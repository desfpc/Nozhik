//! Flat file index and deterministic fuzzy ranking.
use crate::{FsError, tree::walker};
use fuzzy_matcher::{FuzzyMatcher, skim::SkimMatcherV2};
use std::path::{Path, PathBuf};
/// A ranked native path with a portable display path.
#[derive(Debug, Clone)]
pub struct FileMatch {
    /// Absolute native path for I/O.
    pub path: PathBuf,
    /// Root-relative path using forward slashes.
    pub relative: String,
    /// Fuzzy score, higher is better.
    pub score: i64,
}
/// Immutable index snapshot, used only in the filesystem worker.
#[derive(Default)]
pub struct FileIndex {
    files: Vec<FileMatch>,
}
impl FileIndex {
    /// Scans file paths, including ignored files except Git/application internals.
    /// # Errors
    /// Returns traversal errors instead of silently omitting inaccessible paths.
    pub fn scan(root: &Path) -> Result<Self, FsError> {
        let mut files = Vec::new();
        for entry in walker(root, None).build() {
            let entry = entry?;
            if entry.file_type().is_some_and(|t| t.is_file()) {
                files.push(FileMatch {
                    path: entry.path().into(),
                    relative: portable(
                        entry
                            .path()
                            .strip_prefix(root)
                            .map_err(|_| FsError::OutsideProject)?,
                    ),
                    score: 0,
                });
            }
        }
        Ok(Self { files })
    }
    /// Scores relative paths; empty queries return alphabetically ordered paths.
    #[must_use]
    pub fn search(&self, query: &str, limit: usize) -> Vec<FileMatch> {
        let matcher = SkimMatcherV2::default();
        let query = query.replace('\\', "/");
        let mut results: Vec<_> = self
            .files
            .iter()
            .filter_map(|file| {
                let score = if query.is_empty() {
                    Some(0)
                } else {
                    matcher.fuzzy_match(&file.relative, &query)
                }?;
                Some(FileMatch {
                    score,
                    ..file.clone()
                })
            })
            .collect();
        results.sort_by(|a, b| {
            b.score
                .cmp(&a.score)
                .then_with(|| a.relative.cmp(&b.relative))
        });
        results.truncate(limit);
        results
    }
}
/// Converts native components to a display/search path, leaving I/O paths native.
#[must_use]
pub fn portable(path: &Path) -> String {
    path.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fuzzy_paths_and_nested_ignores() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::create_dir(dir.path().join("src"))?;
        std::fs::write(dir.path().join("src/main.rs"), "")?;
        std::fs::write(dir.path().join("src/skip.rs"), "")?;
        std::fs::write(dir.path().join("src/.gitignore"), "skip.rs")?;
        let index = FileIndex::scan(dir.path())?;
        assert_eq!(index.search("s\\mnrs", 5)[0].relative, "src/main.rs");
        assert_eq!(index.search("skip", 10).len(), 1);
        assert!(index.search("zzz", 10).is_empty());
        assert_eq!(index.search("", 1).len(), 1);
        Ok(())
    }
}
