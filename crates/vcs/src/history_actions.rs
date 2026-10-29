//! Preflight history edits in memory; publish refs only after every patch succeeds.
use crate::{
    Outcome, VcsError,
    git_worker::{invalid, require_clean},
    model::{HistoryAction, HistoryRequest, ResetMode},
};
use git2::{Oid, Repository};
use std::collections::HashSet;
type Result<T> = std::result::Result<T, VcsError>;

#[allow(
    clippy::too_many_lines,
    reason = "Exhaustive history command dispatch shares HEAD and selection preflight"
)]
pub fn execute(repo: &Repository, request: HistoryRequest) -> Result<Outcome> {
    let head = repo.head()?.peel_to_commit()?;
    if head.id().to_string() != request.expected_head
        || crate::git_worker::status(repo)?.branch != request.expected_branch
    {
        return Err(invalid(
            "HEAD changed. Reopen the action from the updated history.",
        ));
    }
    let ids: Vec<Oid> = request
        .commits
        .iter()
        .map(|s| Oid::from_str(s))
        .collect::<std::result::Result<_, _>>()?;
    if ids.len() > 10_000 || ids.iter().collect::<HashSet<_>>().len() != ids.len() {
        return Err(invalid("Select at most 10,000 distinct commits"));
    }
    if !matches!(
        request.action,
        HistoryAction::CherryPick { .. } | HistoryAction::Squash(_) | HistoryAction::Reorder
    ) && ids.len() != 1
    {
        return Err(invalid("This action requires exactly one commit"));
    }
    let first = *ids
        .first()
        .ok_or_else(|| invalid("Select at least one commit"))?;
    let selected = repo.find_commit(first)?;
    if matches!(request.action, HistoryAction::CopyTags) {
        let mut tags = Vec::new();
        for name in repo
            .tag_names(None)?
            .iter()
            .filter_map(std::result::Result::ok)
            .flatten()
        {
            let object = repo.revparse_single(&format!("refs/tags/{name}"))?;
            if object
                .peel_to_commit()
                .is_ok_and(|commit| commit.id() == first)
            {
                tags.push(name.to_owned());
            }
        }
        return Ok(Outcome::Clipboard(tags.join("\n")));
    }
    if matches!(request.action, HistoryAction::View) {
        let base = crate::history_selection::github_base(repo)?;
        return Ok(Outcome::CommitUrl(format!("{base}/commit/{first}")));
    }
    if repo.state() != git2::RepositoryState::Clean {
        return Err(invalid("Finish the current repository operation first"));
    }
    match request.action {
        HistoryAction::Branch(name) => {
            repo.branch(name.trim(), &selected, false)?;
        }
        HistoryAction::Tag(name) => {
            repo.tag_lightweight(name.trim(), selected.as_object(), false)?;
        }
        HistoryAction::Reset(mode) => {
            // Soft/mixed deliberately retain local work; hard is explicitly confirmed in UI.
            if !repo.graph_descendant_of(head.id(), first)? && head.id() != first {
                return Err(invalid("Reset target is no longer in this branch"));
            }
            repo.reset(
                selected.as_object(),
                match mode {
                    ResetMode::Soft => git2::ResetType::Soft,
                    ResetMode::Mixed => git2::ResetType::Mixed,
                    ResetMode::Hard => git2::ResetType::Hard,
                },
                None,
            )?;
        }
        HistoryAction::Checkout => {
            require_clean(repo)?;
            checkout(repo, &selected)?;
            repo.set_head_detached(first)?;
        }
        HistoryAction::Amend(message) => {
            if first != head.id() {
                return Err(invalid("Only HEAD can be amended"));
            }
            validate_message(&message)?;
            let mut index = repo.index()?;
            if index.has_conflicts() {
                return Err(invalid("Resolve index conflicts first"));
            }
            let tree = repo.find_tree(index.write_tree()?)?;
            selected.amend(
                Some("HEAD"),
                None,
                Some(&repo.signature()?),
                None,
                Some(&message),
                Some(&tree),
            )?;
        }
        HistoryAction::Revert => {
            require_clean(repo)?;
            if selected.parent_count() == 0 {
                return Err(invalid("Reverting a root commit is unavailable"));
            }
            let mut index = repo.revert_commit(
                &selected,
                &head,
                u32::from(selected.parent_count() > 1),
                None,
            )?;
            if index.has_conflicts() {
                return Err(invalid(
                    "Revert conflicts with this branch; no files or refs changed",
                ));
            }
            let tree = repo.find_tree(index.write_tree_to(repo)?)?;
            let signature = repo.signature()?;
            let id = repo.commit(
                None,
                &signature,
                &signature,
                &format!(
                    "Revert \"{}\"\n\nThis reverts commit {first}.",
                    selected.summary()?.unwrap_or("")
                ),
                &tree,
                &[&head],
            )?;
            publish(repo, head.id(), id)?;
        }
        HistoryAction::CherryPick { branch, create } => {
            require_clean(repo)?;
            let target = if create {
                if !git2::Reference::is_valid_name(&format!("refs/heads/{branch}"))
                    || repo.find_branch(&branch, git2::BranchType::Local).is_ok()
                {
                    return Err(invalid("Choose a valid unused local branch name"));
                }
                head.id()
            } else {
                repo.find_branch(&branch, git2::BranchType::Local)?
                    .get()
                    .peel_to_commit()?
                    .id()
            };
            let mut tip = target;
            for id in ids.iter().rev() {
                tip = replay(repo, *id, tip)?;
            }
            let commit = repo.find_commit(tip)?;
            checkout(repo, &commit)?;
            if create {
                repo.branch(&branch, &commit, false)?;
            } else {
                repo.find_branch(&branch, git2::BranchType::Local)?
                    .get_mut()
                    .set_target(tip, "Nozhik: cherry-pick")?;
            }
            repo.set_head(&format!("refs/heads/{branch}"))?;
        }
        HistoryAction::Squash(message) => {
            rewrite(repo, &ids, Some(&message))?;
        }
        HistoryAction::Reorder => {
            rewrite(repo, &ids, None)?;
        }
        HistoryAction::CopyTags | HistoryAction::View => {}
    }
    Ok(Outcome::Done("History action completed".into()))
}
fn validate_message(message: &str) -> Result<()> {
    if message.lines().next().is_none_or(|s| s.trim().is_empty()) || message.contains('\0') {
        return Err(invalid("A commit summary is required"));
    }
    Ok(())
}
fn checkout(repo: &Repository, commit: &git2::Commit<'_>) -> Result<()> {
    repo.checkout_tree(
        commit.as_object(),
        Some(
            git2::build::CheckoutBuilder::new()
                .safe()
                .overwrite_ignored(false),
        ),
    )?;
    Ok(())
}
fn publish(repo: &Repository, previous: Oid, next: Oid) -> Result<()> {
    let mut reference = repo.head()?;
    if reference.target() != Some(previous) {
        return Err(invalid("HEAD changed during the operation"));
    }
    checkout(repo, &repo.find_commit(next)?)?;
    reference.set_target(next, "Nozhik: edit history")?;
    Ok(())
}
fn replay(repo: &Repository, id: Oid, onto: Oid) -> Result<Oid> {
    let original = repo.find_commit(id)?;
    if original.parent_count() == 0 {
        return Err(invalid("Root commits cannot be replayed"));
    }
    let parent = repo.find_commit(onto)?;
    let mut index = repo.cherrypick_commit(
        &original,
        &parent,
        u32::from(original.parent_count() > 1),
        None,
    )?;
    if index.has_conflicts() {
        return Err(invalid(
            "The requested order conflicts; no files or refs changed",
        ));
    }
    let tree = repo.find_tree(index.write_tree_to(repo)?)?;
    Ok(repo.commit(
        None,
        &original.author(),
        &repo.signature()?,
        original.message()?,
        &tree,
        &[&parent],
    )?)
}
fn rewrite(repo: &Repository, ids: &[Oid], message: Option<&str>) -> Result<()> {
    require_clean(repo)?;
    if repo.head_detached()? {
        return Err(invalid("Check out a local branch before rewriting history"));
    }
    if ids.len() < 2 && message.is_some() {
        return Err(invalid("Select at least two commits to squash"));
    }
    let selected: HashSet<_> = ids.iter().copied().collect();
    if selected.len() != ids.len() {
        return Err(invalid("Duplicate commit selection"));
    }
    let head = repo.head()?.peel_to_commit()?;
    let mut chain = Vec::new();
    let mut cursor = head.id();
    let mut remaining = selected.clone();
    while !remaining.is_empty() {
        let commit = repo.find_commit(cursor)?;
        if commit.parent_count() == 0 || chain.len() >= 10_000 {
            return Err(invalid(
                "Selection must belong to one linear history after a non-root base",
            ));
        }
        chain.push(cursor);
        remaining.remove(&cursor);
        cursor = commit.parent_id(0)?;
    }
    let mut tip = cursor;
    if let Some(message) = message {
        validate_message(message)?;
        let positions: Vec<_> = chain
            .iter()
            .enumerate()
            .filter_map(|(i, id)| selected.contains(id).then_some(i))
            .collect();
        let start = *positions
            .first()
            .ok_or_else(|| invalid("Empty selection"))?;
        let end = *positions.last().ok_or_else(|| invalid("Empty selection"))?;
        if end - start + 1 != ids.len() {
            return Err(invalid("Squash requires consecutive commits"));
        }
        let newest = repo.find_commit(chain[start])?;
        let oldest = repo.find_commit(chain[end])?;
        tip = repo.commit(
            None,
            &oldest.author(),
            &repo.signature()?,
            message,
            &newest.tree()?,
            &[&repo.find_commit(cursor)?],
        )?;
        for id in chain[..start].iter().rev() {
            tip = replay(repo, *id, tip)?;
        }
    } else {
        if selected.len() != chain.len() {
            return Err(invalid(
                "Reorder must include the complete suffix through HEAD",
            ));
        }
        if ids == chain {
            return Ok(());
        }
        for id in ids.iter().rev() {
            tip = replay(repo, *id, tip)?;
        }
    }
    publish(repo, head.id(), tip)
}
