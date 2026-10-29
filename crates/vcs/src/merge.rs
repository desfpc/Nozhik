//! Branch switching and merge lifecycle; all disk and repository access is worker-only.
use crate::{
    VcsError,
    git_worker::{invalid, require_clean, safe_disk_path, stage},
    model::{BranchInfo, MergeConflict},
};
use git2::{BranchType, Repository, RepositoryState};
use std::{fs, path::PathBuf};
type Result<T> = std::result::Result<T, VcsError>;

fn branch_commit<'a>(repo: &'a Repository, name: &str) -> Result<git2::Commit<'a>> {
    let branch = repo
        .find_branch(name, BranchType::Local)
        .or_else(|_| repo.find_branch(name, BranchType::Remote))?;
    Ok(branch.get().peel_to_commit()?)
}
pub fn branches(repo: &Repository, names: &[String]) -> Result<Vec<BranchInfo>> {
    let default = repo
        .find_reference("refs/remotes/origin/HEAD")
        .ok()
        .and_then(|r| r.symbolic_target().ok().flatten().map(str::to_owned));
    names
        .iter()
        .map(|name| {
            let commit = branch_commit(repo, name)?;
            let is_default = default
                .as_ref()
                .map_or(name == "main" || name == "master", |d| {
                    d.strip_prefix("refs/remotes/origin/") == Some(name.as_str())
                });
            Ok(BranchInfo {
                name: name.clone(),
                relative_time: super::git_worker::commit_info(&commit).relative_time,
                is_remote: repo.find_branch(name, BranchType::Local).is_err(),
                is_default,
            })
        })
        .collect()
}

/// Marker-free regular text files can be staged on Continue. Missing/binary/symlink
/// conflicts require an explicit index resolution (e.g. Include Selected Files).
pub fn conflicts(repo: &Repository) -> Result<Vec<MergeConflict>> {
    let index = repo.index()?;
    let mut result = Vec::new();
    for conflict in index.conflicts()? {
        let conflict = conflict?;
        let entry = conflict
            .our
            .as_ref()
            .or(conflict.their.as_ref())
            .or(conflict.ancestor.as_ref())
            .ok_or_else(|| invalid("Conflict has no path"))?;
        let path = PathBuf::from(
            std::str::from_utf8(&entry.path).map_err(|_| invalid("Non-UTF-8 conflict path"))?,
        );
        // Structural conflicts need an explicit index resolution. Only ordinary
        // same-path text conflicts can be resolved just by saving marker-free text.
        let same_path = conflict
            .our
            .as_ref()
            .zip(conflict.their.as_ref())
            .is_some_and(|(ours, theirs)| {
                ours.path == theirs.path
                    && ours.mode == theirs.mode
                    && matches!(ours.mode, 0o100_644 | 0o100_755)
            });
        let remaining = if same_path {
            remaining_markers(repo, &path)?.unwrap_or(1)
        } else {
            1
        };
        result.push(MergeConflict { path, remaining });
    }
    if repo.state() == RepositoryState::Merge {
        // An external git add must not make leftover markers appear resolved.
        let head = repo.head()?.peel_to_tree()?;
        let diff = repo.diff_tree_to_index(Some(&head), Some(&index), None)?;
        for delta in diff.deltas() {
            let Some(path) = delta.new_file().path() else {
                continue;
            };
            if result.iter().any(|c| c.path == path) {
                continue;
            }
            let remaining = remaining_markers(repo, path)?;
            let indexed_markers = if let Some(entry) = index.get_path(path, 0) {
                let blob = repo.find_blob(entry.id)?;
                if blob.size() <= 8 * 1024 * 1024 {
                    marker_count(blob.content()).unwrap_or(0)
                } else {
                    0
                }
            } else {
                0
            };
            // Keep marker-containing index versions in the resolution set even
            // after the saved worktree is fixed, so Continue stages that fix.
            if remaining.is_some_and(|n| n > 0) || indexed_markers > 0 {
                result.push(MergeConflict {
                    path: path.to_path_buf(),
                    remaining: remaining.unwrap_or(1),
                });
            }
        }
    }
    Ok(result)
}
fn remaining_markers(repo: &Repository, path: &std::path::Path) -> Result<Option<usize>> {
    let full = safe_disk_path(repo, path)?;
    if !fs::symlink_metadata(&full)
        .is_ok_and(|m| m.file_type().is_file() && m.len() <= 8 * 1024 * 1024)
    {
        return Ok(None);
    }
    Ok(marker_count(&fs::read(full)?))
}
fn marker_count(bytes: &[u8]) -> Option<usize> {
    if bytes.contains(&0) {
        return None;
    }
    let text = String::from_utf8_lossy(bytes);
    let markers = text
        .lines()
        .filter(|l| {
            l.starts_with("<<<<<<<") || l.starts_with("=======") || l.starts_with(">>>>>>>")
        })
        .count();
    Some(markers.div_ceil(3))
}
/// Creating from HEAD preserves both the index and working changes exactly.
pub fn create(repo: &Repository, name: &str) -> Result<()> {
    if repo.state() != RepositoryState::Clean {
        return Err(invalid(
            "Finish the current repository operation before creating a branch",
        ));
    }
    let head = repo.head()?.peel_to_commit()?;
    let branch = repo.branch(name, &head, false)?;
    repo.set_head(branch.get().name()?)?;
    Ok(())
}
pub fn preview(repo: &Repository, name: &str) -> Result<usize> {
    let ours = repo.head()?.peel_to_commit()?;
    let theirs = branch_commit(repo, name)?;
    let index = repo.merge_commits(&ours, &theirs, None)?;
    Ok(index
        .conflicts()?
        .collect::<std::result::Result<Vec<_>, _>>()?
        .len())
}
pub fn begin(repo: &Repository, name: &str) -> Result<()> {
    require_clean(repo)?;
    // Check identity before creating a pending operation.
    repo.signature()?;
    let target = branch_commit(repo, name)?;
    let annotated = repo.find_annotated_commit(target.id())?;
    let (analysis, _) = repo.merge_analysis(&[&annotated])?;
    if analysis.is_up_to_date() {
        return Ok(());
    }
    repo.merge(
        &[&annotated],
        None,
        Some(
            git2::build::CheckoutBuilder::new()
                .safe()
                .allow_conflicts(true)
                .conflict_style_merge(true)
                .overwrite_ignored(false),
        ),
    )?;
    if !repo.index()?.has_conflicts() {
        finish(repo)?;
    }
    Ok(())
}
pub fn finish(repo: &Repository) -> Result<()> {
    if repo.state() != RepositoryState::Merge {
        return Err(invalid("No merge is in progress"));
    }
    let signature = repo.signature()?;
    let conflicts = conflicts(repo)?;
    if conflicts.iter().any(|c| c.remaining > 0) {
        return Err(invalid(
            "Resolve all conflict markers before continuing the merge",
        ));
    }
    let paths: Vec<_> = conflicts.into_iter().map(|c| c.path).collect();
    if !paths.is_empty() {
        stage(repo, &paths, true)?;
    }
    let mut index = repo.index()?;
    if index.has_conflicts() {
        return Err(invalid("Unresolved index conflicts"));
    }
    let head = repo.head()?.peel_to_commit()?;
    let merge_head = fs::read_to_string(repo.path().join("MERGE_HEAD"))?;
    let mut parents = vec![head];
    for oid in merge_head.lines() {
        parents.push(repo.find_commit(git2::Oid::from_str(oid)?)?);
    }
    let tree = repo.find_tree(index.write_tree()?)?;
    let message = repo.message().unwrap_or_else(|_| "Merge branch".into());
    repo.commit(
        Some("HEAD"),
        &signature,
        &signature,
        &message,
        &tree,
        &parents.iter().collect::<Vec<_>>(),
    )?;
    repo.cleanup_state()?;
    Ok(())
}
pub fn abort(repo: &Repository) -> Result<()> {
    if repo.state() != RepositoryState::Merge {
        return Err(invalid("No merge is in progress"));
    }
    let head = repo.head()?.peel_to_commit()?;
    repo.reset(head.as_object(), git2::ResetType::Hard, None)?;
    repo.cleanup_state()?;
    Ok(())
}

