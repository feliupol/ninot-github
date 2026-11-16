//! The `status` event, sent when a commit status is created.

use super::{Ctx, PayloadError, model};
use crate::types;
use crate::urls::RepoUrls;
use crate::webhook_types as wt;

/// Queues `status` for a commit status on `sha`.
pub(crate) fn commit_status(
    ctx: &mut Ctx,
    sha: &str,
    status: &types::Status,
) -> Result<(), PayloadError> {
    if !ctx.wants("status") {
        return Ok(());
    }
    let Some((repository, sender)) = ctx.parties("status", None)? else {
        return Ok(());
    };
    let urls = ctx.urls();
    let payload = status_event(ctx, sha, status, &urls, repository, sender);
    ctx.enqueue_built("status", payload)
}

/// The `status` payload for `status` on `sha`.
fn status_event(
    ctx: &Ctx,
    sha: &str,
    status: &types::Status,
    urls: &RepoUrls,
    repository: wt::Repository,
    sender: wt::User,
) -> Result<wt::StatusEvent, PayloadError> {
    Ok(wt::StatusEvent {
        avatar_url: None,
        branches: branches(ctx, sha, urls),
        commit: commit(ctx, sha, urls),
        context: status.context.clone(),
        created_at: model::timestamp("status.created_at", &status.created_at)?,
        description: status.description.clone(),
        id: status.id,
        installation: None,
        name: status.context.clone(),
        organization: None,
        repository,
        sender,
        sha: sha.to_owned(),
        state: model::parse("status.state", &status.state.to_ascii_lowercase())?,
        target_url: status.target_url.clone(),
        updated_at: model::timestamp("status.updated_at", &status.updated_at)?,
    })
}

/// The commit `sha`, from git when the repository has it.
fn commit(ctx: &Ctx, sha: &str, repo: &RepoUrls) -> wt::StatusEventCommit {
    let urls = repo.commit(sha);
    let commit = ctx
        .git_dir()
        .and_then(|dir| ninot_common::git::commit(&dir, sha).ok());
    let (tree, message, author, committer, parents) = match commit {
        Some(commit) => (
            commit.tree,
            commit.message,
            model::committer(&commit.author),
            model::committer(&commit.committer),
            commit.parents,
        ),
        None => (
            sha.to_owned(),
            String::new(),
            model::stub_committer(),
            model::stub_committer(),
            Vec::new(),
        ),
    };
    wt::StatusEventCommit {
        author: None,
        comments_url: urls.comments(),
        commit: wt::StatusEventCommitCommit {
            author: wt::StatusEventCommitCommitAuthor {
                date: author.date.unwrap_or_else(crate::time::now),
                email: author.email,
                name: author.name,
                username: None,
            },
            comment_count: 0,
            committer: wt::StatusEventCommitCommitCommitter {
                date: committer.date.unwrap_or_else(crate::time::now),
                email: committer.email,
                name: committer.name,
                username: None,
            },
            message,
            tree: wt::StatusEventCommitCommitTree {
                url: repo.git_tree(&tree),
                sha: tree,
            },
            url: urls.git.clone(),
            verification: wt::StatusEventCommitCommitVerification {
                payload: None,
                reason: wt::StatusEventCommitCommitVerificationReason::Unsigned,
                signature: None,
                verified: false,
            },
        },
        committer: None,
        html_url: urls.html.clone(),
        node_id: format!("C_{sha}"),
        parents: parents
            .iter()
            .map(|parent| {
                let urls = repo.commit(parent);
                wt::StatusEventCommitParentsItem {
                    html_url: urls.html,
                    sha: parent.clone(),
                    url: urls.api,
                }
            })
            .collect(),
        sha: sha.to_owned(),
        url: urls.api.clone(),
    }
}

/// Up to ten branches whose head is `sha`.
fn branches(ctx: &Ctx, sha: &str, repo: &RepoUrls) -> Vec<wt::StatusEventBranchesItem> {
    let Some(dir) = ctx.git_dir() else {
        return Vec::new();
    };
    let Ok(refs) = ninot_common::git::refs(&dir, "refs/heads/") else {
        return Vec::new();
    };
    refs.into_iter()
        .filter(|reference| reference.sha == sha)
        .take(10)
        .map(|reference| wt::StatusEventBranchesItem {
            commit: wt::StatusEventBranchesItemCommit {
                url: repo.commit(&reference.sha).api,
                sha: reference.sha,
            },
            name: reference
                .name
                .strip_prefix("refs/heads/")
                .unwrap_or(&reference.name)
                .to_owned(),
            protected: false,
        })
        .collect()
}
