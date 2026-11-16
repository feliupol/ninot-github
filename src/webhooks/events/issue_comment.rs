//! The `issue_comment` event.

use serde::Serialize;

use super::{Ctx, PayloadError, model};
use crate::types;
use crate::urls::RepoUrls;
use crate::webhook_types as wt;

/// Queues `issue_comment` `created` for a comment on issue or pull request `number`.
pub(crate) fn created(
    ctx: &mut Ctx,
    number: u64,
    comment: &types::IssueComment,
) -> Result<(), PayloadError> {
    deliver(
        ctx,
        number,
        comment,
        |comment, issue, repository, sender| wt::IssueCommentCreated {
            action: wt::IssueCommentCreatedAction::Created,
            comment,
            installation: None,
            issue,
            organization: None,
            repository,
            sender,
        },
    )
}

/// Queues `issue_comment` `edited` with the previous body.
pub(crate) fn edited(
    ctx: &mut Ctx,
    number: u64,
    comment: &types::IssueComment,
    from: &str,
) -> Result<(), PayloadError> {
    deliver(
        ctx,
        number,
        comment,
        |comment, issue, repository, sender| wt::IssueCommentEdited {
            action: wt::IssueCommentEditedAction::Edited,
            changes: wt::IssueCommentEditedChanges {
                body: Some(wt::IssueCommentEditedChangesBody {
                    from: from.to_owned(),
                }),
            },
            comment,
            installation: None,
            issue,
            organization: None,
            repository,
            sender,
        },
    )
}

/// Queues `issue_comment` `deleted` for the comment that was just removed.
pub(crate) fn deleted(
    ctx: &mut Ctx,
    number: u64,
    comment: &types::IssueComment,
) -> Result<(), PayloadError> {
    deliver(
        ctx,
        number,
        comment,
        |comment, issue, repository, sender| wt::IssueCommentDeleted {
            action: wt::IssueCommentDeletedAction::Deleted,
            comment,
            installation: None,
            issue,
            organization: None,
            repository,
            sender,
        },
    )
}

/// Builds the payload from the webhook comment, issue, repository and sender, and queues it.
fn deliver<P: Serialize>(
    ctx: &mut Ctx,
    number: u64,
    comment: &types::IssueComment,
    payload: impl FnOnce(wt::IssueComment, wt::Issue, wt::Repository, wt::User) -> P,
) -> Result<(), PayloadError> {
    if !ctx.wants("issue_comment") {
        return Ok(());
    }
    let (Some(repository), Some(sender)) = (ctx.repository(), ctx.sender(None)) else {
        return Ok(());
    };
    let Some(issue) = issue(ctx, number) else {
        return Ok(());
    };
    let payload = issue.and_then(|issue| {
        let comment = issue_comment(comment, &sender)?;
        Ok(payload(comment, issue, repository, sender))
    });
    ctx.enqueue_built("issue_comment", payload)
}

/// The issue or pull request `number` as a webhook issue, when it exists.
fn issue(ctx: &Ctx, number: u64) -> Option<Result<wt::Issue, PayloadError>> {
    let account = ctx.state.account(ctx.owner)?;
    let repo = account.repo(ctx.name)?;
    if let Some(issue) = repo.issues.get(&number) {
        let context = crate::render::IssueContext {
            state: ctx.state,
            request: ctx.request,
            owner: &account.login,
            repo,
        };
        return Some(model::issue(&crate::render::issue(context, issue)));
    }
    let pull = repo.pull_requests.get(&number)?.data.as_ref()?;
    Some(issue_from_pull(pull, &ctx.urls()))
}

/// The issue a pull request is, as GitHub sends it with a comment on the pull request.
fn issue_from_pull(pull: &types::PullRequest, repo: &RepoUrls) -> Result<wt::Issue, PayloadError> {
    let issue = repo.issue(pull.number.unsigned_abs());
    Ok(wt::Issue {
        active_lock_reason: pull
            .active_lock_reason
            .as_deref()
            .map(|reason| model::parse("pull_request.active_lock_reason", reason))
            .transpose()?,
        assignee: pull.assignee.as_ref().map(model::nullable_user),
        assignees: pull.assignees.iter().map(model::user).collect(),
        author_association: model::author_association(&pull.author_association)?,
        body: pull.body.clone(),
        closed_at: pull.closed_at,
        comments: pull.comments,
        comments_url: pull.comments_url.clone(),
        created_at: pull.created_at,
        draft: None,
        events_url: issue.events(),
        html_url: pull.html_url.clone(),
        id: pull.id,
        labels: pull
            .labels
            .iter()
            .map(|label| wt::Label {
                color: label.color.clone(),
                default: label.default,
                description: label.description.clone(),
                id: label.id,
                name: label.name.clone(),
                node_id: label.node_id.clone(),
                url: label.url.clone(),
            })
            .collect(),
        labels_url: issue.labels(),
        locked: pull.locked,
        milestone: None,
        node_id: pull.node_id.clone(),
        number: pull.number,
        performed_via_github_app: None,
        pull_request: Some(wt::IssuePullRequest {
            diff_url: Some(pull.diff_url.clone()),
            html_url: Some(pull.html_url.clone()),
            merged_at: pull.merged_at,
            patch_url: Some(pull.patch_url.clone()),
            url: Some(pull.url.clone()),
        }),
        reactions: model::reactions(None, &pull.issue_url),
        repository_url: repo.api.clone(),
        state: model::parse("pull_request.state", &pull.state.to_string())?,
        state_reason: None,
        timeline_url: None,
        title: pull.title.clone(),
        updated_at: pull.updated_at,
        url: pull.issue_url.clone(),
        user: model::user(&pull.user),
    })
}

/// The webhook comment for `comment`, written by `sender` when it has no author.
fn issue_comment(
    comment: &types::IssueComment,
    sender: &wt::User,
) -> Result<wt::IssueComment, PayloadError> {
    Ok(wt::IssueComment {
        author_association: comment
            .author_association
            .as_ref()
            .map_or(Ok(wt::AuthorAssociation::None), model::author_association)?,
        body: comment.body.clone().unwrap_or_default(),
        created_at: comment.created_at,
        html_url: comment.html_url.clone(),
        id: comment.id,
        issue_url: comment.issue_url.clone(),
        node_id: comment.node_id.clone(),
        performed_via_github_app: None,
        reactions: model::reactions(comment.reactions.as_ref(), &comment.url),
        updated_at: comment.updated_at,
        url: comment.url.clone(),
        user: comment
            .user
            .as_ref()
            .map_or_else(|| sender.clone(), model::nullable_user),
    })
}
