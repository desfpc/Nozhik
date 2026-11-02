use super::*;
use std::fmt::Write;
fn repository() -> std::result::Result<(tempfile::TempDir, Repository), Box<dyn std::error::Error>>
{
    let dir = tempfile::tempdir()?;
    let repo = Repository::init(dir.path())?;
    let mut config = repo.config()?;
    config.set_str("user.name", "Nozhik Test")?;
    config.set_str("user.email", "test@example.invalid")?;
    config.set_str("commit.gpgsign", "false")?;
    drop(config);
    Ok((dir, repo))
}
#[test]
fn initial_status_diff_and_index_only_commit() -> std::result::Result<(), Box<dyn std::error::Error>>
{
    let (dir, repo) = repository()?;
    fs::write(dir.path().join("main.rs"), "first\n")?;
    let snapshot = status(&repo)?;
    assert_eq!(snapshot.changes.len(), 1);
    assert_eq!(snapshot.changes[0].status, ChangeStatus::New);
    assert!(!snapshot.changes[0].is_staged);
    let path = PathBuf::from("main.rs");
    let diff = file_diff(
        &repo,
        &DiffTarget::Working {
            path: path.clone(),
            staged: false,
        },
    )?;
    assert_eq!(diff.added, 1);
    assert_eq!(diff.hunks[0].lines[0].new_line, Some(1));
    stage(&repo, std::slice::from_ref(&path), true)?;
    fs::write(dir.path().join(&path), "first\nsecond\n")?;
    let change = &status(&repo)?.changes[0];
    assert!(change.is_staged && change.has_unstaged);
    let created = commit(&repo, "Initial commit", "A body")?;
    assert_eq!(created.summary, "Initial commit");
    assert_eq!(created.body, "A body");
    let head = repo.head()?.peel_to_commit()?;
    let tree = head.tree()?;
    assert_eq!(
        repo.find_blob(tree.get_path(&path)?.id())?.content(),
        b"first\n"
    );
    assert_eq!(status(&repo)?.changes.len(), 1);
    let diff = file_diff(
        &repo,
        &DiffTarget::Working {
            path: path.clone(),
            staged: false,
        },
    )?;
    assert_eq!(diff.added, 1);
    assert_eq!(diff.deleted, 0);
    stage(&repo, std::slice::from_ref(&path), true)?;
    stage(&repo, std::slice::from_ref(&path), false)?;
    assert!(!status(&repo)?.changes[0].is_staged);
    assert!(commit(&repo, "Empty", "").is_err());
    assert!(commit(&repo, "", "").is_err());
    let historic = file_diff(
        &repo,
        &DiffTarget::Commit {
            sha: created.sha,
            path,
        },
    )?;
    assert_eq!(historic.added, 1);
    assert_eq!(history(&repo, 10)?.len(), 1);
    Ok(())
}
#[test]
fn deletion_rename_binary_and_discard_are_scoped()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let (dir, repo) = repository()?;
    for name in ["a.txt", "keep.txt"] {
        fs::write(dir.path().join(name), "original\n")?;
    }
    stage(&repo, &["a.txt".into(), "keep.txt".into()], true)?;
    commit(&repo, "Base", "")?;
    fs::rename(dir.path().join("a.txt"), dir.path().join("renamed.txt"))?;
    let paths: Vec<_> = status(&repo)?.changes.into_iter().map(|c| c.path).collect();
    stage(&repo, &paths, true)?;
    let snapshot = status(&repo)?;
    assert!(
        snapshot
            .changes
            .iter()
            .any(|c| c.status == ChangeStatus::Renamed)
    );
    discard(&repo, Path::new("renamed.txt"))?;
    assert_eq!(fs::read_to_string(dir.path().join("a.txt"))?, "original\n");
    assert!(!dir.path().join("renamed.txt").exists());
    fs::write(dir.path().join("keep.txt"), "keep this edit\n")?;
    fs::remove_file(dir.path().join("a.txt"))?;
    let diff = file_diff(
        &repo,
        &DiffTarget::Working {
            path: "a.txt".into(),
            staged: false,
        },
    )?;
    assert_eq!(diff.deleted, 1);
    discard(&repo, Path::new("a.txt"))?;
    assert_eq!(
        fs::read_to_string(dir.path().join("keep.txt"))?,
        "keep this edit\n"
    );
    fs::write(dir.path().join("binary.dat"), [0, 1, 2])?;
    assert!(
        file_diff(
            &repo,
            &DiffTarget::Working {
                path: "binary.dat".into(),
                staged: false
            }
        )?
        .binary
    );
    assert!(discard(&repo, Path::new("../outside")).is_err());
    assert!(validate_path(Path::new(".git/config")).is_err());
    Ok(())
}
#[test]
fn branches_ignore_patterns_and_non_repository()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let (dir, repo) = repository()?;
    fs::write(dir.path().join("base"), "base")?;
    stage(&repo, &["base".into()], true)?;
    commit(&repo, "Base", "")?;
    checkout(&repo, "feature/test", true)?;
    assert_eq!(status(&repo)?.branch, "feature/test");
    fs::write(dir.path().join("a[1].log"), "log")?;
    ignore_file(&repo, Path::new("a[1].log"))?;
    assert!(repo.status_should_ignore(Path::new("a[1].log"))?);
    assert!(!repo.status_should_ignore(Path::new("a1.log"))?);
    assert!(checkout(&repo, "another", true).is_err());
    let other = tempfile::tempdir()?;
    let mut worker = GitWorker::default();
    assert!(matches!(
        worker.execute(Task::Open(other.path().into()))?,
        Outcome::Status(None)
    ));
    assert!(worker.execute(Task::Refresh).is_err());
    Ok(())
}

