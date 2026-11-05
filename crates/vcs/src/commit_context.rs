//! Per-file staged evidence for commit messages. Repository access stays on the Git worker.
//!
//! A single truncated patch hides later files from a local model. This reader keeps every
//! staged path. Each text patch is capped, and the largest patches shrink first so one big
//! file cannot erase the rest of the commit.
use crate::{
    VcsError,
    git_worker::head_commit,
    model::{CommitContext, StagedFileChange, StagedStatus},
};
use git2::{Delta, DiffFlags};
use std::hash::{DefaultHasher, Hash, Hasher};

const PER_FILE_PATCH: usize = 8 * 1024;
const TOTAL_PATCH: usize = 512 * 1024;
const PATCH_FLOOR: usize = 1536;
const MAX_HUNKS: usize = 40;
const TEXT_DIFF_LIMIT: i64 = 8 * 1024 * 1024;

struct PartialFile {
    path: String,
    old_path: Option<String>,
    status: StagedStatus,
    binary: bool,
    insertions: u32,
    deletions: u32,
    patch: Vec<u8>,
    truncated: bool,
    hunks: Vec<String>,
    hunks_capped: bool,
}

/// Reads HEAD-to-index evidence for the staged commit.
///
/// # Errors
/// Returns an error when the repository is mid-operation, conflicted, empty of staged
/// changes, or the diff cannot be read. The index and worktree are not modified.
pub fn read(repo: &git2::Repository) -> Result<CommitContext, VcsError> {
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
    options.context_lines(3).max_size(TEXT_DIFF_LIMIT);
    let mut diff = repo.diff_tree_to_index(tree.as_ref(), Some(&index), Some(&mut options))?;
    diff.find_similar(None)?;
    if diff.deltas().len() == 0 {
        return Err(crate::git_worker::invalid(
            "Select at least one file for the commit",
        ));
    }
    let mut files = Vec::new();
    let printed = diff.print(git2::DiffFormat::Patch, |delta, _hunk, line| {
        observe(&mut files, &delta, &line);
        true
    });
    printed?;
    enforce_total_budget(&mut files);
    files.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then(left.old_path.cmp(&right.old_path))
    });
    Ok(CommitContext {
        fingerprint: identity.finish(),
        files: files.into_iter().map(finish).collect(),
    })
}

fn observe(files: &mut Vec<PartialFile>, delta: &git2::DiffDelta<'_>, line: &git2::DiffLine<'_>) {
    let (path, old_path, status) = identity(delta);
    if files.last().is_none_or(|file| file.path != path) {
        files.push(PartialFile {
            path,
            old_path,
            status,
            binary: delta.flags().contains(DiffFlags::BINARY),
            insertions: 0,
            deletions: 0,
            patch: Vec::new(),
            truncated: false,
            hunks: Vec::new(),
            hunks_capped: false,
        });
    }
    let Some(index) = files.len().checked_sub(1) else {
        return;
    };
    let Some(file) = files.get_mut(index) else {
        return;
    };
    if delta.flags().contains(DiffFlags::BINARY) || line.origin() == 'B' {
        file.binary = true;
        file.patch.clear();
        file.insertions = 0;
        file.deletions = 0;
        file.truncated = false;
        return;
    }
    if file.binary {
        return;
    }
    if line.origin() == 'H' {
        record_hunk(file, line.content());
    }
    match line.origin() {
        '+' => file.insertions = file.insertions.saturating_add(1),
        '-' => file.deletions = file.deletions.saturating_add(1),
        _ => {}
    }
    let prefix = matches!(line.origin(), '+' | '-' | ' ');
    let extra = line.content().len() + usize::from(prefix);
    if file.patch.len().saturating_add(extra) > PER_FILE_PATCH {
        file.truncated = true;
        return;
    }
    if prefix {
        file.patch.push(match line.origin() {
            '+' => b'+',
            '-' => b'-',
            _ => b' ',
        });
    }
    file.patch.extend_from_slice(line.content());
}

fn identity(delta: &git2::DiffDelta<'_>) -> (String, Option<String>, StagedStatus) {
    let status = match delta.status() {
        Delta::Added | Delta::Untracked => StagedStatus::Added,
        Delta::Deleted => StagedStatus::Deleted,
        Delta::Renamed => StagedStatus::Renamed,
        Delta::Copied => StagedStatus::Copied,
        _ => StagedStatus::Modified,
    };
    let new_path = delta.new_file().path().map(display_path);
    let old_path = delta.old_file().path().map(display_path);
    let path =
        new_path.unwrap_or_else(|| old_path.clone().unwrap_or_else(|| "(unknown)".to_owned()));
    let old_path = match status {
        StagedStatus::Renamed | StagedStatus::Copied => old_path.filter(|old| old != &path),
        _ => None,
    };
    (path, old_path, status)
}

