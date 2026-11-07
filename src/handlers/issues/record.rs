//! Records issue events for mutations that GitHub lists in the issue-event stream.

use crate::handlers::repos::Found;
use crate::state::{IssueEvent, IssueEventKind, IssueEventLabel, Repo, State};
use crate::types;

use super::label_key;

/// Labels, assignees, milestone, state and lock of an issue or pull request.
pub(crate) struct Snapshot {
    /// Attached labels.
    pub labels: Vec<types::Label>,
    /// Assigned users.
    pub assignees: Vec<types::SimpleUser>,
    /// Milestone title, when one is set.
    pub milestone: Option<String>,
    /// `open` or `closed`.
    pub state: String,
    /// Whether the conversation is locked.
    pub locked: bool,
}

/// Reads the fields whose changes produce issue events.
pub(crate) fn snapshot(repo: &Repo, number: u64) -> Result<Snapshot, crate::api::ApiResponse> {
    use crate::api::ApiResponse;
    if let Some(pull) = repo.pull_requests.get(&number) {
        let data = pull
            .data
            .as_ref()
            .ok_or_else(|| ApiResponse::error(500, "Pull request REST model is not seeded"))?;
        return Ok(Snapshot {
            labels: data.labels.iter().map(super::label_from_pull).collect(),
            assignees: data.assignees.clone(),
            milestone: data
                .milestone
                .as_ref()
                .map(|milestone| milestone.title.clone()),
            state: match data.state {
                types::PullRequestState::Open => "open",
                types::PullRequestState::Closed => "closed",
            }
            .to_owned(),
            locked: data.locked,
        });
    }
    let issue = repo
        .issues
        .get(&number)
        .ok_or_else(ApiResponse::not_found)?;
    Ok(Snapshot {
        labels: issue
            .data
            .labels
            .iter()
            .filter_map(super::label_from_issue_item)
            .collect(),
        assignees: issue.data.assignees.clone(),
        milestone: issue
            .data
            .milestone
            .as_ref()
            .map(|milestone| milestone.title.clone()),
        state: issue.data.state.clone(),
        locked: issue.data.locked,
    })
}

/// Records unlabeled/labeled, unassigned/assigned, demilestoned/milestoned and closed/reopened
/// events for each difference between `before` and `after`.
pub(crate) fn record_changes(
    state: &mut State,
    found: &Found,
    number: u64,
    actor: &types::SimpleUser,
    before: &Snapshot,
    after: &Snapshot,
) {
    let mut events = Vec::new();
    for label in &before.labels {
        if !after
            .labels
            .iter()
            .any(|current| label_key(&current.name) == label_key(&label.name))
        {
            events.push(with_label(state, actor, IssueEventKind::Unlabeled, label));
        }
    }
    for label in &after.labels {
        if !before
            .labels
            .iter()
            .any(|current| label_key(&current.name) == label_key(&label.name))
        {
            events.push(with_label(state, actor, IssueEventKind::Labeled, label));
        }
    }
    for user in &before.assignees {
        if !after
            .assignees
            .iter()
            .any(|current| current.login.eq_ignore_ascii_case(&user.login))
        {
            events.push(with_assignee(
                state,
                actor,
                IssueEventKind::Unassigned,
                user,
            ));
        }
    }
    for user in &after.assignees {
        if !before
            .assignees
            .iter()
            .any(|current| current.login.eq_ignore_ascii_case(&user.login))
        {
            events.push(with_assignee(state, actor, IssueEventKind::Assigned, user));
        }
    }
    if before.milestone.as_deref() != after.milestone.as_deref() {
        if let Some(title) = &before.milestone {
            events.push(with_milestone(
                state,
                actor,
                IssueEventKind::Demilestoned,
                title,
            ));
        }
        if let Some(title) = &after.milestone {
            events.push(with_milestone(
                state,
                actor,
                IssueEventKind::Milestoned,
                title,
            ));
        }
    }
    if before.state != after.state {
        let kind = if after.state == "closed" {
            IssueEventKind::Closed
        } else {
            IssueEventKind::Reopened
        };
        events.push(IssueEvent::new(
            state.next_id(),
            kind,
            actor.clone(),
            super::now(),
        ));
    }
    append(super::repo_mut(state, found), number, events);
}

/// Records a `labeled` event for `label`.
pub(crate) fn record_labeled(
    state: &mut State,
    found: &Found,
    number: u64,
    actor: types::SimpleUser,
    label: &types::Label,
) {
    let event = with_label(state, &actor, IssueEventKind::Labeled, label);
    append(super::repo_mut(state, found), number, vec![event]);
}

/// Records a `locked` event. `reason` is JSON null when absent.
pub(crate) fn record_locked(
    state: &mut State,
    found: &Found,
    number: u64,
    actor: types::SimpleUser,
    reason: Option<String>,
) {
    let mut event = IssueEvent::new(state.next_id(), IssueEventKind::Locked, actor, super::now());
    event.lock_reason = reason;
    append(super::repo_mut(state, found), number, vec![event]);
}

/// Records an `unlocked` event.
pub(crate) fn record_unlocked(
    state: &mut State,
    found: &Found,
    number: u64,
    actor: types::SimpleUser,
) {
    let event = IssueEvent::new(
        state.next_id(),
        IssueEventKind::Unlocked,
        actor,
        super::now(),
    );
    append(super::repo_mut(state, found), number, vec![event]);
}

fn with_label(
    state: &mut State,
    actor: &types::SimpleUser,
    kind: IssueEventKind,
    label: &types::Label,
) -> IssueEvent {
    let mut event = IssueEvent::new(state.next_id(), kind, actor.clone(), super::now());
    event.label = Some(IssueEventLabel {
        name: label.name.clone(),
        color: label.color.clone(),
    });
    event
}

fn with_assignee(
    state: &mut State,
    actor: &types::SimpleUser,
    kind: IssueEventKind,
    assignee: &types::SimpleUser,
) -> IssueEvent {
    let mut event = IssueEvent::new(state.next_id(), kind, actor.clone(), super::now());
    event.assignee = Some(assignee.clone());
    event
}

fn with_milestone(
    state: &mut State,
    actor: &types::SimpleUser,
    kind: IssueEventKind,
    title: &str,
) -> IssueEvent {
    let mut event = IssueEvent::new(state.next_id(), kind, actor.clone(), super::now());
    event.milestone_title = Some(title.to_owned());
    event
}

fn append(repo: &mut Repo, number: u64, events: Vec<IssueEvent>) {
    if let Some(pull) = repo.pull_requests.get_mut(&number) {
        pull.issue_events.extend(events);
    } else if let Some(issue) = repo.issues.get_mut(&number) {
        issue.events.extend(events);
    }
}
