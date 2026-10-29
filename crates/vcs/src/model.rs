//! Owned Git snapshots; no libgit2 lifetimes or handles cross the worker boundary.
use std::path::PathBuf;

/// Display category for a Git delta.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeStatus {
    /// Existing file edited, including mode/type changes.
    Modified,
    /// Added to index or untracked.
    New,
    /// Removed file.
    Deleted,
    /// Renamed file with an original path.
    Renamed,
    /// Unresolved merge conflict.
    Conflicted,
}
/// One repository-relative file, which can have both staged and unstaged edits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    /// Repository-relative native path.
    pub path: PathBuf,
    /// Original path for renames.
    pub old_path: Option<PathBuf>,
    /// Display category.
    pub status: ChangeStatus,
    /// The index differs from HEAD for this file.
    pub is_staged: bool,
    /// The worktree differs from the index for this file.
    pub has_unstaged: bool,
}
/// Authoritative repository snapshot.
#[derive(Debug, Clone)]
pub struct GitRepoStatus {
    /// Canonical worktree root (may be above the IDE's project root).
    pub root: PathBuf,
    /// HEAD commit ID; absent before the first commit.
    pub head_sha: Option<String>,
    /// Current branch, or detached HEAD description.
    pub branch: String,
    /// Whether HEAD is detached.
    pub detached: bool,
    /// Whether this local branch tracks an upstream.
    pub has_upstream: bool,
    /// Upstream shorthand, if configured.
    pub upstream: Option<String>,
    /// Commits ahead of the upstream.
    pub ahead: usize,
    /// Commits behind the upstream.
    pub behind: usize,
    /// Sorted changed files.
    pub changes: Vec<FileChange>,
    /// Local and remote branch names for the switcher.
    pub branches: Vec<String>,
    /// Branch commit ages and default-branch grouping.
    pub branch_info: Vec<BranchInfo>,
    /// Unmerged index entries and on-disk resolution state.
    pub conflicts: Vec<MergeConflict>,
    /// Origin remote exists.
    pub has_origin: bool,
    /// Human-readable repository operation state, e.g. merge/rebase.
    pub operation: Option<String>,
}
/// Content type of a unified diff line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLineKind {
    /// Unchanged context.
    Context,
    /// Added line.
    Addition,
    /// Removed line.
    Deletion,
}
/// A line in a textual patch. Numbers are one-based.
#[derive(Debug, Clone)]
pub struct DiffLine {
    /// Rendering category.
    pub kind: DiffLineKind,
    /// Old line number, absent on additions.
    pub old_line: Option<u32>,
    /// New line number, absent on deletions.
    pub new_line: Option<u32>,
    /// UTF-8 display text; non-UTF-8 patch bytes are displayed lossily.
    pub text: String,
}
/// One contiguous patch hunk.
#[derive(Debug, Clone)]
pub struct DiffHunk {
    /// Unified header, including `@@` positions.
    pub header: String,
    /// Old start line.
    pub old_start: u32,
    /// Old line count.
    pub old_len: u32,
    /// New start line.
    pub new_start: u32,
    /// New line count.
    pub new_len: u32,
    /// Context, additions, and deletions.
    pub lines: Vec<DiffLine>,
}
/// A bounded display patch, generated off the UI thread.
#[derive(Debug, Clone)]
pub struct FileDiff {
    /// Selected repository-relative file.
    pub path: PathBuf,
    /// Parsed hunks.
    pub hunks: Vec<DiffHunk>,
    /// Addition count in displayed hunks.
    pub added: usize,
    /// Deletion count in displayed hunks.
    pub deleted: usize,
    /// Binary or oversized file, for which textual patching is unavailable.
    pub binary: bool,
    /// Display limit reached.
    pub truncated: bool,
}
/// History entry and commit details.
#[derive(Debug, Clone)]
pub struct CommitInfo {
    /// Full hexadecimal object ID.
    pub sha: String,
    /// First message line.
    pub summary: String,
    /// Remaining message body.
    pub body: String,
    /// Author name and email.
    pub author: String,
    /// Commit timestamp, seconds since Unix epoch.
    pub timestamp: i64,
    /// Relative time computed on the worker, e.g. `2 minutes ago`.
    pub relative_time: String,
}
/// Identifies a patch without retaining any Git object.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DiffTarget {
    /// Current file: HEAD-to-worktree combined patch or HEAD-to-index staged patch.
    Working {
        /// Repository-relative file.
        path: PathBuf,
        /// Only show staged contents that a commit will use.
        staged: bool,
    },
    /// Historical file: first parent to selected commit (empty tree for root commits).
    Commit {
        /// Full commit ID.
        sha: String,
        /// Repository-relative file.
        path: PathBuf,
    },
}

/// Branch presentation computed on the repository worker.
#[derive(Debug, Clone)]
pub struct BranchInfo {
    /// Local name or remote/name.
    pub name: String,
    /// Age of the tip commit.
    pub relative_time: String,
    /// Whether this is a remote-tracking branch.
    pub is_remote: bool,
    /// Remote default or conventional main/master branch.
    pub is_default: bool,
}
/// A conflicted path; removing markers does not mutate the index until Continue.
#[derive(Debug, Clone)]
pub struct MergeConflict {
    /// Repository-relative path.
    pub path: PathBuf,
    /// Remaining conflict regions (at least one for binary/missing files).
    pub remaining: usize,
}
