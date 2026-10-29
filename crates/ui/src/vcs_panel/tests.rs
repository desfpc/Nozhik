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
    let output = frame(panel, ctx, vec![]);
    let point = text_point(&output, label)?;
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