#[test]
fn local_origin_fetch_push_ahead_behind_and_fast_forward()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let (dir, repo) = repository()?;
    let remote_dir = tempfile::tempdir()?;
    let remote = Repository::init_bare(remote_dir.path())?;
    repo.remote(
        "origin",
        remote_dir.path().to_str().ok_or("non-UTF-8 temp path")?,
    )?;
    fs::write(dir.path().join("file.txt"), "base\n")?;
    stage(&repo, &["file.txt".into()], true)?;
    commit(&repo, "Base", "")?;
    let branch = status(&repo)?.branch;
    push(&repo, &crate::clone::Authentication::Automatic)?;
    assert!(status(&repo)?.has_upstream);
    assert_eq!(status(&repo)?.ahead, 0);
    remote.set_head(&format!("refs/heads/{branch}"))?;
    let peer_dir = tempfile::tempdir()?;
    let peer = Repository::clone(
        remote_dir.path().to_str().ok_or("non-UTF-8 temp path")?,
        peer_dir.path(),
    )?;
    let mut config = peer.config()?;
    config.set_str("user.name", "Peer")?;
    config.set_str("user.email", "peer@example.invalid")?;
    drop(config);
    fs::write(peer_dir.path().join("file.txt"), "base\nremote edit\n")?;
    stage(&peer, &["file.txt".into()], true)?;
    commit(&peer, "Remote commit", "")?;
    push(&peer, &crate::clone::Authentication::Automatic)?;
    fetch(&repo, &crate::clone::Authentication::Automatic)?;
    assert_eq!(status(&repo)?.behind, 1);
    pull(&repo, &crate::clone::Authentication::Automatic)?;
    assert_eq!(
        fs::read_to_string(dir.path().join("file.txt"))?,
        "base\nremote edit\n"
    );
    fs::write(dir.path().join("local.txt"), "local")?;
    stage(&repo, &["local.txt".into()], true)?;
    commit(&repo, "Local commit", "")?;
    assert_eq!(status(&repo)?.ahead, 1);
    fs::write(peer_dir.path().join("peer.txt"), "peer")?;
    stage(&peer, &["peer.txt".into()], true)?;
    commit(&peer, "Diverging commit", "")?;
    push(&peer, &crate::clone::Authentication::Automatic)?;
    assert!(pull(&repo, &crate::clone::Authentication::Automatic).is_err());
    assert!(push(&repo, &crate::clone::Authentication::Automatic).is_err());
    assert_eq!(fs::read_to_string(dir.path().join("local.txt"))?, "local");
    let mut worker = GitWorker::default();
    worker.execute(Task::Open(dir.path().to_path_buf()))?;
    worker.execute(Task::PullPush)?;
    assert_eq!((status(&repo)?.ahead, status(&repo)?.behind), (0, 0));
    assert_eq!(repo.head()?.peel_to_commit()?.parent_count(), 2);
    assert_eq!(
        remote
            .find_reference(&format!("refs/heads/{branch}"))?
            .target(),
        repo.head()?.target()
    );
    pull(&peer, &crate::clone::Authentication::Automatic)?;
    history_commit(&repo, "file.txt", "local conflict\n")?;
    history_commit(&peer, "file.txt", "remote conflict\n")?;
    push(&peer, &crate::clone::Authentication::Automatic)?;
    let remote_tip = peer.head()?.target();
    let result = worker.execute(Task::PullPush)?;
    assert!(matches!(result, Outcome::Done(message) if message.contains("then push")));
    assert_eq!(repo.state(), git2::RepositoryState::Merge);
    assert_eq!(
        remote
            .find_reference(&format!("refs/heads/{branch}"))?
            .target(),
        remote_tip
    );
    crate::merge::abort(&repo)?;
    Ok(())
}

#[test]
fn literal_paths_and_deleted_parent_directories()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let (dir, repo) = repository()?;
    fs::create_dir(dir.path().join("nested"))?;
    for name in ["a[1].txt", "a1.txt", "nested/file.txt"] {
        fs::write(dir.path().join(name), "base\n")?;
    }
    let paths: Vec<_> = status(&repo)?.changes.into_iter().map(|c| c.path).collect();
    stage(&repo, &paths, true)?;
    commit(&repo, "Base", "")?;
    fs::write(dir.path().join("a[1].txt"), "one\n")?;
    fs::write(dir.path().join("a1.txt"), "two\n")?;
    stage(&repo, &["a[1].txt".into(), "a1.txt".into()], true)?;
    stage(&repo, &["a[1].txt".into()], false)?;
    assert!(
        status(&repo)?
            .changes
            .iter()
            .any(|c| c.path == Path::new("a1.txt") && c.is_staged)
    );
    discard(&repo, Path::new("a[1].txt"))?;
    assert_eq!(fs::read_to_string(dir.path().join("a1.txt"))?, "two\n");
    fs::remove_file(dir.path().join("nested/file.txt"))?;
    fs::remove_dir(dir.path().join("nested"))?;
    discard(&repo, Path::new("nested/file.txt"))?;
    assert_eq!(
        fs::read_to_string(dir.path().join("nested/file.txt"))?,
        "base\n"
    );
    Ok(())
}

#[test]
fn staging_rename_does_not_stage_resurrected_source()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let (dir, repo) = repository()?;
    fs::write(dir.path().join("old.txt"), "original\n")?;
    stage(&repo, &["old.txt".into()], true)?;
    commit(&repo, "Base", "")?;
    fs::rename(dir.path().join("old.txt"), dir.path().join("new.txt"))?;
    let paths: Vec<_> = status(&repo)?.changes.into_iter().map(|c| c.path).collect();
    stage(&repo, &paths, true)?;
    fs::write(dir.path().join("old.txt"), "independent new contents\n")?;
    stage(&repo, &["new.txt".into()], true)?;
    assert!(repo.index()?.get_path(Path::new("old.txt"), 0).is_none());
    assert!(discard(&repo, Path::new("new.txt")).is_err());
    assert_eq!(
        fs::read_to_string(dir.path().join("old.txt"))?,
        "independent new contents\n"
    );
    Ok(())
}

