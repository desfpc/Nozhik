//! libgit2 operations. Repository handles are created and used on this worker only.
use crate::{
    Outcome, Task, VcsError,
    model::{
        ChangeStatus, CommitInfo, DiffHunk, DiffLine, DiffLineKind, DiffTarget, FileChange,
        FileDiff, GitRepoStatus,
    },
};
use git2::{BranchType, Delta, Diff, DiffOptions, ErrorCode, Repository, Status};
use std::{
    fs,
    io::Write,
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

type Result<T> = std::result::Result<T, VcsError>;
#[derive(Default)]
pub struct GitWorker {
    repo: Option<Repository>,
}
impl GitWorker {
    #[allow(
        clippy::too_many_lines,
        reason = "Exhaustive serial Git command dispatch keeps every command on the owning worker"
    )]
    pub fn execute(&mut self, task: Task) -> Result<Outcome> {
        if let Task::Open(path) = task {
            self.repo = None;
            match Repository::discover(path) {
                Ok(repo) if !repo.is_bare() => self.repo = Some(repo),
                Ok(_) => return Ok(Outcome::Status(None)),
                Err(error) if error.code() == ErrorCode::NotFound => {
                    return Ok(Outcome::Status(None));
                }
                Err(error) => return Err(error.into()),
            }
            return self.snapshot();
        }
        let repo = self
            .repo
            .as_mut()
            .ok_or_else(|| invalid("Open a Git repository first"))?;
        let message = match task {
            Task::Refresh => return self.snapshot(),
            Task::Stage { paths, staged } => {
                stage(repo, &paths, staged)?;
                "Index updated"
            }
            Task::Diff(target) => return Ok(Outcome::Diff(file_diff(repo, &target)?)),
            Task::History(limit) => return Ok(Outcome::History(history(repo, limit)?)),
            Task::Details(sha) => {
                let commit = repo.find_commit(git2::Oid::from_str(&sha)?)?;
                let tree = commit.tree()?;
                let parent = parent_tree(&commit)?;
                let mut diff = repo.diff_tree_to_tree(parent.as_ref(), Some(&tree), None)?;
                diff.find_similar(None)?;
                let files = diff
                    .deltas()
                    .filter_map(|delta| change_from_delta(&delta))
                    .collect();
                return Ok(Outcome::Details {
                    commit: commit_info(&commit),
                    files,
                });
            }
            Task::Commit { summary, body } => {
                return Ok(Outcome::Committed(commit(repo, &summary, &body)?));
            }
            Task::Fetch => {
                fetch(repo)?;
                "Fetched origin"
            }
            Task::Push => {
                push(repo)?;
                "Pushed origin"
            }
            Task::Pull => {
                pull(repo)?;
                "Branch is up to date"
            }
            Task::Checkout(name) => {
                checkout(repo, &name, false)?;
                "Branch switched"
            }
            Task::SwitchBranch {
                name,
                leave_changes,
            } => {
                crate::merge::switch(repo, &name, leave_changes)?;
                "Branch switched"
            }
            Task::PreviewMerge(branch) => {
                return Ok(Outcome::MergePreview {
                    conflicts: crate::merge::preview(repo, &branch)?,
                    branch,
                });
            }
            Task::Merge(branch) => {
                crate::merge::begin(repo, &branch)?;
                "Merge updated; resolve any conflicts before continuing"
            }
            Task::ContinueMerge => {
                crate::merge::finish(repo)?;
                "Merge completed"
            }
            Task::AbortMerge => {
                crate::merge::abort(repo)?;
                "Merge aborted"
            }
            Task::DiscardMany(paths) => {
                for path in &paths {
                    safe_disk_path(repo, path)?;
                }
                for path in paths {
                    discard(repo, &path)?;
                }
                "Changes discarded on disk; editor buffers retained"
            }
            Task::NewBranch(name) => {
                crate::merge::create(repo, &name)?;
                "Branch created"
            }
            Task::Discard(path) => {
                discard(repo, &path)?;
                "Changes discarded on disk; editor buffers retained"
            }
            Task::IgnoreExtension(extension) => {
                if extension.is_empty() || extension.contains(['/', '\\', '\n', '\r', '\0']) {
                    return Err(invalid("Invalid file extension"));
                }
                let mut pattern = String::from("*.");
                for ch in extension.chars() {
                    if matches!(ch, '\\' | ' ' | '#' | '!' | '*' | '?' | '[' | ']') {
                        pattern.push('\\');
                    }
                    pattern.push(ch);
                }
                append_ignore(repo, &pattern)?;
                "Added extension to .gitignore"
            }
            Task::OpenExternal { paths, reveal } => {
                for path in paths {
                    open_external(&safe_disk_path(repo, &path)?, reveal)?;
                }
                "Opened selected paths"
            }
            Task::Ignore(path) => {
                ignore_file(repo, &path)?;
                "Added to .gitignore (tracked files remain tracked)"
            }
            Task::Open(_) => return Err(invalid("Unexpected open command")),
        };
        Ok(Outcome::Done(message.into()))
    }
    fn snapshot(&self) -> Result<Outcome> {
        self.repo
            .as_ref()
            .map(status)
            .transpose()
            .map(Outcome::Status)
    }
}
pub fn invalid(message: &str) -> VcsError {
    VcsError::Invalid(message.into())
}
pub fn workdir(repo: &Repository) -> Result<&Path> {
    repo.workdir()
        .ok_or_else(|| invalid("Bare repositories are not supported"))
}
pub fn head_commit(repo: &Repository) -> Result<Option<git2::Commit<'_>>> {
    match repo.head() {
        Ok(head) => Ok(Some(head.peel_to_commit()?)),
        Err(error) if matches!(error.code(), ErrorCode::UnbornBranch | ErrorCode::NotFound) => {
            Ok(None)
        }
        Err(error) => Err(error.into()),
    }
}
fn parent_tree<'repo>(commit: &git2::Commit<'repo>) -> Result<Option<git2::Tree<'repo>>> {
    if commit.parent_count() == 0 {
        return Ok(None);
    }
    Ok(Some(commit.parent(0)?.tree()?))
}
pub fn status(repo: &Repository) -> Result<GitRepoStatus> {
    let mut changes = status_changes(repo)?;
    let conflicts = crate::merge::conflicts(repo)?;
    for change in &mut changes {
        if conflicts
            .iter()
            .any(|c| c.path == change.path && c.remaining > 0)
        {
            change.status = ChangeStatus::Conflicted;
        }
    }
    let head = head_commit(repo)?;
    let detached = repo.head_detached()?;
    let branch = if detached {
        format!(
            "Detached {}",
            head.as_ref()
                .map(|c| c.id().to_string()[..7].to_owned())
                .unwrap_or_default()
        )
    } else {
        match repo.head() {
            Ok(reference) => reference.shorthand().unwrap_or("HEAD").to_owned(),
            Err(error) if error.code() == ErrorCode::UnbornBranch => repo
                .find_reference("HEAD")?
                .symbolic_target()?
                .unwrap_or("refs/heads/HEAD")
                .trim_start_matches("refs/heads/")
                .to_owned(),
            Err(error) => return Err(error.into()),
        }
    };
    let upstream_branch = if detached {
        None
    } else {
        repo.find_branch(&branch, BranchType::Local)
            .ok()
            .and_then(|b| b.upstream().ok())
    };
    let upstream = upstream_branch
        .as_ref()
        .and_then(|b| b.name().ok().flatten())
        .map(str::to_owned);
    let (ahead, behind) = match (
        head.as_ref(),
        upstream_branch.as_ref().and_then(|b| b.get().target()),
    ) {
        (Some(head), Some(target)) => repo.graph_ahead_behind(head.id(), target)?,
        _ => (0, 0),
    };
    let mut branches = Vec::new();
    for branch in repo.branches(None)? {
        if let Some(name) = branch?.0.name()?
            && !name.ends_with("/HEAD")
        {
            branches.push(name.to_owned());
        }
    }
    branches.sort();
    Ok(GitRepoStatus {
        root: workdir(repo)?.canonicalize()?,
        head_sha: head.as_ref().map(|c| c.id().to_string()),
        branch,
        detached,
        has_upstream: upstream.is_some(),
        upstream,
        ahead,
        behind,
        changes,
        branch_info: crate::merge::branches(repo, &branches)?,
        conflicts,
        branches,
        has_origin: repo.find_remote("origin").is_ok(),
        operation: (repo.state() != git2::RepositoryState::Clean)
            .then(|| format!("{:?}", repo.state())),
    })
}
fn status_changes(repo: &Repository) -> Result<Vec<FileChange>> {
    let mut options = git2::StatusOptions::new();
    options
        .include_untracked(true)
        .recurse_untracked_dirs(true)
        .renames_head_to_index(true)
        .renames_index_to_workdir(true);
    let entries = repo.statuses(Some(&mut options))?;
    let mut changes = Vec::new();
    for entry in &entries {
        let flags = entry.status();
        let delta = entry.index_to_workdir().or_else(|| entry.head_to_index());
        let Some(delta) = delta else {
            continue;
        };
        let Some(mut change) = change_from_delta(&delta) else {
            continue;
        };
        if flags.is_conflicted() {
            change.status = ChangeStatus::Conflicted;
        }
        if flags.intersects(Status::INDEX_RENAMED | Status::WT_RENAMED) {
            change.status = ChangeStatus::Renamed;
            change.old_path = entry
                .head_to_index()
                .filter(|d| d.status() == Delta::Renamed)
                .or_else(|| {
                    entry
                        .index_to_workdir()
                        .filter(|d| d.status() == Delta::Renamed)
                })
                .and_then(|d| d.old_file().path().map(Path::to_path_buf));
        }
        if !matches!(
            change.status,
            ChangeStatus::Renamed | ChangeStatus::Conflicted
        ) {
            change.status = if flags.intersects(Status::INDEX_DELETED | Status::WT_DELETED) {
                ChangeStatus::Deleted
            } else if flags.intersects(Status::INDEX_NEW | Status::WT_NEW) {
                ChangeStatus::New
            } else {
                ChangeStatus::Modified
            };
        }
        change.is_staged = flags.intersects(
            Status::INDEX_NEW
                | Status::INDEX_MODIFIED
                | Status::INDEX_DELETED
                | Status::INDEX_RENAMED
                | Status::INDEX_TYPECHANGE,
        );
        change.has_unstaged = flags.intersects(
            Status::WT_NEW
                | Status::WT_MODIFIED
                | Status::WT_DELETED
                | Status::WT_RENAMED
                | Status::WT_TYPECHANGE
                | Status::WT_UNREADABLE
                | Status::CONFLICTED,
        );
        changes.push(change);
    }
    changes.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(changes)
}
fn change_from_delta(delta: &git2::DiffDelta<'_>) -> Option<FileChange> {
    let path = delta
        .new_file()
        .path()
        .or_else(|| delta.old_file().path())?
        .to_path_buf();
    let status = match delta.status() {
        Delta::Added | Delta::Untracked => ChangeStatus::New,
        Delta::Deleted => ChangeStatus::Deleted,
        Delta::Renamed => ChangeStatus::Renamed,
        Delta::Conflicted => ChangeStatus::Conflicted,
        _ => ChangeStatus::Modified,
    };
    let old_path = (status == ChangeStatus::Renamed)
        .then(|| delta.old_file().path().map(Path::to_path_buf))
        .flatten();
    Some(FileChange {
        path,
        old_path,
        status,
        is_staged: false,
        has_unstaged: false,
    })
}
fn validate_path(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty()
        || path.components().any(|c| {
            !matches!(c, Component::Normal(_)) || c.as_os_str().eq_ignore_ascii_case(".git")
        })
    {
        return Err(invalid(
            "Expected a repository-relative file path outside .git",
        ));
    }
    Ok(())
}
pub fn safe_disk_path(repo: &Repository, path: &Path) -> Result<PathBuf> {
    validate_path(path)?;
    let root = workdir(repo)?.canonicalize()?;
    let full = root.join(path);
    let mut ancestor = full
        .parent()
        .ok_or_else(|| invalid("Invalid file parent"))?;
    loop {
        match fs::symlink_metadata(ancestor) {
            Ok(_) => break,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                ancestor = ancestor
                    .parent()
                    .ok_or_else(|| invalid("No existing parent"))?;
            }
            Err(error) => return Err(error.into()),
        }
    }
    if !ancestor.canonicalize()?.starts_with(&root) {
        return Err(invalid("Symlink directory escapes the repository"));
    }
    if fs::symlink_metadata(&full).is_ok_and(|metadata| metadata.is_dir()) {
        return Err(invalid(
            "Directory and submodule changes require a dedicated Git client",
        ));
    }
    Ok(full)
}
pub fn stage(repo: &Repository, paths: &[PathBuf], staged: bool) -> Result<()> {
    let snapshot = status(repo)?;
    let mut targets = Vec::new();
    for path in paths {
        validate_path(path)?;
        let change = snapshot
            .changes
            .iter()
            .find(|c| &c.path == path)
            .ok_or_else(|| invalid("File is no longer changed; refresh status"))?;
        targets.push(path.clone());
        if let Some(old) = &change.old_path {
            validate_path(old)?;
            targets.push(old.clone());
        }
    }
    if !staged {
        return unstage_paths(repo, &targets);
    }
    {
        let mut index = repo.index()?;
        for path in targets {
            if !paths.contains(&path) {
                // A rename source is removed from the index. A resurrected source
                // in the worktree is a separate change and must not be staged here.
                match index.remove_path(&path) {
                    Ok(()) => {}
                    Err(error) if error.code() == ErrorCode::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
                continue;
            }
            let full = workdir(repo)?.join(&path);
            match fs::symlink_metadata(full) {
                Ok(_) => {
                    safe_disk_path(repo, &path)?;
                    index.add_path(&path)?;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    match index.remove_path(&path) {
                        Ok(()) => {}
                        Err(error) if error.code() == ErrorCode::NotFound => {}
                        Err(error) => return Err(error.into()),
                    }
                }
                Err(error) => return Err(error.into()),
            }
        }
        index.write()?;
    }
    Ok(())
}
fn unstage_paths(repo: &Repository, paths: &[PathBuf]) -> Result<()> {
    // reset_default accepts wildcard pathspecs. Restore literal index entries instead.
    let mut index = repo.index()?;
    let mut baseline = git2::Index::new()?;
    if let Some(head) = head_commit(repo)? {
        baseline.read_tree(&head.tree()?)?;
    }
    for path in paths {
        validate_path(path)?;
        match index.conflict_remove(path) {
            Ok(()) => {}
            Err(error) if error.code() == ErrorCode::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        if let Some(entry) = baseline.get_path(path, 0) {
            index.add(&entry)?;
        } else {
            match index.remove_path(path) {
                Ok(()) => {}
                Err(error) if error.code() == ErrorCode::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
    }
    index.write()?;
    Ok(())
}
fn commit(repo: &Repository, summary: &str, body: &str) -> Result<CommitInfo> {
    let summary = summary.trim();
    if summary.is_empty() || summary.contains(['\n', '\r', '\0']) {
        return Err(invalid("Summary must be one nonempty line"));
    }
    if repo.state() != git2::RepositoryState::Clean {
        return Err(invalid("Finish the current merge/rebase before committing"));
    }
    let mut index = repo.index()?;
    if index.has_conflicts() {
        return Err(invalid("Resolve index conflicts before committing"));
    }
    let parent = head_commit(repo)?;
    let parent_tree = parent.as_ref().map(git2::Commit::tree).transpose()?;
    let diff = repo.diff_tree_to_index(parent_tree.as_ref(), Some(&index), None)?;
    if diff.deltas().len() == 0 {
        return Err(invalid("No staged changes to commit"));
    }
    let tree = repo.find_tree(index.write_tree()?)?;
    let author = repo.signature()?;
    let message = if body.trim().is_empty() {
        summary.to_owned()
    } else {
        format!("{summary}\n\n{}", body.trim())
    };
    let parents: Vec<_> = parent.iter().collect();
    let id = repo.commit(Some("HEAD"), &author, &author, &message, &tree, &parents)?;
    Ok(commit_info(&repo.find_commit(id)?))
}
pub fn commit_info(commit: &git2::Commit<'_>) -> CommitInfo {
    let signature = commit.author();
    let timestamp = commit.time().seconds();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
    let age = now.saturating_sub(timestamp).max(0);
    let (count, unit) = if age < 60 {
        (age, "second")
    } else if age < 3600 {
        (age / 60, "minute")
    } else if age < 86400 {
        (age / 3600, "hour")
    } else {
        (age / 86400, "day")
    };
    CommitInfo {
        sha: commit.id().to_string(),
        summary: String::from_utf8_lossy(commit.summary_bytes().unwrap_or(b"(no summary)"))
            .into_owned(),
        body: String::from_utf8_lossy(commit.body_bytes().unwrap_or_default())
            .trim()
            .to_owned(),
        author: format!(
            "{} <{}>",
            String::from_utf8_lossy(signature.name_bytes()),
            String::from_utf8_lossy(signature.email_bytes())
        ),
        timestamp,
        relative_time: format!("{count} {unit}{} ago", if count == 1 { "" } else { "s" }),
    }
}
fn history(repo: &Repository, limit: usize) -> Result<Vec<CommitInfo>> {
    if head_commit(repo)?.is_none() {
        return Ok(Vec::new());
    }
    let mut walk = repo.revwalk()?;
    walk.set_sorting(git2::Sort::TOPOLOGICAL | git2::Sort::TIME)?;
    walk.push_head()?;
    walk.take(limit.min(10_000))
        .map(|id| Ok(commit_info(&repo.find_commit(id?)?)))
        .collect()
}
fn file_diff(repo: &Repository, target: &DiffTarget) -> Result<FileDiff> {
    let mut options = DiffOptions::new();
    options
        .include_untracked(true)
        .recurse_untracked_dirs(true)
        .show_untracked_content(true)
        .context_lines(3)
        .max_size(2 * 1024 * 1024);
    let (path, mut diff) = match target {
        DiffTarget::Working { path, staged } => {
            validate_path(path)?;
            let head = head_commit(repo)?;
            let tree = head.as_ref().map(git2::Commit::tree).transpose()?;
            let diff = if *staged {
                repo.diff_tree_to_index(tree.as_ref(), None, Some(&mut options))?
            } else if repo.index()?.has_conflicts() {
                repo.diff_tree_to_workdir(tree.as_ref(), Some(&mut options))?
            } else {
                repo.diff_tree_to_workdir_with_index(tree.as_ref(), Some(&mut options))?
            };
            (path, diff)
        }
        DiffTarget::Commit { sha, path } => {
            validate_path(path)?;
            let commit = repo.find_commit(git2::Oid::from_str(sha)?)?;
            let tree = commit.tree()?;
            let parent = parent_tree(&commit)?;
            (
                path,
                repo.diff_tree_to_tree(parent.as_ref(), Some(&tree), Some(&mut options))?,
            )
        }
    };
    diff.find_similar(None)?;
    parse_diff(&diff, path)
}
fn parse_diff(diff: &Diff<'_>, path: &Path) -> Result<FileDiff> {
    let mut result = FileDiff {
        path: path.to_path_buf(),
        hunks: Vec::new(),
        added: 0,
        deleted: 0,
        binary: false,
        truncated: false,
    };
    let mut bytes = 0;
    let mut line_count = 0;
    for (index, delta) in diff.deltas().enumerate() {
        if delta.new_file().path() != Some(path) && delta.old_file().path() != Some(path) {
            continue;
        }
        let delta_patch = git2::Patch::from_diff(diff, index)?;
        result.binary |= delta.flags().contains(git2::DiffFlags::BINARY);
        let Some(delta_patch) = delta_patch else {
            continue;
        };
        for h in 0..delta_patch.num_hunks() {
            let (hunk, count) = delta_patch.hunk(h)?;
            let mut lines = Vec::new();
            for l in 0..count {
                let line = delta_patch.line_in_hunk(h, l)?;
                let kind = match line.origin() {
                    '+' => DiffLineKind::Addition,
                    '-' => DiffLineKind::Deletion,
                    ' ' => DiffLineKind::Context,
                    _ => continue,
                };
                bytes += line.content().len();
                if bytes > 2 * 1024 * 1024 || line_count >= 50_000 {
                    result.truncated = true;
                    break;
                }
                result.added += usize::from(kind == DiffLineKind::Addition);
                result.deleted += usize::from(kind == DiffLineKind::Deletion);
                line_count += 1;
                let text = String::from_utf8_lossy(line.content());
                let mut chars = text.chars();
                let mut displayed: String = chars.by_ref().take(4096).collect();
                if chars.next().is_some() {
                    displayed.push_str(" … [line truncated]");
                    result.truncated = true;
                }
                lines.push(DiffLine {
                    kind,
                    old_line: line.old_lineno(),
                    new_line: line.new_lineno(),
                    text: displayed,
                });
            }
            result.hunks.push(DiffHunk {
                header: String::from_utf8_lossy(hunk.header()).trim_end().to_owned(),
                old_start: hunk.old_start(),
                old_len: hunk.old_lines(),
                new_start: hunk.new_start(),
                new_len: hunk.new_lines(),
                lines,
            });
            if result.truncated {
                return Ok(result);
            }
        }
    }
    Ok(result)
}
pub fn require_clean(repo: &Repository) -> Result<()> {
    if repo.state() != git2::RepositoryState::Clean || !status(repo)?.changes.is_empty() {
        return Err(invalid(
            "Commit or discard working and staged changes before switching or pulling",
        ));
    }
    Ok(())
}
pub fn checkout(repo: &Repository, name: &str, create: bool) -> Result<()> {
    require_clean(repo)?;
    if !git2::Reference::is_valid_name(&format!("refs/heads/{name}")) {
        return Err(invalid("Invalid branch name"));
    }
    let branch = if create {
        repo.branch(
            name,
            &head_commit(repo)?.ok_or_else(|| invalid("Create an initial commit first"))?,
            false,
        )?
    } else {
        repo.find_branch(name, BranchType::Local)?
    };
    let reference = branch.get();
    let target = reference.peel_to_commit()?;
    repo.checkout_tree(
        target.as_object(),
        Some(
            git2::build::CheckoutBuilder::new()
                .safe()
                .overwrite_ignored(false),
        ),
    )?;
    repo.set_head(reference.name()?)?;
    Ok(())
}
fn callbacks() -> std::result::Result<git2::RemoteCallbacks<'static>, git2::Error> {
    let config = git2::Config::open_default()?;
    let mut callbacks = git2::RemoteCallbacks::new();
    callbacks.credentials(move |url, username, allowed| {
        if allowed.contains(git2::CredentialType::SSH_KEY) {
            return git2::Cred::ssh_key_from_agent(username.unwrap_or("git"));
        }
        if allowed.contains(git2::CredentialType::USER_PASS_PLAINTEXT) {
            return git2::Cred::credential_helper(&config, url, username);
        }
        if allowed.contains(git2::CredentialType::USERNAME) {
            return git2::Cred::username(username.unwrap_or("git"));
        }
        git2::Cred::default()
    });
    callbacks.push_update_reference(|_, status| {
        status.map_or(Ok(()), |message| Err(git2::Error::from_str(message)))
    });
    Ok(callbacks)
}
fn fetch(repo: &Repository) -> Result<()> {
    let mut remote = repo.find_remote("origin")?;
    let mut options = git2::FetchOptions::new();
    options.remote_callbacks(callbacks()?);
    remote.fetch(&[] as &[&str], Some(&mut options), None)?;
    Ok(())
}
fn push(repo: &Repository) -> Result<()> {
    let head = repo.head()?;
    if !head.is_branch() {
        return Err(invalid("Cannot push detached HEAD"));
    }
    let name = head.shorthand()?;
    let mut local = repo.find_branch(name, BranchType::Local)?;
    let upstream = local.upstream().ok();
    let remote_name = upstream.as_ref().and_then(|b| b.name().ok().flatten());
    if remote_name.is_some_and(|s| !s.starts_with("origin/")) {
        return Err(invalid("This branch tracks a remote other than origin"));
    }
    let destination = remote_name
        .and_then(|s| s.strip_prefix("origin/"))
        .unwrap_or(name);
    let spec = format!("refs/heads/{name}:refs/heads/{destination}");
    let mut options = git2::PushOptions::new();
    options.remote_callbacks(callbacks()?);
    repo.find_remote("origin")?
        .push(&[&spec], Some(&mut options))?;
    let establish = upstream.is_none();
    drop(upstream);
    if establish {
        local.set_upstream(Some(&format!("origin/{name}")))?;
    }
    Ok(())
}
fn pull(repo: &Repository) -> Result<()> {
    require_clean(repo)?;
    fetch(repo)?;
    let mut head = repo.head()?;
    let name = head.shorthand()?;
    if !head.is_branch() {
        return Err(invalid("Cannot pull detached HEAD"));
    }
    let branch = repo.find_branch(name, BranchType::Local)?;
    let upstream = branch.upstream()?;
    if !upstream.name()?.is_some_and(|n| n.starts_with("origin/")) {
        return Err(invalid("Pull supports origin upstreams only"));
    }
    let target = upstream.get().peel_to_commit()?;
    let current = head.peel_to_commit()?;
    let (ahead, behind) = repo.graph_ahead_behind(current.id(), target.id())?;
    if behind == 0 {
        return Ok(());
    }
    if ahead > 0 {
        return Err(invalid(
            "Branches diverged; fast-forward pull is unavailable",
        ));
    }
    repo.checkout_tree(
        target.as_object(),
        Some(
            git2::build::CheckoutBuilder::new()
                .safe()
                .overwrite_ignored(false),
        ),
    )?;
    head.set_target(target.id(), "Nozhik: fast-forward pull")?;
    Ok(())
}
fn discard(repo: &Repository, path: &Path) -> Result<()> {
    if repo.state() != git2::RepositoryState::Clean {
        return Err(invalid("Discard is unavailable during merge/rebase"));
    }
    let snapshot = status(repo)?;
    let change = snapshot
        .changes
        .iter()
        .find(|c| c.path == path)
        .ok_or_else(|| invalid("File is no longer changed"))?;
    if change.status == ChangeStatus::Conflicted {
        return Err(invalid("Resolve conflicts with a dedicated merge tool"));
    }
    let mut paths = vec![path.to_path_buf()];
    if let Some(old) = &change.old_path {
        if fs::symlink_metadata(workdir(repo)?.join(old)).is_ok() {
            return Err(invalid(
                "Original rename path already exists; move it before discarding the rename",
            ));
        }
        paths.push(old.clone());
    }
    for path in &paths {
        safe_disk_path(repo, path)?;
    }
    let head = head_commit(repo)?;
    let tree = head.as_ref().map(git2::Commit::tree).transpose()?;
    unstage_paths(repo, &paths)?;
    for path in paths {
        if tree
            .as_ref()
            .is_some_and(|tree| tree.get_path(&path).is_ok())
        {
            let object = head.as_ref().ok_or_else(|| invalid("Missing HEAD"))?;
            repo.checkout_tree(
                object.as_object(),
                Some(
                    git2::build::CheckoutBuilder::new()
                        .force()
                        .disable_pathspec_match(true)
                        .path(&path),
                ),
            )?;
        } else {
            let full = safe_disk_path(repo, &path)?;
            match fs::remove_file(full) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
    }
    Ok(())
}
fn ignore_file(repo: &Repository, path: &Path) -> Result<()> {
    validate_path(path)?;
    let text = path
        .to_str()
        .ok_or_else(|| invalid("Non-UTF-8 paths cannot be written to .gitignore"))?
        .replace(std::path::MAIN_SEPARATOR, "/");
    if text.contains(['\n', '\r', '\0']) {
        return Err(invalid("Cannot ignore a path containing newlines"));
    }
    let mut pattern = String::from("/");
    for ch in text.chars() {
        if matches!(ch, '\\' | ' ' | '#' | '!' | '*' | '?' | '[' | ']') {
            pattern.push('\\');
        }
        pattern.push(ch);
    }
    append_ignore(repo, &pattern)
}
fn append_ignore(repo: &Repository, pattern: &str) -> Result<()> {
    let destination = workdir(repo)?.join(".gitignore");
    match fs::symlink_metadata(&destination) {
        Ok(metadata) if !metadata.file_type().is_file() => {
            return Err(invalid(".gitignore must be a regular file"));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let mut file = fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(destination)?;
    writeln!(file, "\n{pattern}")?;
    file.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests;

fn open_external(path: &Path, reveal: bool) -> Result<()> {
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut c = std::process::Command::new("open");
        if reveal {
            c.arg("-R");
        }
        c.arg("--").arg(path);
        c
    };
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut c = std::process::Command::new("explorer.exe");
        if reveal {
            c.arg(format!("/select,{}", path.display()));
        } else {
            c.arg(path);
        }
        c
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut command = {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(if reveal {
            path.parent().unwrap_or(path)
        } else {
            path
        });
        c
    };
    let result = command.status()?;
    if !result.success() {
        return Err(invalid("The external application could not open the path"));
    }
    Ok(())
}
