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
#[derive(Debug, Clone, PartialEq, Eq)]
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
#[derive(Debug, Clone, PartialEq, Eq)]
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    /// Selected repository-relative file.
    pub path: PathBuf,
    /// Parsed hunks.
    pub hunks: Vec<DiffHunk>,
    /// Complete context from the same Git comparison, absent if display limits prevent it.
    pub full_hunks: Option<Vec<DiffHunk>>,
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
    /// Number of parents (zero for a root commit).
    pub parent_count: usize,
    /// Tags pointing to this commit, populated in history snapshots.
    pub tags: Vec<String>,
}
/// Unique changed path across the selected commits, including intermediate changes.
#[derive(Debug, Clone)]
pub struct HistoryFile {
    /// Newest selected delta for this path; index flags are always false.
    pub change: FileChange,
    /// Contributing selected commits, newest first (one ID per commit).
    pub commits: Vec<String>,
    /// Current worktree contains a regular file at a safely resolved path.
    pub worktree_exists: bool,
    /// Historical GitHub file URL; deletions link to their first parent.
    pub github_url: Option<String>,
}
impl HistoryFile {
    /// Show this file's newest selected commit; older contributions remain metadata.
    #[must_use]
    pub fn diff_target(&self) -> Option<DiffTarget> {
        self.commits.first().map(|sha| DiffTarget::Commit {
            sha: sha.clone(),
            path: self.change.path.clone(),
        })
    }
}
/// Owned details for exactly one requested commit selection.
#[derive(Debug, Clone)]
pub struct HistorySelection {
    /// Selected commit metadata, newest first.
    pub commits: Vec<CommitInfo>,
    /// Sorted union of paths, including canceled or later renamed/deleted changes.
    pub files: Vec<HistoryFile>,
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

/// Confirmed operation on history. Commit lists use newest-first display order.
#[derive(Debug, Clone)]
pub struct HistoryRequest {
    /// HEAD when the dialog opened; stale dialogs cannot mutate a newer checkout.
    pub expected_head: String,
    /// Checked-out branch name (or detached description) when the dialog opened.
    pub expected_branch: String,
    /// Selected full commit IDs, newest first (reorder uses the desired full order).
    pub commits: Vec<String>,
    /// Concrete action and its validated user input.
    pub action: HistoryAction,
}
/// History mutations and read-only metadata commands.
#[derive(Debug, Clone)]
pub enum HistoryAction {
    /// Replace HEAD's message and include its current staged changes.
    Amend(String),
    /// Move HEAD; soft retains index/worktree, mixed resets index, hard resets both.
    Reset(ResetMode),
    /// Safely enter detached HEAD.
    Checkout,
    /// Apply the inverse patch and commit it.
    Revert,
    /// Create a branch at a historical commit without switching.
    Branch(String),
    /// Create a lightweight tag.
    Tag(String),
    /// Replay selected commits oldest first onto a local destination, optionally new.
    CherryPick {
        /// Local destination branch name.
        branch: String,
        /// Create the destination from current HEAD.
        create: bool,
    },
    /// Combine a contiguous selection and replay its descendants.
    Squash(String),
    /// Replay a complete linear suffix in the supplied newest-first order.
    Reorder,
    /// Copy tags pointing at a commit.
    CopyTags,
    /// Open the commit on GitHub using origin.
    View,
}
/// Which local state a confirmed reset replaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetMode {
    /// Move the ref only.
    Soft,
    /// Also reset the index.
    Mixed,
    /// Also replace tracked working files.
    Hard,
}