fn merge_fixture()
-> std::result::Result<(tempfile::TempDir, Repository, String), Box<dyn std::error::Error>> {
    let (dir, repo) = repository()?;
    fs::write(dir.path().join("file.txt"), "base\n")?;
    stage(&repo, &["file.txt".into()], true)?;
    commit(&repo, "Base", "")?;
    let original = status(&repo)?.branch;
    checkout(&repo, "feature", true)?;
    fs::write(dir.path().join("file.txt"), "theirs\n")?;
    stage(&repo, &["file.txt".into()], true)?;
    commit(&repo, "Theirs", "")?;
    checkout(&repo, &original, false)?;
    fs::write(dir.path().join("file.txt"), "ours\n")?;
    stage(&repo, &["file.txt".into()], true)?;
    commit(&repo, "Ours", "")?;
    Ok((dir, repo, original))
}
#[test]
fn merge_preview_conflict_edit_continue_and_abort()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let (dir, repo, _) = merge_fixture()?;
    let before = repo.head()?.target();
    assert_eq!(crate::merge::preview(&repo, "feature")?, 1);
    assert_eq!(repo.state(), git2::RepositoryState::Clean);
    assert_eq!(fs::read_to_string(dir.path().join("file.txt"))?, "ours\n");
    crate::merge::begin(&repo, "feature")?;
    assert_eq!(repo.state(), git2::RepositoryState::Merge);
    let snapshot = status(&repo)?;
    assert_eq!(snapshot.conflicts.len(), 1);
    assert_eq!(snapshot.conflicts[0].remaining, 1);
    assert!(crate::merge::finish(&repo).is_err());
    let diff = file_diff(
        &repo,
        &DiffTarget::Working {
            path: "file.txt".into(),
            staged: false,
        },
    )?;
    assert!(
        diff.hunks
            .iter()
            .flat_map(|h| &h.lines)
            .any(|l| l.text.contains("<<<<<<<"))
    );
    assert!(crate::merge::switch(&mut Repository::open(dir.path())?, "feature", false).is_err());
    fs::write(dir.path().join("file.txt"), "resolved\n")?;
    assert_eq!(status(&repo)?.conflicts[0].remaining, 0);
    crate::merge::finish(&repo)?;
    assert_eq!(repo.state(), git2::RepositoryState::Clean);
    assert_eq!(repo.head()?.peel_to_commit()?.parent_count(), 2);
    assert_eq!(status(&repo)?.changes.len(), 0);
    assert_eq!(
        fs::read_to_string(dir.path().join("file.txt"))?,
        "resolved\n"
    );
    // Recreate the same merge, then abort without deleting unrelated untracked files.
    let base = repo.find_commit(before.ok_or("missing original HEAD")?)?;
    repo.reset(base.as_object(), git2::ResetType::Hard, None)?;
    crate::merge::begin(&repo, "feature")?;
    fs::write(dir.path().join("keep.tmp"), "keep")?;
    crate::merge::abort(&repo)?;
    assert_eq!(repo.head()?.target(), before);
    assert_eq!(fs::read_to_string(dir.path().join("file.txt"))?, "ours\n");
    assert_eq!(fs::read_to_string(dir.path().join("keep.tmp"))?, "keep");
    assert_eq!(repo.state(), git2::RepositoryState::Clean);
    Ok(())
}
#[test]
fn branch_stash_roundtrip_preserves_index_and_untracked_files()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let (dir, mut repo, original) = merge_fixture()?;
    fs::write(dir.path().join("file.txt"), "staged work\n")?;
    stage(&repo, &["file.txt".into()], true)?;
    fs::write(dir.path().join("file.txt"), "unstaged work\n")?;
    fs::write(dir.path().join("new.txt"), "new work\n")?;
    crate::merge::switch(&mut repo, "feature", true)?;
    assert_eq!(status(&repo)?.branch, "feature");
    assert_eq!(status(&repo)?.changes.len(), 0);
    assert!(!dir.path().join("new.txt").exists());
    crate::merge::switch(&mut repo, &original, false)?;
    assert_eq!(
        fs::read_to_string(dir.path().join("file.txt"))?,
        "unstaged work\n"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("new.txt"))?,
        "new work\n"
    );
    let change = status(&repo)?
        .changes
        .into_iter()
        .find(|c| c.path == Path::new("file.txt"))
        .ok_or("missing work")?;
    assert!(change.is_staged && change.has_unstaged);
    // Bring-changes refuses an overlapping checkout and retains all work.
    assert!(crate::merge::switch(&mut repo, "feature", false).is_err());
    assert_eq!(status(&repo)?.branch, original);
    assert_eq!(
        fs::read_to_string(dir.path().join("file.txt"))?,
        "unstaged work\n"
    );
    Ok(())
}
#[test]
fn nonconflicting_merge_remote_switch_and_bulk_actions()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let (dir, mut repo) = repository()?;
    fs::write(dir.path().join("base.txt"), "base\n")?;
    stage(&repo, &["base.txt".into()], true)?;
    commit(&repo, "Base", "")?;
    let original = status(&repo)?.branch;
    checkout(&repo, "feature", true)?;
    fs::write(dir.path().join("feature.txt"), "feature\n")?;
    stage(&repo, &["feature.txt".into()], true)?;
    commit(&repo, "Feature", "")?;
    let target = repo.head()?.peel_to_commit()?;
    repo.reference(
        "refs/remotes/origin/remote-feature",
        target.id(),
        false,
        "test",
    )?;
    // A fetch refspec supplies remote tracking validation without using a network.
    repo.remote("origin", "https://example.invalid/test.git")?;
    drop(target);
    checkout(&repo, &original, false)?;
    assert_eq!(crate::merge::preview(&repo, "feature")?, 0);
    crate::merge::begin(&repo, "feature")?;
    assert_eq!(repo.state(), git2::RepositoryState::Clean);
    assert_eq!(repo.head()?.peel_to_commit()?.parent_count(), 2);
    assert_eq!(
        fs::read_to_string(dir.path().join("feature.txt"))?,
        "feature\n"
    );
    assert!(
        status(&repo)?
            .branches
            .iter()
            .any(|b| b == "origin/remote-feature")
    );
    crate::merge::switch(&mut repo, "origin/remote-feature", false)?;
    assert_eq!(status(&repo)?.branch, "remote-feature");
    assert_eq!(
        status(&repo)?.upstream.as_deref(),
        Some("origin/remote-feature")
    );
    fs::write(dir.path().join("one.tmp"), "one")?;
    fs::write(dir.path().join("two.tmp"), "two")?;
    let mut worker = GitWorker::default();
    worker.execute(Task::Open(dir.path().into()))?;
    worker.execute(Task::IgnoreExtension("log".into()))?;
    assert!(repo.status_should_ignore(Path::new("nested/a.log"))?);
    worker.execute(Task::DiscardMany(vec!["one.tmp".into(), "two.tmp".into()]))?;
    assert!(!dir.path().join("one.tmp").exists() && !dir.path().join("two.tmp").exists());
    Ok(())
}

#[test]
fn staging_markers_does_not_bypass_merge_resolution_gate()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let (dir, repo, _) = merge_fixture()?;
    crate::merge::begin(&repo, "feature")?;
    stage(&repo, &["file.txt".into()], true)?;
    assert!(!repo.index()?.has_conflicts());
    assert_eq!(status(&repo)?.conflicts[0].remaining, 1);
    assert!(crate::merge::finish(&repo).is_err());
    std::fs::write(dir.path().join("file.txt"), "resolved\n")?;
    assert!(status(&repo)?.conflicts.iter().all(|c| c.remaining == 0));
    crate::merge::finish(&repo)?;
    assert_eq!(repo.head()?.peel_to_commit()?.parent_count(), 2);
    let head = repo.head()?.peel_to_commit()?;
    let entry = head.tree()?.get_path(Path::new("file.txt"))?;
    assert_eq!(repo.find_blob(entry.id())?.content(), b"resolved\n");
    assert_eq!(status(&repo)?.changes.len(), 0);
    Ok(())
}

