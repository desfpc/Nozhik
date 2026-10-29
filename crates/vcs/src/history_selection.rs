//! Read-only selected-commit union, newest file revisions and historical links.
use crate::{
    VcsError,
    git_worker::{change_from_delta, commit_info, invalid, parent_tree, safe_disk_path},
    model::{HistoryFile, HistorySelection},
};
use git2::{Oid, Repository};
use std::{
    collections::{BTreeMap, HashSet},
    path::Path,
};
type Result<T> = std::result::Result<T, VcsError>;

fn commit_ids(shas: &[String]) -> Result<Vec<Oid>> {
    if shas.is_empty() || shas.len() > 10_000 {
        return Err(invalid("Select between 1 and 10,000 distinct commits"));
    }
    let ids = shas
        .iter()
        .map(|sha| Oid::from_str(sha))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if ids.iter().collect::<HashSet<_>>().len() != ids.len() {
        return Err(invalid("Duplicate commit selection"));
    }
    Ok(ids)
}
pub fn details(repo: &Repository, shas: &[String]) -> Result<HistorySelection> {
    let ids = commit_ids(shas)?;
    let mut commits = Vec::with_capacity(ids.len());
    let mut files = BTreeMap::new();
    // A missing/non-GitHub origin simply disables the historical browser action.
    let base = github_base(repo).ok();
    for id in ids {
        let commit = repo.find_commit(id)?;
        let tree = commit.tree()?;
        let parent = parent_tree(&commit)?;
        let mut diff = repo.diff_tree_to_tree(parent.as_ref(), Some(&tree), None)?;
        diff.find_similar(None)?;
        for delta in diff.deltas() {
            let Some(change) = change_from_delta(&delta) else {
                continue;
            };
            let file = files.entry(change.path.clone()).or_insert_with(|| {
                let worktree_exists = safe_disk_path(repo, &change.path).is_ok_and(|p| p.is_file());
                let github_url = base.as_ref().and_then(|base| {
                    let (revision, path) = if tree.get_path(&change.path).is_ok() {
                        (id, change.path.as_path())
                    } else {
                        (
                            commit.parent_id(0).ok()?,
                            change.old_path.as_deref().unwrap_or(&change.path),
                        )
                    };
                    Some(format!("{base}/blob/{revision}/{}", encode_path(path)?))
                });
                HistoryFile {
                    change,
                    commits: Vec::new(),
                    worktree_exists,
                    github_url,
                }
            });
            let sha = id.to_string();
            if !file.commits.contains(&sha) {
                file.commits.push(sha);
            }
        }
        commits.push(commit_info(&commit));
    }
    Ok(HistorySelection {
        commits,
        files: files.into_values().collect(),
    })
}

/// Shared canonical GitHub repository URL for history commit/file actions.
pub fn github_base(repo: &Repository) -> Result<String> {
    let remote = repo.find_remote("origin")?;
    let url = remote.url()?;
    let path = url
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("git@github.com:"))
        .or_else(|| url.strip_prefix("ssh://git@github.com/"))
        .ok_or_else(|| invalid("View on GitHub requires a github.com origin"))?
        .trim_end_matches('/')
        .trim_end_matches(".git");
    if path.split('/').count() != 2
        || path.split('/').any(str::is_empty)
        || path
            .chars()
            .any(|c| !(c.is_ascii_alphanumeric() || matches!(c, '/' | '-' | '_' | '.')))
    {
        return Err(invalid("Invalid GitHub origin path"));
    }
    Ok(format!("https://github.com/{path}"))
}
fn encode_path(path: &Path) -> Option<String> {
    use std::fmt::Write;
    let mut encoded = String::new();
    // Git paths use forward slashes even when the worktree runs on Windows.
    let portable = path
        .iter()
        .map(|component| component.to_str())
        .collect::<Option<Vec<_>>>()?
        .join("/");
    for byte in portable.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else {
            write!(&mut encoded, "%{byte:02X}").ok()?;
        }
    }
    Some(encoded)
}