fn checkout(repo: &Repository, name: &str) -> Result<()> {
    if repo.state() != RepositoryState::Clean {
        return Err(invalid(
            "Finish the current repository operation before switching",
        ));
    }
    let local = repo.find_branch(name, BranchType::Local).ok();
    let target = branch_commit(repo, name)?;
    let local_name = if local.is_some() {
        name
    } else {
        name.split_once('/').map_or(name, |(_, n)| n)
    };
    if local.is_none() && repo.find_branch(local_name, BranchType::Local).is_ok() {
        return Err(invalid(
            "A local branch with this name already exists; select it explicitly",
        ));
    }
    let mut created = if local.is_none() {
        let mut branch = repo.branch(local_name, &target, false)?;
        if let Err(error) = branch.set_upstream(Some(name)) {
            branch.delete()?;
            return Err(error.into());
        }
        Some(branch)
    } else {
        None
    };
    if let Err(error) = repo.checkout_tree(
        target.as_object(),
        Some(
            git2::build::CheckoutBuilder::new()
                .safe()
                .overwrite_ignored(false),
        ),
    ) {
        if let Some(branch) = &mut created {
            branch.delete()?;
        }
        return Err(error.into());
    }
    repo.set_head(&format!("refs/heads/{local_name}"))?;
    Ok(())
}
fn saved_stash(repo: &mut Repository, branch: &str) -> Result<Option<usize>> {
    let tag = format!("Nozhik branch: {branch}");
    let mut found = None;
    repo.stash_foreach(|i, message, _| {
        if message.ends_with(&tag) {
            found = Some(i);
            false
        } else {
            true
        }
    })?;
    Ok(found)
}
pub fn switch(repo: &mut Repository, name: &str, leave: bool) -> Result<()> {
    if repo.state() != RepositoryState::Clean {
        return Err(invalid(
            "Finish the current repository operation before switching",
        ));
    }
    let source = repo.head()?.shorthand()?.to_owned();
    // Resolve the target before stashing any user work.
    branch_commit(repo, name)?;
    let dirty = !super::git_worker::status(repo)?.changes.is_empty();
    let stashed = if leave && dirty {
        let signature = repo.signature()?;
        repo.stash_save(
            &signature,
            &format!("Nozhik branch: {source}"),
            Some(git2::StashFlags::INCLUDE_UNTRACKED),
        )?;
        true
    } else {
        false
    };
    if let Err(error) = checkout(repo, name) {
        if stashed {
            repo.stash_pop(
                0,
                Some(git2::StashApplyOptions::new().reinstantiate_index()),
            )?;
        }
        return Err(error);
    }
    let destination = repo.head()?.shorthand()?.to_owned();
    if let Some(i) = saved_stash(repo, &destination)? {
        // Failed apply retains the stash; never drop work until restoration succeeds.
        repo.stash_pop(
            i,
            Some(git2::StashApplyOptions::new().reinstantiate_index()),
        )?;
    }
    Ok(())
}
