//! Pointer-driven Git workflows against disposable repositories.
use super::*;
use egui::{Event, Pos2, Rect, Vec2};

fn commit_file(
    repo: &git2::Repository,
    path: &str,
    text: &str,
    message: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    std::fs::write(repo.workdir().ok_or("missing worktree")?.join(path), text)?;
    let mut index = repo.index()?;
    index.add_path(std::path::Path::new(path))?;
    index.write()?;
    let tree = repo.find_tree(index.write_tree()?)?;
    let parent = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
    let signature = repo.signature()?;
    repo.commit(
        Some("HEAD"),
        &signature,
        &signature,
        message,
        &tree,
        &parent.iter().collect::<Vec<_>>(),
    )?;
    Ok(())
}
fn fixture()
-> Result<(tempfile::TempDir, git2::Repository, VcsPanel, Context), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let repo = git2::Repository::init(dir.path())?;
    {
        let mut config = repo.config()?;
        config.set_str("user.name", "UI QA")?;
        config.set_str("user.email", "qa@example.invalid")?;
    }
    commit_file(&repo, "file.txt", "base\n", "Base")?;
    let original = repo.head()?.name()?.to_owned();
    let head = repo.head()?.peel_to_commit()?;
    repo.branch("feature", &head, false)?;
    drop(head);
    repo.set_head("refs/heads/feature")?;
    commit_file(&repo, "file.txt", "theirs\n", "Theirs")?;
    repo.set_head(&original)?;
    repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))?;
    commit_file(&repo, "file.txt", "ours\n", "Ours")?;
    let ctx = Context::default();
    theme::apply(&ctx);
    let mut panel = VcsPanel::default();
    panel.set_project(dir.path());
    settle(&mut panel, &ctx)?;
    Ok((dir, repo, panel, ctx))
}
fn frame(panel: &mut VcsPanel, ctx: &Context, events: Vec<Event>) -> egui::FullOutput {
    ctx.run(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1100.0, 800.0))),
            modifiers: events
                .iter()
                .find_map(|e| match e {
                    Event::PointerButton { modifiers, .. } => Some(*modifiers),
                    _ => None,
                })
                .unwrap_or_default(),
            events,
            ..Default::default()
        },
        |ctx| {
            panel.tick(ctx, false);
            egui::TopBottomPanel::top("git_test_toolbar")
                .exact_height(54.0)
                .show(ctx, |ui| {
                    ui.horizontal(|ui| panel.toolbar(ui, 300.0));
                });
            egui::SidePanel::left("git_test_sidebar")
                .exact_width(300.0)
                .show(ctx, |ui| panel.panel(ui));
            egui::CentralPanel::default().show(ctx, |ui| panel.center(ui, 13.0));
        },
    )
}
fn settle(panel: &mut VcsPanel, ctx: &Context) -> Result<(), Box<dyn std::error::Error>> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let _output = frame(panel, ctx, vec![]);
        if panel.pending.is_empty() {
            break;
        }
        if Instant::now() > deadline {
            return Err(format!("Git timeout: {:?}", panel.error).into());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    for _ in 0..3 {
        let _output = frame(panel, ctx, vec![]);
    }
    Ok(())
}
fn text_point(output: &egui::FullOutput, label: &str) -> Result<Pos2, Box<dyn std::error::Error>> {
    output
        .shapes
        .iter()
        .rev()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Text(t) if t.galley.text() == label && shape.clip_rect.contains(t.pos) => {
                Some(t.pos + t.galley.size() * 0.5)
            }
            _ => None,
        })
        .ok_or_else(|| format!("Missing UI label: {label}").into())
}
fn click_at(
    panel: &mut VcsPanel,
    ctx: &Context,
    pos: Pos2,
    button: egui::PointerButton,
    modifiers: egui::Modifiers,
) {
    for pressed in [true, false] {
        let _output = frame(
            panel,
            ctx,
            vec![
                Event::PointerMoved(pos),
                Event::PointerButton {
                    pos,
                    button,
                    pressed,
                    modifiers,
                },
            ],
        );
    }
}
fn click(
    panel: &mut VcsPanel,
    ctx: &Context,
    label: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let _sizing = frame(panel, ctx, vec![]);
    let output = frame(panel, ctx, vec![]);
    let point = text_point(&output, label).map_err(|error| {
        format!(
            "{error}: {:?}",
            output
                .shapes
                .iter()
                .filter_map(|s| match &s.shape {
                    egui::Shape::Text(t) => Some(t.galley.text()),
                    _ => None,
                })
                .collect::<Vec<_>>()
        )
    })?;
    click_at(
        panel,
        ctx,
        point,
        egui::PointerButton::Primary,
        egui::Modifiers::NONE,
    );
    Ok(())
}
#[test]
fn real_branch_popup_create_and_switch_with_dirty_work() -> Result<(), Box<dyn std::error::Error>> {
    let (dir, repo, mut panel, ctx) = fixture()?;
    std::fs::write(dir.path().join("file.txt"), "unsaved on disk\n")?;
    panel.submit(Task::Refresh);
    settle(&mut panel, &ctx)?;
    panel.submit(Task::Stage {
        paths: vec!["file.txt".into()],
        staged: true,
    });
    settle(&mut panel, &ctx)?;
    panel.summary = "Do not commit through an open modal".into();
    click(&mut panel, &ctx, "Current Branch")?;
    assert!(panel.open_branches);
    settle(&mut panel, &ctx)?;
    click(&mut panel, &ctx, "New Branch")?;
    assert!(panel.dialog == controls::Dialog::NewBranch);
    panel.commit_selected();
    assert_eq!(panel.mutations.len(), 0);
    assert_eq!(repo.head()?.peel_to_commit()?.summary()?, Some("Ours"));
    panel.new_branch = "ui-created".into();
    settle(&mut panel, &ctx)?;
    click(&mut panel, &ctx, "Create Branch")?;
    settle(&mut panel, &ctx)?;
    assert_eq!(panel.branch(), "ui-created");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("file.txt"))?,
        "unsaved on disk\n"
    );
    click(&mut panel, &ctx, "Current Branch")?;
    settle(&mut panel, &ctx)?;
    click(&mut panel, &ctx, "feature")?;
    assert!(matches!(panel.dialog, controls::Dialog::Switch(_)));
    settle(&mut panel, &ctx)?;
    // Default radio choice leaves changes on ui-created.
    click(&mut panel, &ctx, "Switch Branch")?;
    settle(&mut panel, &ctx)?;
    assert_eq!(panel.branch(), "feature");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("file.txt"))?,
        "theirs\n"
    );
    assert!(
        repo.find_branch("ui-created", git2::BranchType::Local)
            .is_ok()
    );
    Ok(())
}
#[test]
fn real_merge_preview_and_continue_button_follows_saved_resolution()
-> Result<(), Box<dyn std::error::Error>> {
    let (dir, repo, mut panel, ctx) = fixture()?;
    panel.open_branches = true;
    settle(&mut panel, &ctx)?;
    let label = format!("Choose a branch to merge into {}", panel.branch());
    click(&mut panel, &ctx, &label)?;
    settle(&mut panel, &ctx)?;
    click(&mut panel, &ctx, "feature")?;
    settle(&mut panel, &ctx)?;
    assert_eq!(panel.merge_preview, Some(1));
    assert_eq!(repo.state(), git2::RepositoryState::Clean);
    click(&mut panel, &ctx, "Create a merge commit")?;
    settle(&mut panel, &ctx)?;
    assert!(panel.dialog == controls::Dialog::Conflicts);
    assert!(panel.is_merging());
    click(&mut panel, &ctx, "Continue Merge")?;
    settle(&mut panel, &ctx)?;
    assert_eq!(repo.state(), git2::RepositoryState::Merge);
    std::fs::write(dir.path().join("file.txt"), "resolved in editor\n")?;
    panel.submit(Task::Refresh);
    settle(&mut panel, &ctx)?;
    assert!(
        panel
            .repo
            .as_ref()
            .is_some_and(|r| r.conflicts.iter().all(|c| c.remaining == 0))
    );
    click(&mut panel, &ctx, "Continue Merge")?;
    settle(&mut panel, &ctx)?;
    assert_eq!(repo.state(), git2::RepositoryState::Clean);
    assert_eq!(repo.head()?.peel_to_commit()?.parent_count(), 2);
    assert!(panel.error.is_none(), "{:?}", panel.error);
    Ok(())
}
#[test]
fn file_multiselection_menu_and_poll_retain_diff_and_control_availability()
-> Result<(), Box<dyn std::error::Error>> {
    let (dir, _repo, mut panel, ctx) = fixture()?;
    std::fs::write(dir.path().join("file.txt"), "changed\n")?;
    std::fs::write(dir.path().join("other.txt"), "new\n")?;
    panel.submit(Task::Refresh);
    settle(&mut panel, &ctx)?;
    let before = frame(&mut panel, &ctx, vec![]);
    let point = text_point(&before, "other.txt")?;
    click_at(
        &mut panel,
        &ctx,
        point,
        egui::PointerButton::Primary,
        egui::Modifiers {
            command: true,
            ..Default::default()
        },
    );
    assert_eq!(panel.selected_paths.len(), 2);
    click_at(
        &mut panel,
        &ctx,
        point,
        egui::PointerButton::Secondary,
        egui::Modifiers::NONE,
    );
    settle(&mut panel, &ctx)?;
    click(&mut panel, &ctx, "Include Selected Files")?;
    settle(&mut panel, &ctx)?;
    assert_eq!(
        panel
            .repo
            .as_ref()
            .map(|r| r.changes.iter().filter(|c| c.is_staged).count()),
        Some(2)
    );
    panel.selected_paths = vec!["file.txt".into()];
    panel.request_diff(DiffTarget::Working {
        path: "file.txt".into(),
        staged: false,
    });
    settle(&mut panel, &ctx)?;
    let display = panel.displayed_target.clone();
    let shapes = frame(&mut panel, &ctx, vec![]);
    let old_y = text_point(&shapes, "changed")?.y;
    panel.submit(Task::Refresh);
    assert_eq!(panel.mutations.len(), 0);
    assert_eq!(panel.displayed_target, display);
    let next = frame(&mut panel, &ctx, vec![]);
    assert!((text_point(&next, "changed")?.y - old_y).abs() < 0.1);
    settle(&mut panel, &ctx)?;
    panel.request_diff(DiffTarget::Working {
        path: "other.txt".into(),
        staged: false,
    });
    assert_eq!(
        panel.displayed_target,
        Some(DiffTarget::Working {
            path: "other.txt".into(),
            staged: false
        })
    ); // Cached file switches immediately.
    let displayed = panel.displayed_target.clone();
    panel.request_diff(DiffTarget::Working {
        path: "uncached.txt".into(),
        staged: false,
    });
    assert_eq!(panel.displayed_target, displayed); // No empty diff while an uncached target loads.
    Ok(())
}

