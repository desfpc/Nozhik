use super::*;
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
    push(&repo)?;
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
    push(&peer)?;
    fetch(&repo)?;
    assert_eq!(status(&repo)?.behind, 1);
    pull(&repo)?;
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
    push(&peer)?;
    assert!(pull(&repo).is_err());
    assert!(push(&repo).is_err());
    assert_eq!(fs::read_to_string(dir.path().join("local.txt"))?, "local");
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