#[test]
fn full_context_tracks_worktree_index_and_historical_versions()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let (dir, repo) = repository()?;
    let path = PathBuf::from("context.php");
    let mut source = String::new();
    for n in 1..=140 {
        writeln!(source, "// line {n}")?;
    }
    fs::write(dir.path().join(&path), &source)?;
    stage(&repo, std::slice::from_ref(&path), true)?;
    commit(&repo, "Base", "")?;
    let staged = source.replace("// line 70\n", "$value = 'staged';\n");
    fs::write(dir.path().join(&path), &staged)?;
    stage(&repo, std::slice::from_ref(&path), true)?;
    fs::write(dir.path().join(&path), staged.replace("staged", "working"))?;
    for (staged_only, expected) in [(true, "staged"), (false, "working")] {
        let display = file_diff(
            &repo,
            &DiffTarget::Working {
                path: path.clone(),
                staged: staged_only,
            },
        )?;
        assert_eq!((display.added, display.deleted), (1, 1));
        assert_eq!(display.hunks[0].new_start, 67);
        let full = display.full_hunks.ok_or("missing full context")?;
        assert_eq!(full.len(), 1);
        assert_eq!(full[0].new_start, 1);
        assert_eq!(full[0].new_len, 140);
        assert!(full[0].lines.iter().any(|l| l.text.contains(expected)));
        assert_eq!(full[0].lines.last().and_then(|l| l.new_line), Some(140));
    }
    let created = commit(&repo, "Staged", "")?;
    fs::write(dir.path().join(&path), "unrelated on-disk contents\n")?;
    let display = file_diff(
        &repo,
        &DiffTarget::Commit {
            sha: created.sha,
            path,
        },
    )?;
    let full = display.full_hunks.ok_or("missing history context")?;
    assert!(full[0].lines.iter().any(|l| l.text.contains("staged")));
    assert!(!full[0].lines.iter().any(|l| l.text.contains("unrelated")));
    Ok(())
}

fn history_action(
    repo: &Repository,
    ids: &[git2::Oid],
    action: crate::model::HistoryAction,
) -> Result<Outcome> {
    crate::history_actions::execute(
        repo,
        crate::model::HistoryRequest {
            expected_head: repo.head()?.peel_to_commit()?.id().to_string(),
            expected_branch: status(repo)?.branch,
            commits: ids.iter().map(ToString::to_string).collect(),
            action,
        },
    )
}
fn history_commit(repo: &Repository, name: &str, text: &str) -> Result<git2::Oid> {
    fs::write(workdir(repo)?.join(name), text)?;
    stage(repo, &[name.into()], true)?;
    Ok(git2::Oid::from_str(&commit(repo, name, "")?.sha)?)
}
#[test]
fn history_reorder_squash_and_conflict_preflight()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    use crate::model::HistoryAction;
    let (_dir, repo) = repository()?;
    history_commit(&repo, "base", "base\n")?;
    let first = history_commit(&repo, "first", "first\n")?;
    let second = history_commit(&repo, "second", "second\n")?;
    let third = history_commit(&repo, "third", "third\n")?;
    let original_tree = repo.head()?.peel_to_tree()?.id();
    history_action(&repo, &[second, third, first], HistoryAction::Reorder)?;
    let entries = history(&repo, 10)?;
    assert_eq!(
        entries
            .iter()
            .map(|third| third.summary.as_str())
            .collect::<Vec<_>>(),
        vec!["second", "third", "first", "base"]
    );
    assert_eq!(repo.head()?.peel_to_tree()?.id(), original_tree);
    let ids = entries
        .iter()
        .take(2)
        .map(|third| git2::Oid::from_str(&third.sha))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    history_action(
        &repo,
        &ids,
        HistoryAction::Squash("Combined\n\nBody".into()),
    )?;
    assert_eq!(history(&repo, 10)?.len(), 3);
    assert_eq!(history(&repo, 1)?[0].summary, "Combined");
    assert_eq!(repo.head()?.peel_to_tree()?.id(), original_tree);
    let base = repo.head()?.peel_to_commit()?.id();
    let earlier = history_commit(&repo, "base", "next\n")?;
    let later = history_commit(&repo, "base", "last\n")?;
    assert!(history_action(&repo, &[earlier, later], HistoryAction::Reorder).is_err());
    assert_eq!(repo.head()?.target(), Some(later));
    assert_eq!(fs::read_to_string(workdir(&repo)?.join("base"))?, "last\n");
    assert_eq!(status(&repo)?.changes, Vec::<FileChange>::new());
    assert!(
        history_action(
            &repo,
            &[later, base],
            HistoryAction::Squash("Invalid".into())
        )
        .is_err()
    );
    assert_eq!(repo.head()?.target(), Some(later));
    Ok(())
}
#[test]
fn history_amend_reset_revert_branch_tag_and_stale_dialog()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    use crate::model::{HistoryAction, HistoryRequest, ResetMode};
    let (_dir, repo) = repository()?;
    let base = history_commit(&repo, "base", "base\n")?;
    let a = history_commit(&repo, "a", "a\n")?;
    fs::write(workdir(&repo)?.join("staged"), "included\n")?;
    stage(&repo, &["staged".into()], true)?;
    fs::write(workdir(&repo)?.join("a"), "unstaged\n")?;
    history_action(
        &repo,
        &[a],
        HistoryAction::Amend("Amended\n\nDescription".into()),
    )?;
    let amended = repo.head()?.peel_to_commit()?.id();
    assert_ne!(amended, a);
    assert_eq!(repo.find_commit(amended)?.author().name()?, "Nozhik Test");
    assert_eq!(fs::read_to_string(workdir(&repo)?.join("a"))?, "unstaged\n");
    assert!(repo.head()?.peel_to_tree()?.get_name("staged").is_some());
    discard(&repo, std::path::Path::new("a"))?;
    history_action(&repo, &[base], HistoryAction::Branch("from-base".into()))?;
    assert_eq!(
        repo.find_branch("from-base", BranchType::Local)?
            .get()
            .target(),
        Some(base)
    );
    history_action(&repo, &[amended], HistoryAction::Tag("v1".into()))?;
    assert!(
        matches!(history_action(&repo, &[amended], HistoryAction::CopyTags)?, Outcome::Clipboard(text) if text == "v1")
    );
    repo.remote("origin", "git@github.com:owner/project.git")?;
    assert!(
        matches!(history_action(&repo, &[amended], HistoryAction::View)?, Outcome::CommitUrl(url) if url == format!("https://github.com/owner/project/commit/{amended}"))
    );
    let stale = HistoryRequest {
        expected_head: a.to_string(),
        expected_branch: status(&repo)?.branch,
        commits: vec![amended.to_string()],
        action: HistoryAction::Reset(ResetMode::Hard),
    };
    assert!(crate::history_actions::execute(&repo, stale).is_err());
    history_action(&repo, &[amended], HistoryAction::Revert)?;
    assert_eq!(
        repo.head()?.peel_to_tree()?.id(),
        repo.find_commit(base)?.tree_id()
    );
    let reverted = repo.head()?.peel_to_commit()?.id();
    history_action(&repo, &[base], HistoryAction::Reset(ResetMode::Soft))?;
    assert_eq!(status(&repo)?.changes, Vec::<FileChange>::new()); // revert tree already equals base
    history_action(&repo, &[base], HistoryAction::Checkout)?;
    assert!(repo.head_detached()?);
    assert!(repo.find_commit(reverted).is_ok()); // still recoverable via reflog/object
    Ok(())
}
#[test]
fn history_cherry_pick_is_ordered_and_refuses_conflicts_without_switching()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    use crate::model::HistoryAction;
    let (_dir, repo) = repository()?;
    let base = history_commit(&repo, "base", "base\n")?;
    repo.branch("destination", &repo.find_commit(base)?, false)?;
    let a = history_commit(&repo, "a", "a\n")?;
    let b = history_commit(&repo, "b", "b\n")?;
    history_action(
        &repo,
        &[b, a],
        HistoryAction::CherryPick {
            branch: "destination".into(),
            create: false,
        },
    )?;
    assert_eq!(status(&repo)?.branch, "destination");
    assert_eq!(
        history(&repo, 3)?
            .iter()
            .map(|c| c.summary.as_str())
            .collect::<Vec<_>>(),
        vec!["b", "a", "base"]
    );
    history_action(
        &repo,
        &[a],
        HistoryAction::CherryPick {
            branch: "new-branch".into(),
            create: true,
        },
    )?;
    assert_eq!(status(&repo)?.branch, "new-branch");
    let change = history_commit(&repo, "base", "change\n")?;
    let incompatible = history_commit(&repo, "base", "incompatible\n")?;
    let before = status(&repo)?;
    assert!(
        history_action(
            &repo,
            &[change],
            HistoryAction::CherryPick {
                branch: "new-branch".into(),
                create: false
            }
        )
        .is_err()
    );
    assert_eq!(repo.head()?.target(), Some(incompatible));
    assert_eq!(status(&repo)?.branch, before.branch);
    assert_eq!(status(&repo)?.changes, Vec::<FileChange>::new());
    Ok(())
}