#[test]
fn sparse_file_rows_and_footer_keep_their_geometry_across_popup_and_refresh()
-> Result<(), Box<dyn std::error::Error>> {
    let (dir, _repo, mut panel, ctx) = fixture()?;
    for path in ["one.txt", "two.txt"] {
        std::fs::write(dir.path().join(path), "new\n")?;
    }
    panel.submit(Task::Refresh);
    settle(&mut panel, &ctx)?;
    let before = frame(&mut panel, &ctx, vec![]);
    let sidebar_point =
        |output: &egui::FullOutput, label: &str| -> Result<Pos2, Box<dyn std::error::Error>> {
            output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(t) if t.galley.text() == label && t.pos.x < 300.0 => {
                        Some(t.pos + t.galley.size() * 0.5)
                    }
                    _ => None,
                })
                .ok_or_else(|| format!("Missing sidebar label: {label}").into())
        };
    let one = sidebar_point(&before, "one.txt")?;
    let two = sidebar_point(&before, "two.txt")?;
    assert!(
        (two.y - one.y - 28.0).abs() < 2.0,
        "rows should have a fixed 28-point height: {one:?} {two:?}"
    );
    let summary = text_point(&before, "Summary (required)")?;
    assert!(
        summary.y > 550.0,
        "footer must stay near the bottom: {summary:?}"
    );
    panel.open_branches = true;
    settle(&mut panel, &ctx)?;
    panel.open_branches = false;
    let after = frame(&mut panel, &ctx, vec![]);
    assert!((text_point(&after, "Summary (required)")?.y - summary.y).abs() < 0.1);
    assert!((sidebar_point(&after, "one.txt")?.y - one.y).abs() < 0.1);
    Ok(())
}

