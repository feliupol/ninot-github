//! The `issues` event.

use serde::Serialize;

use super::{Ctx, PayloadError, model};
use crate::types;
use crate::webhook_types as wt;

/// Queues `issues` `opened`. An opened issue was never closed.
pub(crate) fn opened(ctx: &mut Ctx, issue: &types::Issue) -> Result<(), PayloadError> {
    deliver(ctx, issue, |mut issue, repository, sender| {
        issue.closed_at = None;
        wt::IssuesOpened {
            action: wt::IssuesOpenedAction::Opened,
            changes: None,
            installation: None,
            issue,
            organization: None,
            repository,
            sender,
        }
    })
}

/// Queues one `issues` `labeled` event for `label`. Pull requests use `pull_request` instead.
pub(crate) fn labeled(
    ctx: &mut Ctx,
    issue: &types::Issue,
    label: &types::Label,
) -> Result<(), PayloadError> {
    deliver(ctx, issue, |issue, repository, sender| wt::IssuesLabeled {
        action: wt::IssuesLabeledAction::Labeled,
        installation: None,
        issue,
        label: Some(model::label(label)),
        organization: None,
        repository,
        sender,
    })
}

/// Queues one `issues` `unlabeled` event for `label`.
pub(crate) fn unlabeled(
    ctx: &mut Ctx,
    issue: &types::Issue,
    label: &types::Label,
) -> Result<(), PayloadError> {
    deliver(ctx, issue, |issue, repository, sender| {
        wt::IssuesUnlabeled {
            action: wt::IssuesUnlabeledAction::Unlabeled,
            installation: None,
            issue,
            label: Some(model::label(label)),
            organization: None,
            repository,
            sender,
        }
    })
}

/// Queues `issues` `edited` describing the previous title and/or body.
pub(crate) fn edited(
    ctx: &mut Ctx,
    issue: &types::Issue,
    title_from: Option<&str>,
    body_from: Option<&str>,
) -> Result<(), PayloadError> {
    if title_from.is_none() && body_from.is_none() {
        return Ok(());
    }
    let changes = wt::IssuesEditedChanges {
        body: body_from.map(|from| wt::IssuesEditedChangesBody {
            from: from.to_owned(),
        }),
        title: title_from.map(|from| wt::IssuesEditedChangesTitle {
            from: from.to_owned(),
        }),
    };
    deliver(ctx, issue, |issue, repository, sender| wt::IssuesEdited {
        action: wt::IssuesEditedAction::Edited,
        changes,
        installation: None,
        issue,
        label: None,
        organization: None,
        repository,
        sender,
    })
}

/// Queues `issues` `closed`.
pub(crate) fn closed(ctx: &mut Ctx, issue: &types::Issue) -> Result<(), PayloadError> {
    deliver(ctx, issue, |issue, repository, sender| wt::IssuesClosed {
        action: wt::IssuesClosedAction::Closed,
        installation: None,
        issue,
        organization: None,
        repository,
        sender,
    })
}

/// Queues `issues` `reopened`.
pub(crate) fn reopened(ctx: &mut Ctx, issue: &types::Issue) -> Result<(), PayloadError> {
    deliver(ctx, issue, |issue, repository, sender| wt::IssuesReopened {
        action: wt::IssuesReopenedAction::Reopened,
        installation: None,
        issue,
        organization: None,
        repository,
        sender,
    })
}

/// Queues `issues` `assigned` for `assignee`.
pub(crate) fn assigned(
    ctx: &mut Ctx,
    issue: &types::Issue,
    assignee: &types::SimpleUser,
) -> Result<(), PayloadError> {
    deliver(ctx, issue, |issue, repository, sender| wt::IssuesAssigned {
        action: wt::IssuesAssignedAction::Assigned,
        assignee: Some(model::user(assignee)),
        installation: None,
        issue,
        organization: None,
        repository,
        sender,
    })
}

/// Queues `issues` `unassigned` for `assignee`.
pub(crate) fn unassigned(
    ctx: &mut Ctx,
    issue: &types::Issue,
    assignee: &types::SimpleUser,
) -> Result<(), PayloadError> {
    deliver(ctx, issue, |issue, repository, sender| {
        wt::IssuesUnassigned {
            action: wt::IssuesUnassignedAction::Unassigned,
            assignee: Some(model::user(assignee)),
            installation: None,
            issue,
            organization: None,
            repository,
            sender,
        }
    })
}

/// Queues `issues` `locked`.
pub(crate) fn locked(ctx: &mut Ctx, issue: &types::Issue) -> Result<(), PayloadError> {
    deliver(ctx, issue, |issue, repository, sender| wt::IssuesLocked {
        action: wt::IssuesLockedAction::Locked,
        installation: None,
        issue,
        organization: None,
        repository,
        sender,
    })
}

/// Queues `issues` `unlocked`. An unlocked issue has no lock reason.
pub(crate) fn unlocked(ctx: &mut Ctx, issue: &types::Issue) -> Result<(), PayloadError> {
    deliver(ctx, issue, |mut issue, repository, sender| {
        issue.active_lock_reason = None;
        wt::IssuesUnlocked {
            action: wt::IssuesUnlockedAction::Unlocked,
            installation: None,
            issue,
            organization: None,
            repository,
            sender,
        }
    })
}

