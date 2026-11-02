//! Bounded staged-only input; repository access stays on the serial Git worker.
use crate::{VcsError, git_worker::head_commit, model::CommitContext};
use std::hash::{DefaultHasher, Hash, Hasher};

pub fn read(repo: &git2::Repository) -> Result<CommitContext, VcsError> {
    const LIMIT: usize = 24 * 1024;
    let mut index = repo.index()?;
    index.read(true)?;
    if index.has_conflicts() || repo.state() != git2::RepositoryState::Clean {
        return Err(crate::git_worker::invalid(
            "Resolve the Git operation before generating a commit message",
        ));
    }
    let head = head_commit(repo)?;
    let tree = head.as_ref().map(git2::Commit::tree).transpose()?;
    let mut identity = DefaultHasher::new();
    head.as_ref().map(git2::Commit::id).hash(&mut identity);
    repo.head()
        .ok()
        .and_then(|reference| reference.name().ok().map(str::to_owned))
        .hash(&mut identity);
    for entry in index.iter() {
        entry.path.hash(&mut identity);
        entry.id.hash(&mut identity);
        entry.mode.hash(&mut identity);
        entry.flags.hash(&mut identity);
        entry.flags_extended.hash(&mut identity);
    }
    let mut options = git2::DiffOptions::new();
    options.context_lines(3).max_size(1024 * 1024);
    let diff = repo.diff_tree_to_index(tree.as_ref(), Some(&index), Some(&mut options))?;
    if diff.deltas().len() == 0 {
        return Err(crate::git_worker::invalid(
            "Select at least one file for the commit",
        ));
    }
    let mut patch = Vec::new();
    let mut truncated = false;
    let printed = diff.print(git2::DiffFormat::Patch, |_, _, line| {
        let prefix = matches!(line.origin(), '+' | '-' | ' ');
        if patch.len() + line.content().len() + usize::from(prefix) > LIMIT {
            truncated = true;
            return false;
        }
        if prefix {
            patch.push(match line.origin() {
                '+' => b'+',
                '-' => b'-',
                _ => b' ',
            });
        }
        patch.extend_from_slice(line.content());
        true
    });
    if let Err(error) = printed
        && !(truncated && error.code() == git2::ErrorCode::User)
    {
        return Err(error.into());
    }
    let mut patch = String::from_utf8_lossy(&patch).into_owned();
    if truncated {
        patch.push_str("\n[Staged diff truncated; additional changes omitted]\n");
    }
    Ok(CommitContext {
        fingerprint: identity.finish(),
        patch,
    })
}