#[test]
fn history_full_width_rows_select_commits_and_files_in_separate_columns()
-> Result<(), Box<dyn std::error::Error>> {
    let (_dir, repo, mut panel, ctx) = fixture()?;
    commit_file(&repo, "second.txt", "second file\n", "Two files")?;
    // Amend the newest commit so the selected commit has two changed paths.
    let head = repo.head()?.peel_to_commit()?;
    std::fs::write(
        repo.workdir().ok_or("worktree")?.join("file.txt"),
        "updated\n",
    )?;
    let mut index = repo.index()?;
    index.add_path(std::path::Path::new("file.txt"))?;
    index.write()?;
    let tree = repo.find_tree(index.write_tree()?)?;
    head.amend(Some("HEAD"), None, None, None, None, Some(&tree))?;
    panel.submit(Task::Refresh);
    settle(&mut panel, &ctx)?;
    click(&mut panel, &ctx, "History")?;
    settle(&mut panel, &ctx)?;
    let newest = panel.history[0].sha.clone();
    let output = frame(&mut panel, &ctx, vec![]);
    let old = text_point(&output, "Ours")?;
    // Far beyond the short title, inside the right-hand padding of its row.
    click_at(
        &mut panel,
        &ctx,
        Pos2::new(280.0, old.y + 20.0),
        egui::PointerButton::Primary,
        egui::Modifiers::NONE,
    );
    settle(&mut panel, &ctx)?;
    assert_ne!(panel.history_sha.as_ref(), Some(&newest));
    let output = frame(&mut panel, &ctx, vec![]);
    let newest_point = text_point(&output, "Two files")?;
    click_at(
        &mut panel,
        &ctx,
        Pos2::new(280.0, newest_point.y + 20.0),
        egui::PointerButton::Primary,
        egui::Modifiers::NONE,
    );
    settle(&mut panel, &ctx)?;
    assert_eq!(panel.history_sha.as_ref(), Some(&newest));
    let output = frame(&mut panel, &ctx, vec![]);
    let second = text_point(&output, "second.txt")?;
    let first = output
        .shapes
        .iter()
        .find_map(|s| match &s.shape {
            egui::Shape::Text(t) if t.galley.text() == "file.txt" && t.pos.x < 600.0 => {
                Some(t.pos + t.galley.size() * 0.5)
            }
            _ => None,
        })
        .ok_or("file row")?;
    assert!(second.x > 300.0 && second.x < 600.0);
    assert!((second.y - first.y - 32.0).abs() < 1.0);
    // Click empty space at the end of the file row.
    click_at(
        &mut panel,
        &ctx,
        Pos2::new(530.0, second.y),
        egui::PointerButton::Primary,
        egui::Modifiers::NONE,
    );
    settle(&mut panel, &ctx)?;
    assert_eq!(
        panel.diff_target,
        Some(DiffTarget::Commit {
            sha: newest,
            path: "second.txt".into()
        })
    );
    let output = frame(&mut panel, &ctx, vec![]);
    let diff_header = text_point(&output, "second file")?;
    assert!(
        diff_header.x > 600.0,
        "diff is to the right of the files: {diff_header:?}"
    );
    Ok(())
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "Pointer-driven flow verifies selection, menu targeting and confirmed mutation together"
)]
fn history_pointer_multiselection_menus_and_confirmed_revert()
-> Result<(), Box<dyn std::error::Error>> {
    let (_dir, repo, mut panel, ctx) = fixture()?;
    commit_file(&repo, "second.txt", "second\n", "Second history change")?;
    commit_file(&repo, "third.txt", "third\n", "Third history change")?;
    panel.submit(Task::Refresh);
    settle(&mut panel, &ctx)?;
    click(&mut panel, &ctx, "History")?;
    settle(&mut panel, &ctx)?;
    let labels = [
        "Third history change",
        "Second history change",
        "Ours",
        "Base",
    ];
    let output = frame(&mut panel, &ctx, vec![]);
    let points = labels
        .iter()
        .map(|label| {
            output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text)
                        if text.galley.text() == *label && text.pos.x < 300.0 =>
                    {
                        Some(Pos2::new(280.0, text.pos.y + 24.0))
                    }
                    _ => None,
                })
                .ok_or_else(|| format!("Missing history row {label}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    click_at(
        &mut panel,
        &ctx,
        points[0],
        egui::PointerButton::Primary,
        egui::Modifiers::NONE,
    );
    click_at(
        &mut panel,
        &ctx,
        points[2],
        egui::PointerButton::Primary,
        egui::Modifiers::SHIFT,
    );
    assert_eq!(panel.history_selection.len(), 3);
    click_at(
        &mut panel,
        &ctx,
        points[1],
        egui::PointerButton::Primary,
        egui::Modifiers::COMMAND,
    );
    assert_eq!(panel.history_selection.len(), 2);
    click_at(
        &mut panel,
        &ctx,
        points[0],
        egui::PointerButton::Secondary,
        egui::Modifiers::NONE,
    );
    assert_eq!(panel.history_selection.len(), 2);
    let output = frame(&mut panel, &ctx, vec![]);
    assert!(
        text_point(&output, "Cherry-pick 2 Commits…").is_ok(),
        "{:?}",
        output
            .shapes
            .iter()
            .filter_map(|s| if let egui::Shape::Text(t) = &s.shape {
                Some(t.galley.text())
            } else {
                None
            })
            .collect::<Vec<_>>()
    );
    assert!(text_point(&output, "Squash 2 Commits…").is_ok());
    click(&mut panel, &ctx, "Copy SHAs")?;
    settle(&mut panel, &ctx)?;
    // Right-click outside the selection replaces it before opening the single menu.
    click_at(
        &mut panel,
        &ctx,
        points[1],
        egui::PointerButton::Secondary,
        egui::Modifiers::NONE,
    );
    assert_eq!(panel.history_selection, vec![panel.history[1].sha.clone()]);
    click(&mut panel, &ctx, "Create Tag…")?;
    assert!(panel.modal_open());
    click(&mut panel, &ctx, "Cancel")?;
    settle(&mut panel, &ctx)?;
    assert!(!panel.modal_open());
    click_at(
        &mut panel,
        &ctx,
        points[0],
        egui::PointerButton::Secondary,
        egui::Modifiers::NONE,
    );
    click(&mut panel, &ctx, "Revert Changes in Commit…")?;
    let before = repo.head()?.peel_to_commit()?.id();
    assert!(panel.modal_open());
    assert_eq!(repo.head()?.target(), Some(before));
    click(&mut panel, &ctx, "Revert Commit")?;
    settle(&mut panel, &ctx)?;
    assert_ne!(repo.head()?.target(), Some(before));
    assert_eq!(repo.head()?.peel_to_commit()?.parent_id(0)?, before);
    assert!(!repo.workdir().ok_or("workdir")?.join("third.txt").exists());
    assert!(!panel.modal_open());
    assert!(panel.error.is_none(), "{:?}", panel.error);
    Ok(())
}

