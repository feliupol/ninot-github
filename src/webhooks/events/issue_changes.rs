//! Diffs two issue snapshots and queues the `issues` events GitHub sends for that change.

use super::Ctx;
use super::issues;
use crate::types;

/// Queues `issues` events for the fields that changed between `before` and `after`.
pub(crate) fn from_update(ctx: &mut Ctx, before: &types::Issue, after: &types::Issue) {
    let title_from = (before.title != after.title).then_some(before.title.as_str());
    let before_body = before.body.as_deref().unwrap_or("");
    let after_body = after.body.as_deref().unwrap_or("");
    let body_from = (before_body != after_body).then_some(before_body);
    issues::edited(ctx, after, title_from, body_from);

    let before_labels = issues::issue_labels(before);
    let after_labels = issues::issue_labels(after);
    for label in added_labels(&before_labels, &after_labels) {
        issues::labeled(ctx, after, label);
    }
    for label in added_labels(&after_labels, &before_labels) {
        issues::unlabeled(ctx, after, label);
    }

    let before_assignees = assignees(before);
    let after_assignees = assignees(after);
    for user in added_users(&after_assignees, &before_assignees) {
        issues::unassigned(ctx, after, user);
    }
    for user in added_users(&before_assignees, &after_assignees) {
        issues::assigned(ctx, after, user);
    }

    match (before.milestone.as_ref(), after.milestone.as_ref()) {
        (Some(old), Some(new)) if old.number != new.number => {
            issues::demilestoned(ctx, after, old);
            issues::milestoned(ctx, after, new);
        }
        (Some(old), None) => issues::demilestoned(ctx, after, old),
        (None, Some(new)) => issues::milestoned(ctx, after, new),
        _ => {}
    }

    if before.state != after.state {
        if after.state == "closed" {
            issues::closed(ctx, after);
        } else if after.state == "open" {
            issues::reopened(ctx, after);
        }
    }

    if !before.locked && after.locked {
        issues::locked(ctx, after);
    } else if before.locked && !after.locked {
        issues::unlocked(ctx, after);
    }
}

fn added_labels<'a>(old: &'a [types::Label], new: &'a [types::Label]) -> Vec<&'a types::Label> {
    new.iter()
        .filter(|label| {
            !old.iter()
                .any(|existing| existing.name.eq_ignore_ascii_case(&label.name))
        })
        .collect()
}

fn assignees(issue: &types::Issue) -> Vec<types::SimpleUser> {
    issue.assignees.clone()
}

fn added_users<'a>(
    old: &'a [types::SimpleUser],
    new: &'a [types::SimpleUser],
) -> Vec<&'a types::SimpleUser> {
    new.iter()
        .filter(|user| {
            !old.iter()
                .any(|existing| existing.login.eq_ignore_ascii_case(&user.login))
        })
        .collect()
}