/// Queues `issues` `milestoned` for `milestone`.
pub(crate) fn milestoned(
    ctx: &mut Ctx,
    issue: &types::Issue,
    milestone: &types::NullableMilestone,
) -> Result<(), PayloadError> {
    try_deliver(ctx, issue, |issue, repository, sender| {
        Ok(wt::IssuesMilestoned {
            action: wt::IssuesMilestonedAction::Milestoned,
            installation: None,
            issue,
            milestone: model::milestone(milestone)?,
            organization: None,
            repository,
            sender,
        })
    })
}

/// Queues `issues` `demilestoned` for the removed `milestone`. The issue has no milestone.
pub(crate) fn demilestoned(
    ctx: &mut Ctx,
    issue: &types::Issue,
    milestone: &types::NullableMilestone,
) -> Result<(), PayloadError> {
    try_deliver(ctx, issue, |mut issue, repository, sender| {
        issue.milestone = None;
        Ok(wt::IssuesDemilestoned {
            action: wt::IssuesDemilestonedAction::Demilestoned,
            installation: None,
            issue,
            milestone: model::milestone(milestone)?,
            organization: None,
            repository,
            sender,
        })
    })
}

/// Labels on `issue` as REST labels.
pub(crate) fn issue_labels(issue: &types::Issue) -> Vec<types::Label> {
    model::issue_labels(issue)
}

/// Builds the payload from the webhook issue, repository and sender, and queues it.
fn deliver<P: Serialize>(
    ctx: &mut Ctx,
    issue: &types::Issue,
    payload: impl FnOnce(wt::Issue, wt::Repository, wt::User) -> P,
) -> Result<(), PayloadError> {
    try_deliver(ctx, issue, |issue, repository, sender| {
        Ok(payload(issue, repository, sender))
    })
}

/// [`deliver`] for a payload whose own parts can fail to build.
fn try_deliver<P: Serialize>(
    ctx: &mut Ctx,
    issue: &types::Issue,
    payload: impl FnOnce(wt::Issue, wt::Repository, wt::User) -> Result<P, PayloadError>,
) -> Result<(), PayloadError> {
    if !ctx.wants("issues") {
        return Ok(());
    }
    let (Some(repository), Some(sender)) = (ctx.repository(), ctx.sender(None)) else {
        return Ok(());
    };
    let payload = model::issue(issue).and_then(|issue| payload(issue, repository, sender));
    ctx.enqueue_built("issues", payload)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::ApiRequest;
    use crate::fixtures::IssueSeed;
    use crate::render;
    use crate::state::{Hook, State};
    use crate::validate::check_webhook;

    #[test]
    fn update_actions_match_the_schema() {
        let mut state = State::default();
        state.add_user("admin");
        state.create_repo("admin", "tool", false, None).unwrap();
        let now = crate::state::now();
        state
            .account_mut("admin")
            .unwrap()
            .repo_mut("tool")
            .unwrap()
            .hooks
            .push(Hook {
                id: 1,
                active: true,
                events: vec!["*".to_owned()],
                url: "http://127.0.0.1:1/hook".to_owned(),
                content_type: "json".to_owned(),
                secret: None,
                insecure_ssl: "0".to_owned(),
                created_at: now.clone(),
                updated_at: now,
            });
        let request = ApiRequest::for_urls("http://127.0.0.1:1", None);
        let user = {
            let account = state.account("admin").unwrap();
            render::simple_user(&request, account)
        };
        let issue = crate::fixtures::seeded_issue(
            &mut state,
            &request,
            "admin",
            "tool",
            user.clone(),
            1,
            IssueSeed::default(),
        )
        .unwrap();
        state.add_issue("admin", "tool", 1, issue.clone());
        let mut ctx = Ctx::new(&mut state, &request, "admin", "tool");
        edited(&mut ctx, &issue, Some("Earlier"), Some("")).unwrap();
        let mut closed_issue = issue.clone();
        closed_issue.state = "closed".to_owned();
        closed_issue.closed_at = Some(crate::time::now_utc());
        closed(&mut ctx, &closed_issue).unwrap();
        reopened(&mut ctx, &issue).unwrap();
        assigned(&mut ctx, &issue, &user).unwrap();
        unassigned(&mut ctx, &issue, &user).unwrap();
        let labels = issue_labels(&issue);
        if let Some(label) = labels.first() {
            unlabeled(&mut ctx, &issue, label).unwrap();
        }
        let mut locked_issue = issue.clone();
        locked_issue.locked = true;
        locked(&mut ctx, &locked_issue).unwrap();
        unlocked(&mut ctx, &issue).unwrap();
        for delivery in &ctx.state.deliveries {
            check_webhook("issues", &delivery.payload()).unwrap_or_else(|error| {
                panic!("{} {}\n{error}", delivery.event, delivery.payload());
            });
        }
    }
}
