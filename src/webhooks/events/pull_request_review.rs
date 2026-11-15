//! The `pull_request_review` event.

use super::{Ctx, PayloadError, model};
use crate::types;
use crate::webhook_types as wt;

/// Queues `pull_request_review` `submitted` for a review that is not pending.
///
/// GitHub does not deliver a review while it is still `PENDING`.
pub(crate) fn submitted(
    ctx: &mut Ctx,
    pull: &types::PullRequest,
    review: &types::PullRequestReview,
) -> Result<(), PayloadError> {
    if review.state.eq_ignore_ascii_case("pending") {
        return Ok(());
    }
    if !ctx.wants("pull_request_review") {
        return Ok(());
    }
    let (Some(repository), Some(sender)) = (ctx.repository(), ctx.sender(None)) else {
        return Ok(());
    };
    let payload = submitted_payload(pull, review, repository, sender);
    ctx.enqueue_built("pull_request_review", payload)
}

/// The `submitted` payload for `review` of `pull`.
fn submitted_payload(
    pull: &types::PullRequest,
    review: &types::PullRequestReview,
    repository: wt::Repository,
    sender: wt::User,
) -> Result<wt::PullRequestReviewSubmitted, PayloadError> {
    Ok(wt::PullRequestReviewSubmitted {
        action: wt::PullRequestReviewSubmittedAction::Submitted,
        installation: None,
        organization: None,
        pull_request: simple_pull_request(pull, &repository)?,
        review: pull_review(review, &pull.head.sha, &sender)?,
        repository,
        sender,
    })
}

/// The pull request a review belongs to, whose head and base are in `repository`.
fn simple_pull_request(
    pull: &types::PullRequest,
    repository: &wt::Repository,
) -> Result<wt::SimplePullRequest, PayloadError> {
    Ok(model::pull_request_object!(
        SimplePullRequest,
        SimplePullRequestBase,
        SimplePullRequestHead,
        SimplePullRequestLinks,
        pull,
        repository,
        { draft: pull.draft.unwrap_or(false), }
    ))
}

/// The webhook review for `review` of the commit `head`, by `sender` when it has no author.
fn pull_review(
    review: &types::PullRequestReview,
    head: &str,
    sender: &wt::User,
) -> Result<wt::PullRequestReview, PayloadError> {
    Ok(wt::PullRequestReview {
        author_association: model::author_association(&review.author_association)?,
        body: Some(review.body.clone()),
        commit_id: review
            .commit_id
            .clone()
            .filter(|commit| !commit.is_empty())
            .unwrap_or_else(|| head.to_owned()),
        html_url: review.html_url.clone(),
        id: review.id,
        links: wt::PullRequestReviewLinks {
            html: wt::Link {
                href: review.links.html.href.clone(),
            },
            pull_request: wt::Link {
                href: review.links.pull_request.href.clone(),
            },
        },
        node_id: review.node_id.clone(),
        pull_request_url: review.pull_request_url.clone(),
        state: model::parse("review.state", &review.state.to_ascii_lowercase())?,
        submitted_at: review.submitted_at,
        user: review
            .user
            .as_ref()
            .map_or_else(|| sender.clone(), model::nullable_user),
    })
}