#[test]
fn remote_header_paints_pull_push_and_both_count_badges() -> Result<(), Box<dyn std::error::Error>>
{
    let (_dir, _repo, mut panel, ctx) = fixture()?;
    for (ahead, behind, title) in [
        (0, 0, "Fetch origin"),
        (3, 0, "Push origin"),
        (0, 2, "Pull origin"),
        (3, 2, "Pull & Push origin"),
    ] {
        let status = panel.repo.as_mut().ok_or("status")?;
        status.has_origin = true;
        status.has_upstream = true;
        status.ahead = ahead;
        status.behind = behind;
        let output = frame(&mut panel, &ctx, vec![]);
        assert!(text_point(&output, title).is_ok(), "missing {title}");
        if ahead > 0 {
            assert!(text_point(&output, "3").is_ok());
        }
        if behind > 0 {
            assert!(text_point(&output, "2").is_ok());
        }
    }
    Ok(())
}

#[test]
fn history_squash_cherry_pick_and_reorder_dialogs_apply_real_actions()
-> Result<(), Box<dyn std::error::Error>> {
    for operation in [
        "Squash 2 Commits…",
        "Cherry-pick 2 Commits…",
        "Reorder Commit…",
    ] {
        let (_dir, repo, mut panel, ctx) = fixture()?;
        commit_file(&repo, "second.txt", "second\n", "Second")?;
        commit_file(&repo, "third.txt", "third\n", "Third")?;
        let source = repo.head()?.name()?.to_owned();
        let source_tip = repo.head()?.target();
        panel.submit(Task::Refresh);
        settle(&mut panel, &ctx)?;
        click(&mut panel, &ctx, "History")?;
        settle(&mut panel, &ctx)?;
        let newest = panel.history[0].sha.clone();
        panel.history_click(&newest, egui::Modifiers::NONE);
        if !operation.starts_with("Reorder") {
            let second = panel.history[1].sha.clone();
            panel.history_click(&second, egui::Modifiers::COMMAND);
        }
        let output = frame(&mut panel, &ctx, vec![]);
        let point = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == "Third" && text.pos.x < 300.0 => {
                    Some(Pos2::new(280.0, text.pos.y + 24.0))
                }
                _ => None,
            })
            .ok_or("history row")?;
        click_at(
            &mut panel,
            &ctx,
            point,
            egui::PointerButton::Secondary,
            egui::Modifiers::NONE,
        );
        click(&mut panel, &ctx, operation)?;
        settle(&mut panel, &ctx)?;
        if operation.starts_with("Cherry") {
            let branch = panel
                .repo
                .as_ref()
                .ok_or("status")?
                .branch_info
                .iter()
                .find(|b| b.name == "feature")
                .ok_or("branch")?;
            let label = format!("{}    {}", branch.name, branch.relative_time);
            click(&mut panel, &ctx, &label)?;
        } else if operation.starts_with("Reorder") {
            click(&mut panel, &ctx, "Move down")?;
        }
        let confirm = if operation.starts_with("Cherry") {
            "Cherry-pick 2 Commits to feature"
        } else if operation.starts_with("Squash") {
            "Squash 2 Commits"
        } else {
            "Reorder Commits"
        };
        click(&mut panel, &ctx, confirm)?;
        settle(&mut panel, &ctx)?;
        assert!(panel.error.is_none(), "{operation}: {:?}", panel.error);
        assert!(!panel.modal_open());
        assert!(
            repo.workdir()
                .ok_or("workdir")?
                .join("second.txt")
                .is_file()
        );
        assert!(repo.workdir().ok_or("workdir")?.join("third.txt").is_file());
        if operation.starts_with("Squash") {
            assert_eq!(panel.history.len(), 3);
            assert_eq!(
                repo.head()?.peel_to_commit()?.parent(0)?.summary()?,
                Some("Ours")
            );
        } else if operation.starts_with("Cherry") {
            assert_eq!(panel.branch(), "feature");
            assert_eq!(repo.find_reference(&source)?.target(), source_tip);
            assert_eq!(panel.history[0].summary, "Third");
            assert_eq!(panel.history[1].summary, "Second");
        } else {
            assert_eq!(panel.history[0].summary, "Second");
            assert_eq!(panel.history[1].summary, "Third");
        }
    }
    Ok(())
}

