//! The `pull_request_review_comment` event.

use super::{Ctx, PayloadError, model};
use crate::types;
use crate::webhook_types as wt;

/// Queues `pull_request_review_comment` `created`.
pub(crate) fn created(
    ctx: &mut Ctx,
    pull: &types::PullRequest,
    comment: &types::PullRequestReviewComment,
) -> Result<(), PayloadError> {
    if !ctx.wants("pull_request_review_comment") {
        return Ok(());
    }
    let (Some(repository), Some(sender)) = (ctx.repository(), ctx.sender(None)) else {
        return Ok(());
    };
    let payload = created_payload(pull, comment, repository, sender);
    ctx.enqueue_built("pull_request_review_comment", payload)
}

/// The `created` payload for `comment` on `pull`.
fn created_payload(
    pull: &types::PullRequest,
    comment: &types::PullRequestReviewComment,
    repository: wt::Repository,
    sender: wt::User,
) -> Result<wt::PullRequestReviewCommentCreated, PayloadError> {
    Ok(wt::PullRequestReviewCommentCreated {
        action: wt::PullRequestReviewCommentCreatedAction::Created,
        comment: review_comment(comment, &sender)?,
        installation: None,
        organization: None,
        pull_request: model::pull_request_object!(
            PullRequestReviewCommentCreatedPullRequest,
            PullRequestReviewCommentCreatedPullRequestBase,
            PullRequestReviewCommentCreatedPullRequestHead,
            PullRequestReviewCommentCreatedPullRequestLinks,
            pull,
            &repository,
            { draft: Some(pull.draft.unwrap_or(false)), }
        ),
        repository,
        sender,
    })
}

/// The webhook review comment for `comment`, by `sender` when it has no author. A comment
/// without an original position or line is on the first line.
fn review_comment(
    comment: &types::PullRequestReviewComment,
    sender: &wt::User,
) -> Result<wt::PullRequestReviewComment, PayloadError> {
    let links = &comment.links;
    Ok(wt::PullRequestReviewComment {
        author_association: model::author_association(&comment.author_association)?,
        body: comment.body.clone(),
        commit_id: comment.commit_id.clone(),
        created_at: comment.created_at,
        diff_hunk: comment.diff_hunk.clone(),
        html_url: comment.html_url.clone(),
        id: comment.id,
        in_reply_to_id: comment.in_reply_to_id,
        line: comment.line,
        links: wt::PullRequestReviewCommentLinks {
            html: wt::Link {
                href: links.html.href.clone(),
            },
            pull_request: wt::Link {
                href: links.pull_request.href.clone(),
            },
            self_: wt::Link {
                href: links.self_.href.clone(),
            },
        },
        node_id: comment.node_id.clone(),
        original_commit_id: comment.original_commit_id.clone(),
        original_line: comment.original_line.unwrap_or(1),
        original_position: comment.original_position.unwrap_or(1),
        original_start_line: comment.original_start_line,
        path: comment.path.clone(),
        position: comment.position,
        pull_request_review_id: comment.pull_request_review_id.unwrap_or(0),
        pull_request_url: comment.pull_request_url.clone(),
        reactions: model::reactions(comment.reactions.as_ref(), &comment.url),
        side: model::parse("comment.side", &comment.side.to_string())?,
        start_line: comment.start_line,
        start_side: comment
            .start_side
            .as_ref()
            .map(|side| model::parse("comment.start_side", &side.to_string()))
            .transpose()?,
        subject_type: comment
            .subject_type
            .as_ref()
            .map(|subject| model::parse("comment.subject_type", &subject.to_string()))
            .transpose()?,
        updated_at: comment.updated_at,
        url: comment.url.clone(),
        user: comment
            .user
            .as_ref()
            .map_or_else(|| sender.clone(), model::nullable_user),
    })
}
