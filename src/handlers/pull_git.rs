//! Landing, updating and reverting pull requests with gix. Does not spawn git.

use std::path::Path;

use gix::bstr::ByteSlice;

use super::BOT;
use crate::api::ApiResponse;
use ninot_common::git;

/// How a pull request lands on its base branch.
#[derive(Clone, Copy)]
pub(crate) enum LandMethod {
    /// A merge commit with the base and the head as parents.
    Merge,
    /// One commit on the base with the head's tree.
    Squash,
    /// The head's commits replayed onto the base.
    Rebase,
}

/// A git failure while landing or reverting a pull request.
pub(crate) enum GitFail {
    /// The trees do not merge.
    Conflict,
    /// A ref or commit is absent. The text is the message.
    Missing(&'static str),
    /// Any other git error, as text.
    Message(String),
}

impl GitFail {
    /// The REST response for this failure.
    pub(crate) fn rest(self) -> ApiResponse {
        match self {
            Self::Conflict => crate::errors::GithubError::MergeConflict.to_response(),
            Self::Missing(what) => ApiResponse::unprocessable(what),
            Self::Message(text) => ApiResponse::internal(text),
        }
    }

    /// Text for a GraphQL error.
    pub(crate) fn text(self) -> String {
        match self {
            Self::Conflict => "merge conflict".to_owned(),
            Self::Missing(what) => what.to_owned(),
            Self::Message(text) => text,
        }
    }
}

/// Commits `head_sha` onto `refs/heads/{base_ref}` and returns the new tip.
///
/// # Errors
///
/// Returns [`GitFail::Conflict`] when the trees do not merge, and [`GitFail::Missing`] when a
/// ref or commit is absent.
pub(crate) fn land(
    git_dir: &Path,
    method: LandMethod,
    base_ref: &str,
    head_sha: &str,
    headline: &str,
    body: &str,
) -> Result<String, GitFail> {
    let base_sha = tip(git_dir, &format!("refs/heads/{base_ref}"))?
        .ok_or(GitFail::Missing("base branch does not exist"))?;
    let sha = match method {
        LandMethod::Merge => merge_commit(git_dir, &base_sha, head_sha, &message(headline, body))?,
        LandMethod::Squash => {
            squash_commit(git_dir, &base_sha, head_sha, &message(headline, body))?
        }
        LandMethod::Rebase => rebase(git_dir, &base_sha, head_sha)?,
    };
    git::create_ref(git_dir, &format!("refs/heads/{base_ref}"), &sha, &BOT).map_err(other)?;
    Ok(sha)
}

/// Merges `base` into `head` and returns the merge commit sha.
///
/// Points `refs/heads/{head_ref}` at that commit.
///
/// # Errors
///
/// Returns [`GitFail::Conflict`] when the trees do not merge. Update-branch answers 422 for
/// that conflict, matching `git merge-tree --write-tree` failing.
pub(crate) fn merge_base_into_head(
    git_dir: &Path,
    head: &str,
    base: &str,
    head_ref: &str,
    base_ref: &str,
) -> Result<String, GitFail> {
    let message = format!("Merge branch '{base_ref}' into {head_ref}\n");
    let sha = merge_commit(git_dir, head, base, &message)?;
    git::create_ref(git_dir, &format!("refs/heads/{head_ref}"), &sha, &BOT).map_err(other)?;
    Ok(sha)
}

/// Reverts `commit_sha` onto `onto_sha` (its first parent is the "mainline") and returns the new
/// commit, without moving a ref.
///
/// # Errors
///
/// Returns [`GitFail::Conflict`] when the revert does not apply.
pub(crate) fn revert_commit(
    git_dir: &Path,
    onto_sha: &str,
    commit_sha: &str,
    message: &str,
) -> Result<String, GitFail> {
    let commit = git::commit(git_dir, commit_sha).map_err(other)?;
    let repo = open(git_dir)?;
    let ancestor = tree_id(&repo, commit_sha)?;
    let ours = tree_id(&repo, onto_sha)?;
    let theirs = match commit.parents.first() {
        Some(parent) => tree_id(&repo, parent)?,
        None => gix::ObjectId::empty_tree(gix::hash::Kind::Sha1),
    };
    let tree = merged_trees(&repo, ancestor, ours, theirs)?;
    let sha = write_commit(&repo, tree, &[oid(onto_sha)?], message)?;
    Ok(sha)
}

/// Whether merging `head` into `base` has unresolved conflicts.
///
/// # Errors
///
/// Returns git failures other than a conflict.
pub(crate) fn conflicts(git_dir: &Path, base: &str, head: &str) -> Result<bool, GitFail> {
    let repo = open(git_dir)?;
    let outcome = repo
        .merge_commits(oid(base)?, oid(head)?, labels(), commit_options(&repo)?)
        .map_err(|error| GitFail::Message(error.to_string()))?;
    Ok(unresolved(&outcome.tree_merge))
}

fn merge_commit(
    git_dir: &Path,
    ours: &str,
    theirs: &str,
    message: &str,
) -> Result<String, GitFail> {
    let repo = open(git_dir)?;
    let tree = merged_commits(&repo, ours, theirs)?;
    write_commit(&repo, tree, &[oid(ours)?, oid(theirs)?], message)
}

fn squash_commit(git_dir: &Path, base: &str, head: &str, message: &str) -> Result<String, GitFail> {
    let repo = open(git_dir)?;
    let tree = merged_commits(&repo, base, head)?;
    write_commit(&repo, tree, &[oid(base)?], message)
}

fn rebase(git_dir: &Path, base: &str, head: &str) -> Result<String, GitFail> {
    let commits = git::new_commits(git_dir, head, &[base]).map_err(other)?;
    let repo = open(git_dir)?;
    let mut tip = base.to_owned();
    for sha in commits {
        let commit = git::commit(git_dir, &sha).map_err(other)?;
        let ancestor = match commit.parents.first() {
            Some(parent) => tree_id(&repo, parent)?,
            None => gix::ObjectId::empty_tree(gix::hash::Kind::Sha1),
        };
        let tree = merged_trees(
            &repo,
            ancestor,
            tree_id(&repo, &tip)?,
            tree_id(&repo, &sha)?,
        )?;
        let text = format!("{}\n", commit.message);
        tip = write_commit(&repo, tree, &[oid(&tip)?], &text)?;
    }
    Ok(tip)
}

fn merged_commits(
    repo: &gix::Repository,
    ours: &str,
    theirs: &str,
) -> Result<gix::ObjectId, GitFail> {
    let mut outcome = repo
        .merge_commits(oid(ours)?, oid(theirs)?, labels(), commit_options(repo)?)
        .map_err(|_| GitFail::Conflict)?;
    if unresolved(&outcome.tree_merge) {
        return Err(GitFail::Conflict);
    }
    outcome
        .tree_merge
        .tree
        .write()
        .map(|id| id.detach())
        .map_err(other)
}

fn merged_trees(
    repo: &gix::Repository,
    ancestor: gix::ObjectId,
    ours: gix::ObjectId,
    theirs: gix::ObjectId,
) -> Result<gix::ObjectId, GitFail> {
    let mut outcome = repo
        .merge_trees(ancestor, ours, theirs, labels(), tree_options(repo)?)
        .map_err(|_| GitFail::Conflict)?;
    if unresolved(&outcome) {
        return Err(GitFail::Conflict);
    }
    outcome.tree.write().map(|id| id.detach()).map_err(other)
}

fn unresolved(outcome: &gix::merge::tree::Outcome<'_>) -> bool {
    outcome.failed_on_first_unresolved_conflict
        || outcome.has_unresolved_conflicts(gix::merge::tree::TreatAsUnresolved::default())
}

fn write_commit(
    repo: &gix::Repository,
    tree: gix::ObjectId,
    parents: &[gix::ObjectId],
    message: &str,
) -> Result<String, GitFail> {
    let signature = gix::actor::Signature {
        name: BOT.name.into(),
        email: BOT.email.into(),
        time: gix::date::Time::now_local_or_utc(),
    };
    let commit = gix::objs::Commit {
        tree,
        parents: parents.to_vec().into(),
        author: signature.clone(),
        committer: signature,
        encoding: None,
        message: message.into(),
        extra_headers: Vec::new(),
    };
    repo.write_object(&commit)
        .map(|id| id.detach().to_string())
        .map_err(other)
}

fn tree_id(repo: &gix::Repository, sha: &str) -> Result<gix::ObjectId, GitFail> {
    let commit = repo.find_commit(oid(sha)?).map_err(other)?;
    Ok(commit.tree_id().map_err(other)?.detach())
}

fn tip(git_dir: &Path, reference: &str) -> Result<Option<String>, GitFail> {
    git::resolve_commit(git_dir, reference).map_err(other)
}

fn open(git_dir: &Path) -> Result<gix::Repository, GitFail> {
    gix::open_opts(git_dir, gix::open::Options::isolated()).map_err(other)
}

fn oid(sha: &str) -> Result<gix::ObjectId, GitFail> {
    gix::ObjectId::from_hex(sha.as_bytes()).map_err(|_| GitFail::Missing("commit does not exist"))
}

fn labels<'a>() -> gix::merge::blob::builtin_driver::text::Labels<'a> {
    gix::merge::blob::builtin_driver::text::Labels {
        ancestor: Some("base".as_bytes().as_bstr()),
        current: Some("ours".as_bytes().as_bstr()),
        other: Some("theirs".as_bytes().as_bstr()),
    }
}

fn commit_options(repo: &gix::Repository) -> Result<gix::merge::commit::Options, GitFail> {
    Ok(repo.tree_merge_options().map_err(other)?.into())
}

fn tree_options(repo: &gix::Repository) -> Result<gix::merge::tree::Options, GitFail> {
    repo.tree_merge_options().map_err(other)
}

fn message(headline: &str, body: &str) -> String {
    let headline = headline.trim();
    let body = body.trim();
    if body.is_empty() {
        format!("{headline}\n")
    } else {
        format!("{headline}\n\n{body}\n")
    }
}

fn other(error: impl std::fmt::Display) -> GitFail {
    GitFail::Message(error.to_string())
}