fn display_path(path: &std::path::Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn record_hunk(file: &mut PartialFile, content: &[u8]) {
    let Ok(text) = std::str::from_utf8(content) else {
        return;
    };
    let Some(label) = text
        .split("@@")
        .nth(2)
        .map(str::trim)
        .filter(|label| !label.is_empty())
    else {
        return;
    };
    if file.hunks.len() >= MAX_HUNKS {
        file.hunks_capped = true;
        return;
    }
    file.hunks.push(label.chars().take(80).collect());
}

fn enforce_total_budget(files: &mut [PartialFile]) {
    let total = |files: &[PartialFile]| files.iter().map(|file| file.patch.len()).sum::<usize>();
    while total(files) > TOTAL_PATCH {
        let Some(index) = largest_above(files, PATCH_FLOOR) else {
            break;
        };
        truncate_patch(&mut files[index], PATCH_FLOOR);
    }
    while total(files) > TOTAL_PATCH {
        let Some(index) = largest_above(files, 0) else {
            break;
        };
        let overflow = total(files) - TOTAL_PATCH;
        let target = files[index].patch.len().saturating_sub(overflow);
        truncate_patch(&mut files[index], target);
    }
}

fn largest_above(files: &[PartialFile], floor: usize) -> Option<usize> {
    files
        .iter()
        .enumerate()
        .filter(|(_, file)| file.patch.len() > floor)
        .max_by_key(|(_, file)| file.patch.len())
        .map(|(index, _)| index)
}

fn truncate_patch(file: &mut PartialFile, max_len: usize) {
    if file.patch.len() <= max_len {
        return;
    }
    let mut end = max_len.min(file.patch.len());
    if let Some(newline) = file.patch.get(..end).and_then(|bytes| {
        bytes
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map(|index| index + 1)
    }) {
        end = newline;
    }
    // A cut inside a multibyte character would make the patch invalid UTF-8.
    while end > 0
        && file
            .patch
            .get(end)
            .is_some_and(|byte| byte & 0b1100_0000 == 0b1000_0000)
    {
        end -= 1;
    }
    file.patch.truncate(end);
    file.truncated = true;
}

fn finish(file: PartialFile) -> StagedFileChange {
    let mut patch = String::from_utf8_lossy(&file.patch).into_owned();
    if file.truncated {
        if !patch.ends_with('\n') {
            patch.push('\n');
        }
        patch.push_str("[Further changes in this file were omitted]\n");
    }
    let mut hunks = file.hunks;
    if file.hunks_capped {
        hunks.push("further hunks".to_owned());
    }
    StagedFileChange {
        path: file.path,
        old_path: file.old_path,
        status: file.status,
        binary: file.binary,
        insertions: file.insertions,
        deletions: file.deletions,
        patch,
        truncated: file.truncated,
        hunks,
    }
}

#[cfg(test)]
mod tests {
    use super::{PATCH_FLOOR, PartialFile, TOTAL_PATCH, enforce_total_budget};
    use crate::model::StagedStatus;

    fn partial(size: usize) -> PartialFile {
        PartialFile {
            path: "a.rs".into(),
            old_path: None,
            status: StagedStatus::Modified,
            binary: false,
            insertions: 1,
            deletions: 0,
            patch: vec![b'a'; size],
            truncated: false,
            hunks: Vec::new(),
            hunks_capped: false,
        }
    }

    #[test]
    fn total_budget_shrinks_the_largest_patch_and_keeps_every_file() {
        let mut files = vec![partial(400), partial(TOTAL_PATCH), partial(200)];
        files[0].path = "small.rs".into();
        files[1].path = "huge.rs".into();
        files[2].path = "tail.rs".into();
        enforce_total_budget(&mut files);
        assert!(files.iter().map(|file| file.patch.len()).sum::<usize>() <= TOTAL_PATCH);
        assert_eq!(files[0].patch.len(), 400);
        assert!(!files[0].truncated);
        assert_eq!(files[2].patch.len(), 200);
        assert!(!files[2].truncated);
        assert!(files[1].truncated);
        assert!(files[1].patch.len() <= PATCH_FLOOR);
    }
}