#[test]
fn history_reset_modes_and_same_sha_branch_gate()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    use crate::model::{HistoryAction, HistoryRequest, ResetMode};
    for mode in [ResetMode::Soft, ResetMode::Mixed, ResetMode::Hard] {
        let (_dir, repo) = repository()?;
        let base = history_commit(&repo, "file", "base\n")?;
        let tip = history_commit(&repo, "file", "committed\n")?;
        fs::write(workdir(&repo)?.join("file"), "staged\n")?;
        stage(&repo, &["file".into()], true)?;
        fs::write(workdir(&repo)?.join("file"), "working\n")?;
        fs::write(workdir(&repo)?.join("untracked"), "retain\n")?;
        history_action(&repo, &[base], HistoryAction::Reset(mode))?;
        assert_eq!(repo.head()?.target(), Some(base));
        assert_eq!(
            fs::read_to_string(workdir(&repo)?.join("untracked"))?,
            "retain\n"
        );
        let index = repo.index()?;
        let indexed = repo.find_blob(
            index
                .get_path(std::path::Path::new("file"), 0)
                .ok_or("index entry")?
                .id,
        )?;
        assert_eq!(
            indexed.content(),
            if mode == ResetMode::Soft {
                b"staged\n".as_slice()
            } else {
                b"base\n".as_slice()
            }
        );
        assert_eq!(
            fs::read_to_string(workdir(&repo)?.join("file"))?,
            if mode == ResetMode::Hard {
                "base\n"
            } else {
                "working\n"
            }
        );
        assert!(repo.find_commit(tip).is_ok());
    }
    let (_dir, repo) = repository()?;
    let head = history_commit(&repo, "file", "base\n")?;
    let branch = status(&repo)?.branch;
    repo.branch("other", &repo.find_commit(head)?, false)?;
    repo.set_head("refs/heads/other")?;
    let request = HistoryRequest {
        expected_head: head.to_string(),
        expected_branch: branch,
        commits: vec![head.to_string()],
        action: HistoryAction::Amend("wrong branch".into()),
    };
    assert!(crate::history_actions::execute(&repo, request).is_err());
    assert_eq!(repo.head()?.target(), Some(head));
    Ok(())
}

#[test]
fn history_merge_commits_use_first_parent_and_tags_skip_noncommits()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    use crate::model::HistoryAction;
    let (_dir, repo) = repository()?;
    let base = history_commit(&repo, "base", "base\n")?;
    let original = status(&repo)?.branch;
    repo.branch("feature", &repo.find_commit(base)?, false)?;
    repo.branch("destination", &repo.find_commit(base)?, false)?;
    checkout(&repo, "feature", false)?;
    history_commit(&repo, "feature-file", "feature\n")?;
    checkout(&repo, &original, false)?;
    let local = history_commit(&repo, "local-file", "local\n")?;
    crate::merge::begin(&repo, "feature")?;
    let merged = repo.head()?.peel_to_commit()?.id();
    assert_eq!(repo.find_commit(merged)?.parent_count(), 2);
    repo.tag_lightweight(
        "blob-tag",
        &repo.find_blob(repo.blob(b"tagged blob")?)?.into_object(),
        false,
    )?;
    history_action(&repo, &[merged], HistoryAction::Tag("release".into()))?;
    let entries = history(&repo, 10)?;
    assert_eq!(entries.len(), 3); // first parent: merge, local, base
    assert_eq!(entries[0].tags, vec!["release"]);
    assert!(
        matches!(history_action(&repo, &[merged], HistoryAction::CopyTags)?, Outcome::Clipboard(tags) if tags == "release")
    );
    history_action(
        &repo,
        &[merged],
        HistoryAction::CherryPick {
            branch: "destination".into(),
            create: false,
        },
    )?;
    let tree = repo.head()?.peel_to_tree()?;
    assert!(tree.get_name("feature-file").is_some());
    assert!(tree.get_name("local-file").is_none());
    checkout(&repo, &original, false)?;
    history_action(
        &repo,
        &[merged, local],
        HistoryAction::Squash("Combined merge".into()),
    )?;
    assert_eq!(repo.head()?.peel_to_commit()?.parent_count(), 1);
    assert_eq!(
        repo.head()?.peel_to_tree()?.id(),
        repo.find_commit(merged)?.tree_id()
    );
    Ok(())
}