fn open_history_file_menu(
    panel: &mut VcsPanel,
    ctx: &Context,
    path: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    settle(panel, ctx)?;
    let output = frame(panel, ctx, vec![]);
    let point = output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Text(t)
                if t.galley.text() == path && (300.0..550.0).contains(&t.pos.x) =>
            {
                Some(Pos2::new(530.0, t.pos.y + 7.0))
            }
            _ => None,
        })
        .ok_or_else(|| format!("Missing history file {path}"))?;
    click_at(
        panel,
        ctx,
        point,
        egui::PointerButton::Secondary,
        egui::Modifiers::NONE,
    );
    Ok(())
}
fn menu_action_output(
    panel: &mut VcsPanel,
    ctx: &Context,
    label: &str,
) -> Result<egui::FullOutput, Box<dyn std::error::Error>> {
    let _sizing = frame(panel, ctx, vec![]);
    let output = frame(panel, ctx, vec![]);
    let pos = text_point(&output, label)?;
    let event = |pressed| {
        vec![
            Event::PointerMoved(pos),
            Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            },
        ]
    };
    let _press = frame(panel, ctx, event(true));
    Ok(frame(panel, ctx, event(false)))
}
#[test]
fn history_group_files_use_newest_selected_commit_badges_and_reject_stale_replies()
-> Result<(), Box<dyn std::error::Error>> {
    let (_dir, repo, mut panel, ctx) = fixture()?;
    commit_file(&repo, "shared.txt", "older selected\n", "Shared old")?;
    let old = repo.head()?.peel_to_commit()?.id().to_string();
    commit_file(&repo, "old-only.txt", "old only\n", "Only old")?;
    let old_only = repo.head()?.peel_to_commit()?.id().to_string();
    commit_file(&repo, "skip-only.txt", "skip\n", "Skip")?;
    commit_file(&repo, "shared.txt", "skipped\n", "Skipped edit")?;
    commit_file(&repo, "shared.txt", "newest selected\n", "Shared new")?;
    let new = repo.head()?.peel_to_commit()?.id().to_string();
    panel.submit(Task::Refresh);
    settle(&mut panel, &ctx)?;
    panel.mode = Mode::History;
    panel.history_click(&old, egui::Modifiers::NONE);
    panel.history_click(&old_only, egui::Modifiers::COMMAND);
    panel.history_click(&new, egui::Modifiers::COMMAND);
    settle(&mut panel, &ctx)?;
    let details = panel.details.clone().ok_or("details")?;
    assert_eq!(
        details
            .files
            .iter()
            .map(|f| f.change.path.as_path())
            .collect::<Vec<_>>(),
        vec![
            std::path::Path::new("old-only.txt"),
            std::path::Path::new("shared.txt")
        ]
    );
    let output = frame(&mut panel, &ctx, vec![]);
    assert!(text_point(&output, "Showing changes from 3 commits").is_ok());
    assert!(text_point(&output, "2 commits").is_ok());
    open_history_file_menu(&mut panel, &ctx, "shared.txt")?;
    assert_eq!(
        panel.diff_target,
        Some(DiffTarget::Commit {
            sha: new.clone(),
            path: "shared.txt".into()
        })
    );
    // Dismiss the menu before changing the selected commits.
    let _output = menu_action_output(&mut panel, &ctx, "Copy Relative File Path")?;
    settle(&mut panel, &ctx)?;
    let stale_id = panel.details_request;
    panel.history_click(&new, egui::Modifiers::COMMAND);
    panel.accept(stale_id, Outcome::SelectionDetails(details), &ctx);
    assert!(panel.details.is_none());
    settle(&mut panel, &ctx)?;
    open_history_file_menu(&mut panel, &ctx, "shared.txt")?;
    assert_eq!(
        panel.diff_target,
        Some(DiffTarget::Commit {
            sha: old.clone(),
            path: "shared.txt".into()
        })
    );
    let _output = menu_action_output(&mut panel, &ctx, "Copy Relative File Path")?;
    settle(&mut panel, &ctx)?;
    panel.history_click(&old, egui::Modifiers::COMMAND);
    panel.history_click(&old_only, egui::Modifiers::COMMAND);
    settle(&mut panel, &ctx)?;
    assert!(panel.details.is_none());
    assert!(panel.diff_target.is_none());
    Ok(())
}
#[test]
fn history_file_context_menu_opens_current_file_copies_paths_and_links_to_revision()
-> Result<(), Box<dyn std::error::Error>> {
    let (_dir, repo, mut panel, ctx) = fixture()?;
    repo.remote("origin", "https://github.com/owner/project.git")?;
    commit_file(&repo, "a # ü.txt", "historical\n", "Historical file")?;
    let sha = repo.head()?.peel_to_commit()?.id().to_string();
    panel.submit(Task::Refresh);
    settle(&mut panel, &ctx)?;
    panel.mode = Mode::History;
    panel.history_click(&sha, egui::Modifiers::NONE);
    settle(&mut panel, &ctx)?;
    open_history_file_menu(&mut panel, &ctx, "a # ü.txt")?;
    let _sizing = frame(&mut panel, &ctx, vec![]);
    let output = frame(&mut panel, &ctx, vec![]);
    for label in [
        "Reveal in File Manager",
        "Open in Nozhik",
        "Open with Default Program",
        "Copy File Path",
        "Copy Relative File Path",
        "View on GitHub",
    ] {
        assert!(text_point(&output, label).is_ok(), "Missing {label}");
    }
    assert!(text_point(&output, "Include Selected Files").is_err());
    assert!(text_point(&output, "Discard 1 Selected Changes…").is_err());
    let output = menu_action_output(&mut panel, &ctx, "Copy File Path")?;
    let root = panel.repo.as_ref().ok_or("repo")?.root.clone();
    assert!(output.platform_output.commands.iter().any(|command| matches!(command,egui::OutputCommand::CopyText(text) if text == &root.join("a # ü.txt").display().to_string())));
    open_history_file_menu(&mut panel, &ctx, "a # ü.txt")?;
    let output = menu_action_output(&mut panel, &ctx, "Copy Relative File Path")?;
    assert!(output.platform_output.commands.iter().any(
        |command| matches!(command,egui::OutputCommand::CopyText(text) if text == "a # ü.txt")
    ));
    open_history_file_menu(&mut panel, &ctx, "a # ü.txt")?;
    click(&mut panel, &ctx, "Open in Nozhik")?;
    assert_eq!(panel.take_open_paths(), vec![root.join("a # ü.txt")]);
    open_history_file_menu(&mut panel, &ctx, "a # ü.txt")?;
    let output = menu_action_output(&mut panel, &ctx, "View on GitHub")?;
    let expected = format!("https://github.com/owner/project/blob/{sha}/a%20%23%20%C3%BC.txt");
    assert!(
        output.platform_output.commands.iter().any(
            |command| matches!(command,egui::OutputCommand::OpenUrl(url) if url.url == expected)
        )
    );
    assert_eq!(repo.head()?.peel_to_commit()?.id().to_string(), sha);
    Ok(())
}

