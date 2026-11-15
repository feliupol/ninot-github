//! The `pull_request` event.

use serde::Serialize;
use serde_json::{Map, Value};

use super::{Ctx, PayloadError, model};
use crate::types;
use crate::webhook_types as wt;

/// Queues `pull_request` `opened` for a pull request that was just created, and so was never
/// closed, merged or locked.
pub(crate) fn opened(ctx: &mut Ctx, pull: &types::PullRequest) -> Result<(), PayloadError> {
    deliver(
        ctx,
        pull,
        None,
        |number, mut pull_request, repository, sender| {
            pull_request.closed_at = None;
            pull_request.merged_at = None;
            pull_request.active_lock_reason = None;
            pull_request.merged_by = None;
            wt::PullRequestOpened {
                action: wt::PullRequestOpenedAction::Opened,
                installation: None,
                number,
                organization: None,
                pull_request,
                repository,
                sender,
            }
        },
    )
}

/// Queues `pull_request` `edited` describing `changes` (`title`, `body`, and/or `base`, each as
/// GitHub sends it, such as `{"title": {"from": "Old"}}`).
pub(crate) fn edited(
    ctx: &mut Ctx,
    pull: &types::PullRequest,
    changes: Map<String, Value>,
) -> Result<(), PayloadError> {
    if changes.is_empty() {
        return Ok(());
    }
    let changes = Value::Object(changes);
    let changes: Result<wt::PullRequestEditedChanges, _> = serde_json::from_value(changes.clone())
        .map_err(|_| PayloadError::Unknown {
            field: "changes",
            value: changes.to_string(),
        });
    try_deliver(
        ctx,
        pull,
        None,
        |number, pull_request, repository, sender| {
            Ok(wt::PullRequestEdited {
                action: wt::PullRequestEditedAction::Edited,
                changes: changes?,
                installation: None,
                number,
                organization: None,
                pull_request,
                repository,
                sender,
            })
        },
    )
}

/// Queues `pull_request` `closed`. A merged pull request keeps `action` `closed`.
pub(crate) fn closed(ctx: &mut Ctx, pull: &types::PullRequest) -> Result<(), PayloadError> {
    deliver(
        ctx,
        pull,
        None,
        |number, pull_request, repository, sender| wt::PullRequestClosed {
            action: wt::PullRequestClosedAction::Closed,
            installation: None,
            number,
            organization: None,
            pull_request,
            repository,
            sender,
        },
    )
}

/// Queues `pull_request` `reopened`. A reopened pull request is neither closed nor merged.
pub(crate) fn reopened(ctx: &mut Ctx, pull: &types::PullRequest) -> Result<(), PayloadError> {
    deliver(
        ctx,
        pull,
        None,
        |number, mut pull_request, repository, sender| {
            pull_request.closed_at = None;
            pull_request.merged_at = None;
            pull_request.merged_by = None;
            wt::PullRequestReopened {
                action: wt::PullRequestReopenedAction::Reopened,
                installation: None,
                number,
                organization: None,
                pull_request,
                repository,
                sender,
            }
        },
    )
}

/// Queues `pull_request` `synchronize` for a head that moved from `before` to `after`.
pub(crate) fn synchronize(
    ctx: &mut Ctx,
    pull: &types::PullRequest,
    before: &str,
    after: &str,
    sender: &types::SimpleUser,
) -> Result<(), PayloadError> {
    deliver(
        ctx,
        pull,
        Some(sender),
        |number, pull_request, repository, sender| wt::PullRequestSynchronize {
            action: wt::PullRequestSynchronizeAction::Synchronize,
            after: after.to_owned(),
            before: before.to_owned(),
            installation: None,
            number,
            organization: None,
            pull_request,
            repository,
            sender,
        },
    )
}

/// Queues one `pull_request` `labeled` event for `label`.
pub(crate) fn labeled(
    ctx: &mut Ctx,
    pull: &types::PullRequest,
    label: &types::Label,
) -> Result<(), PayloadError> {
    deliver(
        ctx,
        pull,
        None,
        |number, pull_request, repository, sender| wt::PullRequestLabeled {
            action: wt::PullRequestLabeledAction::Labeled,
            installation: None,
            label: model::label(label),
            number,
            organization: None,
            pull_request,
            repository,
            sender,
        },
    )
}

/// Queues `pull_request` `review_requested` for one user.
pub(crate) fn review_requested_user(
    ctx: &mut Ctx,
    pull: &types::PullRequest,
    reviewer: &types::SimpleUser,
) -> Result<(), PayloadError> {
    deliver(
        ctx,
        pull,
        None,
        |number, pull_request, repository, sender| wt::PullRequestReviewRequested::Variant0 {
            action: wt::PullRequestReviewRequestedVariant0Action::ReviewRequested,
            installation: None,
            number,
            organization: None,
            pull_request,
            repository,
            requested_reviewer: model::user(reviewer),
            sender,
        },
    )
}

/// Queues `pull_request` `review_requested` for one team.
pub(crate) fn review_requested_team(
    ctx: &mut Ctx,
    pull: &types::PullRequest,
    team: &types::TeamSimple,
) -> Result<(), PayloadError> {
    try_deliver(
        ctx,
        pull,
        None,
        |number, pull_request, repository, sender| {
            Ok(wt::PullRequestReviewRequested::Variant1 {
                action: wt::PullRequestReviewRequestedVariant1Action::ReviewRequested,
                installation: None,
                number,
                organization: None,
                pull_request,
                repository,
                requested_team: model::team(team)?,
                sender,
            })
        },
    )
}

/// Labels on `pull` as REST labels.
pub(crate) fn pull_labels(pull: &types::PullRequest) -> Vec<types::Label> {
    pull.labels
        .iter()
        .map(|label| types::Label {
            archived_at: None,
            archived_by: label.archived_by.clone(),
            color: label.color.clone(),
            default: label.default,
            description: label.description.clone(),
            id: label.id,
            name: label.name.clone(),
            node_id: label.node_id.clone(),
            url: label.url.clone(),
        })
        .collect()
}

/// Builds the payload from the pull request number, webhook pull request, repository and
/// sender (`sender`, or the caller), and queues it.
fn deliver<P: Serialize>(
    ctx: &mut Ctx,
    pull: &types::PullRequest,
    sender: Option<&types::SimpleUser>,
    payload: impl FnOnce(i64, wt::PullRequest, wt::Repository, wt::User) -> P,
) -> Result<(), PayloadError> {
    try_deliver(
        ctx,
        pull,
        sender,
        |number, pull_request, repository, sender| {
            Ok(payload(number, pull_request, repository, sender))
        },
    )
}

/// [`deliver`] for a payload whose own parts can fail to build.
fn try_deliver<P: Serialize>(
    ctx: &mut Ctx,
    pull: &types::PullRequest,
    sender: Option<&types::SimpleUser>,
    payload: impl FnOnce(i64, wt::PullRequest, wt::Repository, wt::User) -> Result<P, PayloadError>,
) -> Result<(), PayloadError> {
    if !ctx.wants("pull_request") {
        return Ok(());
    }
    let (Some(repository), Some(sender)) = (ctx.repository(), ctx.sender(sender)) else {
        return Ok(());
    };
    let payload = model::pull_request(pull, &repository)
        .and_then(|pull_request| payload(pull.number, pull_request, repository, sender));
    ctx.enqueue_built("pull_request", payload)
}