#[test]
fn history_selected_files_union_uses_newest_selected_delta_and_not_skipped_commits()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let (dir, repo) = repository()?;
    history_commit(&repo, "shared.txt", "base\n")?;
    let old = history_commit(&repo, "shared.txt", "older selected\n")?;
    let old_only = history_commit(&repo, "old-only.txt", "selected file\n")?;
    history_commit(&repo, "skip-only.txt", "not selected\n")?;
    history_commit(&repo, "shared.txt", "skipped edit\n")?;
    let new = history_commit(&repo, "shared.txt", "newest selected\n")?;
    fs::write(dir.path().join("shared.txt"), "unsaved working edit\n")?;
    let index_before = fs::read(repo.path().join("index"))?;
    let ids = vec![new.to_string(), old_only.to_string(), old.to_string()];
    let selection = crate::history_selection::details(&repo, &ids)?;
    assert_eq!(selection.commits.len(), 3);
    assert_eq!(
        selection
            .files
            .iter()
            .map(|f| f.change.path.as_path())
            .collect::<Vec<_>>(),
        vec![Path::new("old-only.txt"), Path::new("shared.txt")]
    );
    let shared = selection
        .files
        .iter()
        .find(|f| f.change.path == Path::new("shared.txt"))
        .ok_or("shared file")?;
    assert_eq!(shared.commits, vec![new.to_string(), old.to_string()]);
    assert!(shared.worktree_exists);
    assert!(shared.github_url.is_none());
    let target = shared.diff_target().ok_or("diff target")?;
    assert_eq!(
        target,
        DiffTarget::Commit {
            sha: new.to_string(),
            path: "shared.txt".into()
        }
    );
    let patch = file_diff(&repo, &target)?;
    let lines = patch
        .hunks
        .iter()
        .flat_map(|h| &h.lines)
        .map(|l| l.text.as_str())
        .collect::<Vec<_>>();
    assert_eq!(lines, vec!["skipped edit\n", "newest selected\n"]);
    assert_eq!(repo.head()?.target(), Some(new));
    assert_eq!(fs::read(repo.path().join("index"))?, index_before);
    assert_eq!(
        fs::read_to_string(dir.path().join("shared.txt"))?,
        "unsaved working edit\n"
    );
    assert!(crate::history_selection::details(&repo, &[]).is_err());
    assert!(crate::history_selection::details(&repo, &[new.to_string(), new.to_string()]).is_err());
    Ok(())
}

#[test]
fn history_selected_files_keep_deletions_renames_and_encoded_historical_links()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let (dir, repo) = repository()?;
    let root = history_commit(&repo, "base", "base\n")?;
    let name = "a # ü.txt";
    let created = history_commit(&repo, name, "version\n")?;
    repo.remote("origin", "git@github.com:owner/project.git")?;
    fs::remove_file(dir.path().join(name))?;
    stage(&repo, &[name.into()], true)?;
    let deleted = git2::Oid::from_str(&commit(&repo, "Delete file", "")?.sha)?;
    let selection = crate::history_selection::details(
        &repo,
        &[deleted.to_string(), created.to_string(), root.to_string()],
    )?;
    let file = selection
        .files
        .iter()
        .find(|f| f.change.path == Path::new(name))
        .ok_or("deleted file")?;
    assert_eq!(file.change.status, ChangeStatus::Deleted);
    assert_eq!(file.commits, vec![deleted.to_string(), created.to_string()]);
    assert!(!file.worktree_exists);
    assert_eq!(
        file.github_url.as_deref(),
        Some(
            format!("https://github.com/owner/project/blob/{created}/a%20%23%20%C3%BC.txt")
                .as_str()
        )
    );
    let old = history_commit(&repo, "old.txt", "rename contents\n")?;
    fs::rename(dir.path().join("old.txt"), dir.path().join("renamed.txt"))?;
    let paths = status(&repo)?
        .changes
        .into_iter()
        .map(|c| c.path)
        .collect::<Vec<_>>();
    stage(&repo, &paths, true)?;
    let renamed = commit(&repo, "Rename file", "")?;
    let selection =
        crate::history_selection::details(&repo, &[renamed.sha.clone(), old.to_string()])?;
    assert_eq!(selection.files.len(), 2);
    let file = selection
        .files
        .iter()
        .find(|f| f.change.path == Path::new("renamed.txt"))
        .ok_or("renamed file")?;
    assert_eq!(file.change.status, ChangeStatus::Renamed);
    assert_eq!(file.change.old_path.as_deref(), Some(Path::new("old.txt")));
    assert_eq!(
        file.github_url.as_deref(),
        Some(
            format!(
                "https://github.com/owner/project/blob/{}/renamed.txt",
                renamed.sha
            )
            .as_str()
        )
    );
    repo.remote_set_url("origin", "https://example.invalid/repo.git")?;
    assert!(
        crate::history_selection::details(&repo, &[renamed.sha])?
            .files
            .iter()
            .all(|f| f.github_url.is_none())
    );
    Ok(())
}

#[test]
fn ignore_folder_is_anchored_escaped_and_preserves_tracked_files()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let (dir, repo) = repository()?;
    fs::create_dir_all(dir.path().join(".nozhik/logs"))?;
    fs::write(dir.path().join(".nozhik/logs/tracked.txt"), "tracked")?;
    stage(&repo, &[".nozhik/logs/tracked.txt".into()], true)?;
    commit(&repo, "Tracked log", "")?;
    let mut worker = GitWorker::default();
    worker.execute(Task::Open(dir.path().into()))?;
    worker.execute(Task::IgnoreFolder(".nozhik/logs".into()))?;
    assert!(repo.status_should_ignore(Path::new(".nozhik/logs/new.txt"))?);
    assert!(!repo.status_should_ignore(Path::new(".nozhik/other.txt"))?);
    assert!(!repo.status_should_ignore(Path::new("nested/.nozhik/logs/new.txt"))?);
    assert!(
        repo.index()?
            .get_path(Path::new(".nozhik/logs/tracked.txt"), 0)
            .is_some()
    );
    worker.execute(Task::IgnoreFolder("a[1] #!".into()))?;
    assert!(repo.status_should_ignore(Path::new("a[1] #!/child.txt"))?);
    assert!(!repo.status_should_ignore(Path::new("a1 #!/child.txt"))?);
    let before = fs::read_to_string(dir.path().join(".gitignore"))?;
    assert!(before.contains("/.nozhik/logs/\n"));
    for path in [
        "",
        ".",
        "..",
        "../outside",
        "/absolute",
        ".git",
        "bad\nfolder",
    ] {
        assert!(worker.execute(Task::IgnoreFolder(path.into())).is_err());
    }
    assert_eq!(fs::read_to_string(dir.path().join(".gitignore"))?, before);
    Ok(())
}