#[test]
fn history_missing_worktree_and_unsupported_origin_disable_only_unavailable_actions()
-> Result<(), Box<dyn std::error::Error>> {
    let (dir, repo, mut panel, ctx) = fixture()?;
    repo.remote("origin", "https://example.invalid/project.git")?;
    let sha = panel.history[0].sha.clone();
    std::fs::remove_file(dir.path().join("file.txt"))?;
    panel.mode = Mode::History;
    panel.history_click(&sha, egui::Modifiers::NONE);
    settle(&mut panel, &ctx)?;
    let file = panel
        .details
        .as_ref()
        .and_then(|d| d.files.first())
        .ok_or("file details")?;
    assert!(!file.worktree_exists);
    assert!(file.github_url.is_none());
    open_history_file_menu(&mut panel, &ctx, "file.txt")?;
    let output = menu_action_output(&mut panel, &ctx, "Open in Nozhik")?;
    assert_eq!(panel.take_open_paths(), Vec::<PathBuf>::new());
    assert!(
        !output
            .platform_output
            .commands
            .iter()
            .any(|c| matches!(c, egui::OutputCommand::OpenUrl(_)))
    );
    open_history_file_menu(&mut panel, &ctx, "file.txt")?;
    let output = menu_action_output(&mut panel, &ctx, "View on GitHub")?;
    assert!(
        !output
            .platform_output
            .commands
            .iter()
            .any(|c| matches!(c, egui::OutputCommand::OpenUrl(_)))
    );
    open_history_file_menu(&mut panel, &ctx, "file.txt")?;
    let output = menu_action_output(&mut panel, &ctx, "Copy Relative File Path")?;
    assert!(
        output
            .platform_output
            .commands
            .iter()
            .any(|c| matches!(c,egui::OutputCommand::CopyText(text) if text == "file.txt"))
    );
    assert!(!dir.path().join("file.txt").exists());
    Ok(())
}