#[test]
fn commit_context_reads_only_staged_contents_and_bounds_large_patches()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let (dir, repo) = repository()?;
    let mut worker = GitWorker::default();
    worker.execute(Task::Open(dir.path().into()))?;
    assert!(worker.execute(Task::CommitContext).is_err());
    fs::write(dir.path().join("chosen.txt"), "staged version\n")?;
    stage(&repo, &["chosen.txt".into()], true)?;
    fs::write(dir.path().join("chosen.txt"), "unstaged secret\n")?;
    fs::write(dir.path().join("unselected.txt"), "unselected secret\n")?;
    let before = fs::read(repo.path().join("index"))?;
    let Outcome::CommitContext(first) = worker.execute(Task::CommitContext)? else {
        return Err("missing context".into());
    };
    assert!(first.patch.contains("+staged version"));
    assert!(!first.patch.contains("unstaged secret"));
    assert!(!first.patch.contains("unselected"));
    assert_eq!(fs::read(repo.path().join("index"))?, before);
    let Outcome::CommitContext(same) = worker.execute(Task::CommitContext)? else {
        return Err("missing context".into());
    };
    assert_eq!(same.fingerprint, first.fingerprint);
    fs::write(dir.path().join("large.txt"), "unicode 🦀\n".repeat(10_000))?;
    stage(&repo, &["large.txt".into()], true)?;
    let Outcome::CommitContext(large) = worker.execute(Task::CommitContext)? else {
        return Err("missing context".into());
    };
    assert_ne!(large.fingerprint, first.fingerprint);
    assert!(large.patch.len() < 25 * 1024);
    assert!(large.patch.contains("[Staged diff truncated"));
    fs::write(
        dir.path().join("large.txt"),
        "changed beyond limit\n".repeat(10_000),
    )?;
    stage(&repo, &["large.txt".into()], true)?;
    let Outcome::CommitContext(changed) = worker.execute(Task::CommitContext)? else {
        return Err("missing context".into());
    };
    assert_ne!(large.fingerprint, changed.fingerprint);
    Ok(())
}

#[test]
fn project_git_settings_persist_identity_remotes_and_keep_secrets_in_session()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    use crate::{
        clone::Authentication,
        connection::{AuthMethod, GitSettings},
    };
    let (dir, repo) = repository()?;
    let mut worker = GitWorker::default();
    worker.execute(Task::Open(dir.path().into()))?;
    let Outcome::GitSettings(original) = worker.execute(Task::GitSettings)? else {
        return Err("missing settings".into());
    };
    let auth = Authentication::Token(zeroize::Zeroizing::new("dummy-session-token".into()));
    let requested = GitSettings {
        remote_url: "owner/repo".into(),
        push_url: "https://github.com/owner/other.git".into(),
        author_name: "Project Author".into(),
        author_email: "author@example.invalid".into(),
        auth: auth.profile(),
        ..original.clone()
    };
    let Outcome::GitSettings(saved) = worker.execute(Task::SaveGitSettings {
        expected: original,
        settings: requested,
        authentication: auth,
    })?
    else {
        return Err("missing saved settings".into());
    };
    assert_eq!(saved.remote_url, "https://github.com/owner/repo");
    assert_eq!(
        repo.find_remote("origin")?.pushurl()?,
        Some("https://github.com/owner/other.git")
    );
    assert_eq!(repo.signature()?.name()?, "Project Author");
    assert_eq!(repo.signature()?.email()?, "author@example.invalid");
    assert!(
        matches!(worker.session.authentication(&saved), Authentication::Token(token) if token.as_str() == "dummy-session-token")
    );
    let blank_auth = Authentication::Token(zeroize::Zeroizing::default());
    worker.execute(Task::SaveGitSettings {
        expected: saved.clone(),
        settings: saved.clone(),
        authentication: blank_auth,
    })?;
    assert!(
        matches!(worker.session.authentication(&saved), Authentication::Token(token) if token.as_str() == "dummy-session-token")
    );
    let config = fs::read_to_string(repo.commondir().join("config"))?;
    assert!(!config.contains("dummy-session-token"));
    assert!(!repo.commondir().join("config.lock").exists());
    fs::write(dir.path().join("file.txt"), "new")?;
    worker.execute(Task::Stage {
        paths: vec!["file.txt".into()],
        staged: true,
    })?;
    worker.execute(Task::Commit {
        summary: "Local commit".into(),
        body: String::new(),
    })?;
    assert_eq!(
        repo.head()?.peel_to_commit()?.author().name()?,
        "Project Author"
    );
    let mut fresh = GitWorker::default();
    fresh.execute(Task::Open(dir.path().into()))?;
    let Outcome::GitSettings(reloaded) = fresh.execute(Task::GitSettings)? else {
        return Err("missing reloaded settings".into());
    };
    assert_eq!(reloaded.auth.method, AuthMethod::Token);
    assert!(
        matches!(fresh.session.authentication(&reloaded), Authentication::Token(token) if token.is_empty())
    );
    // External endpoint edits never inherit the old endpoint's session secret.
    repo.remote_set_url("origin", "https://github.com/another/repo")?;
    let current = crate::connection::read(&repo)?;
    assert!(
        matches!(worker.session.authentication(&current), Authentication::Token(token) if token.is_empty())
    );
    let before = fs::read(repo.commondir().join("config"))?;
    assert!(
        worker
            .execute(Task::SaveGitSettings {
                expected: saved.clone(),
                settings: saved,
                authentication: Authentication::Token(zeroize::Zeroizing::default())
            })
            .is_err()
    );
    assert_eq!(fs::read(repo.commondir().join("config"))?, before);
    assert!(!repo.commondir().join("config.lock").exists());
    Ok(())
}

#[test]
fn project_git_settings_validate_before_writing_and_test_local_remote()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    use crate::clone::Authentication;
    let (dir, repo) = repository()?;
    let remote_dir = tempfile::tempdir()?;
    Repository::init_bare(remote_dir.path())?;
    let mut worker = GitWorker::default();
    worker.execute(Task::Open(dir.path().into()))?;
    let expected = crate::connection::read(&repo)?;
    let mut settings = expected.clone();
    settings.remote_url = remote_dir.path().display().to_string();
    let before = fs::read(repo.commondir().join("config"))?;
    worker.execute(Task::TestGitConnection {
        settings: settings.clone(),
        authentication: Authentication::Automatic,
    })?;
    assert_eq!(
        fs::read(repo.commondir().join("config"))?,
        before,
        "connection test must not save"
    );
    worker.execute(Task::SaveGitSettings {
        expected,
        settings,
        authentication: Authentication::Automatic,
    })?;
    worker.execute(Task::Fetch)?;
    let saved = crate::connection::read(&repo)?;
    for invalid_url in [
        "https://user:dummy-password@host/repo",
        "https://host/repo?token=dummy",
        "invalid",
        "\n",
    ] {
        let mut invalid_settings = saved.clone();
        invalid_settings.remote_url = invalid_url.into();
        let before = fs::read(repo.commondir().join("config"))?;
        assert!(
            worker
                .execute(Task::SaveGitSettings {
                    expected: saved.clone(),
                    settings: invalid_settings,
                    authentication: Authentication::Automatic
                })
                .is_err()
        );
        assert_eq!(fs::read(repo.commondir().join("config"))?, before);
    }
    fs::write(repo.commondir().join("config.lock"), "external Git lock")?;
    assert!(
        worker
            .execute(Task::SaveGitSettings {
                expected: saved.clone(),
                settings: saved,
                authentication: Authentication::Automatic
            })
            .is_err()
    );
    assert_eq!(
        fs::read_to_string(repo.commondir().join("config.lock"))?,
        "external Git lock"
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn automatic_git_authentication_uses_repository_local_helper()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let (_dir, repo) = repository()?;
    repo.config()?.set_str(
        "credential.helper",
        "!f() { printf 'username=local-test-user\\npassword=local-test-secret\\n'; }; f",
    )?;
    let config = repo.config()?;
    let helper = git2::CredentialHelper::new("https://example.invalid/owner/repo")
        .config(&config)
        .execute();
    assert!(
        matches!(helper, Some((user, password)) if user == "local-test-user" && password == "local-test-secret")
    );
    let credential = crate::clone::credentials(
        &crate::clone::Authentication::Automatic,
        "https://example.invalid/owner/repo",
        None,
        git2::CredentialType::USER_PASS_PLAINTEXT,
        &config,
    )?;
    assert!(credential.has_username());
    Ok(())
}

#[test]
fn clone_credentials_refuse_an_origin_changed_before_project_open()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let (dir, repo) = repository()?;
    repo.remote("origin", "https://github.com/example/original")?;
    let auth =
        crate::clone::Authentication::Token(zeroize::Zeroizing::new("dummy-clone-secret".into()));
    crate::connection::remember_clone(&repo, &auth)?;
    repo.remote_set_url("origin", "https://github.com/example/changed")?;
    let mut worker = GitWorker::default();
    worker.execute(Task::Open(dir.path().into()))?;
    assert!(
        worker
            .execute(Task::Authenticate(crate::clone::Credentials {
                url: "https://github.com/example/original".into(),
                authentication: auth,
            }))
            .is_err()
    );
    let settings = crate::connection::read(&repo)?;
    assert!(
        matches!(worker.session.authentication(&settings), crate::clone::Authentication::Token(secret) if secret.is_empty())
    );
    Ok(())
}

#[test]
#[ignore = "Requires URL and session token on stdin; read-only GitHub connection"]
fn live_github_token_connection() -> std::result::Result<(), Box<dyn std::error::Error>> {
    use std::io::BufRead;
    let input = std::io::stdin();
    let mut input = input.lock();
    let mut url = String::new();
    input.read_line(&mut url)?;
    let url = crate::clone::normalize_url(url.trim())?;
    let mut token = zeroize::Zeroizing::new(String::new());
    input.read_line(&mut token)?;
    drop(input);
    if token.trim().is_empty() {
        return Err("No token supplied".into());
    }
    let directory = tempfile::tempdir()?;
    let repo = Repository::init(directory.path())?;
    let auth = crate::clone::Authentication::Token(token);
    let mut remote = repo.remote_anonymous(&url)?;
    let result = remote.connect_auth(
        git2::Direction::Fetch,
        Some(callbacks(&repo, &auth, &url)?),
        None,
    );
    match result {
        Ok(connection) => {
            println!(
                "GitHub libgit2 connection: successful; {} advertised refs",
                connection.list()?.len()
            );
        }
        Err(error) => {
            return Err(crate::transport_error::explain("Connection", &url, &error).into());
        }
    }
    Ok(())
}

#[test]
fn token_connection_sends_basic_credentials_and_reports_http_403()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    use std::io::{Read, Write};
    let (_dir, repo) = repository()?;
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let url = format!("http://{}/repo.git", listener.local_addr()?);
    let server = std::thread::spawn(move || -> std::result::Result<(), String> {
        for attempt in 0..2 {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            let (mut socket, _) = loop {
                match listener.accept() {
                    Ok(connection) => break connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        if std::time::Instant::now() >= deadline {
                            return Err("mock accept timeout".into());
                        }
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                    Err(error) => return Err(error.to_string()),
                }
            };
            socket
                .set_read_timeout(Some(std::time::Duration::from_secs(3)))
                .map_err(|error| error.to_string())?;
            let mut request = Vec::new();
            while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                let mut buffer = [0; 2048];
                let count = socket
                    .read(&mut buffer)
                    .map_err(|error| error.to_string())?;
                if count == 0 || request.len() > 16 * 1024 {
                    return Err("invalid mock request".into());
                }
                request.extend_from_slice(&buffer[..count]);
            }
            let request = String::from_utf8_lossy(&request).to_ascii_lowercase();
            if attempt == 0 {
                assert!(!request.contains("authorization:"));
                socket.write_all(b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"Git\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").map_err(|error| error.to_string())?;
            } else {
                // Base64 of the fixed dummy pair x-access-token:dummy-token, never a real credential.
                assert!(
                    request.contains("authorization: basic ec1hy2nlc3mtdg9rzw46zhvtbxktdg9rzw4=")
                );
                socket
                    .write_all(
                        b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    )
                    .map_err(|error| error.to_string())?;
            }
        }
        Ok(())
    });
    let auth =
        crate::clone::Authentication::Token(zeroize::Zeroizing::new(" dummy-token \n".into()));
    let error = test_connection(&repo, &url, &auth)
        .err()
        .ok_or("expected HTTP 403")?;
    let message = error.to_string();
    assert!(message.contains("HTTP 403"), "{message}");
    assert!(!message.contains("dummy-token"));
    server.join().map_err(|_| "mock server panicked")??;
    Ok(())
}
