//! `issues/list-events`, `issues/list-events-for-repo` and `issues/get-event`.
//!
//! The issue-event stream is the stored timeline events of a pull request plus the label,
//! assignee, lock, milestone and state events issue mutations record, rendered as the spec's
//! `issue-event` object. Repository and single-event reads embed the current issue.

#![expect(
    clippy::disallowed_types,
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    reason = "untyped JSON; remove when this module is typed"
)]

use std::sync::Mutex;

use serde_json::{Value, json};

use super::super::Handler;
use crate::api::{ApiRequest, ApiResponse, ApiResult, paginate};
use crate::handlers::pulls::full_team;
use crate::handlers::repos::{Found, find_repo};
use crate::operation::ops;
use crate::render;
use crate::state::{
    IssueEvent, IssueEventKind, Repo, State, TimelineEvent, TimelineEventKind, lock,
};
use crate::types;

impl Handler for ops::IssuesListEvents {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let found = find_repo(&state, request)?;
        let number = super::issue_number(request)?;
        let repo = found.repo(&state);
        let events = events_for(repo, number)?;
        let rows = events
            .iter()
            .map(|event| render_event(&state, request, &found, event, None))
            .collect();
        Ok(paginate(request, &request.path, rows))
    }
}

impl Handler for ops::IssuesListEventsForRepo {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let found = find_repo(&state, request)?;
        let rows = repo_events(super::context(&state, request, &found))
            .into_iter()
            .map(|(event, issue)| render_event(&state, request, &found, &event, Some(&issue)))
            .collect();
        Ok(paginate(request, &request.path, rows))
    }
}

impl Handler for ops::IssuesGetEvent {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let found = find_repo(&state, request)?;
        let id = request
            .param("event_id")
            .parse::<u64>()
            .map_err(|_| ApiResponse::not_found())?;
        let (event, issue) = repo_events(super::context(&state, request, &found))
            .into_iter()
            .find(|(event, _)| event.id == id)
            .ok_or_else(ApiResponse::not_found)?;
        Ok(ApiResponse::ok(render_event(
            &state,
            request,
            &found,
            &event,
            Some(&issue),
        )))
    }
}

fn events_for(repo: &Repo, number: u64) -> Result<Vec<IssueEvent>, ApiResponse> {
    if let Some(pull) = repo.pull_requests.get(&number) {
        return Ok(pull_events(pull));
    }
    repo.issues
        .get(&number)
        .map(|issue| issue.events.clone())
        .ok_or_else(ApiResponse::not_found)
}

fn pull_events(pull: &crate::state::PullRequest) -> Vec<IssueEvent> {
    let mut events: Vec<IssueEvent> = pull.events.iter().map(from_timeline).collect();
    events.extend(pull.issue_events.iter().cloned());
    events.sort_by_key(|event| (event.created_at, event.id));
    events
}

fn repo_events(context: render::IssueContext<'_>) -> Vec<(IssueEvent, types::Issue)> {
    let repo = context.repo;
    let mut rows = Vec::new();
    for issue in repo.issues.values() {
        let rendered = render::issue(context, issue);
        for event in &issue.events {
            rows.push((event.clone(), rendered.clone()));
        }
    }
    for pull in repo.pull_requests.values() {
        let Some(data) = &pull.data else {
            continue;
        };
        let issue = render::issue_from_pull(data);
        for event in pull_events(pull) {
            rows.push((event, issue.clone()));
        }
    }
    rows.sort_by_key(|(event, _)| (event.created_at, event.id));
    rows
}

fn from_timeline(event: &TimelineEvent) -> IssueEvent {
    let mut converted = IssueEvent::new(
        event.id,
        kind_from_timeline(event.kind),
        event.actor.clone(),
        event.created_at,
    );
    converted.commit_id.clone_from(&event.commit_id);
    converted
        .requested_reviewer
        .clone_from(&event.requested_reviewer);
    converted.requested_team = event.requested_team;
    converted
}

fn kind_from_timeline(kind: TimelineEventKind) -> IssueEventKind {
    match kind {
        TimelineEventKind::ReadyForReview => IssueEventKind::ReadyForReview,
        TimelineEventKind::ConvertToDraft => IssueEventKind::ConvertToDraft,
        TimelineEventKind::ReviewRequested => IssueEventKind::ReviewRequested,
        TimelineEventKind::ReviewRequestRemoved => IssueEventKind::ReviewRequestRemoved,
        TimelineEventKind::HeadRefForcePushed => IssueEventKind::HeadRefForcePushed,
        TimelineEventKind::Merged => IssueEventKind::Merged,
        TimelineEventKind::Closed => IssueEventKind::Closed,
        TimelineEventKind::Reopened => IssueEventKind::Reopened,
    }
}

fn render_event(
    state: &State,
    request: &ApiRequest,
    found: &Found,
    event: &IssueEvent,
    issue: Option<&types::Issue>,
) -> Value {
    let repo = request.urls().repo(&found.owner, &found.name);
    let user = |login: &str| {
        serde_json::to_value(render::referenced_user(state, request, login))
            .expect("users serialize to JSON")
    };
    let actor = user(&event.actor);
    let created_at = crate::time::format(&event.created_at);
    let commit_url = event.commit_id.as_ref().map(|sha| repo.commit(sha).api);
    let mut row = json!({
        "id": render::id(event.id),
        "node_id": render::node_id("IssueEvent", event.id),
        "url": repo.issue_event(event.id),
        "actor": actor,
        "event": event.kind.name(),
        "commit_id": event.commit_id,
        "commit_url": commit_url,
        "created_at": created_at,
    });
    if let Some(issue) = issue {
        row["issue"] = serde_json::to_value(issue).expect("issues serialize to JSON");
    }
    if let Some(label) = &event.label {
        row["label"] = json!({ "name": label.name, "color": label.color });
    }
    if let Some(assignee) = &event.assignee {
        row["assignee"] = user(assignee);
        row["assigner"] = actor.clone();
    }
    if let Some(title) = &event.milestone_title {
        row["milestone"] = json!({ "title": title });
    }
    if event.kind == IssueEventKind::Locked {
        row["lock_reason"] = event
            .lock_reason
            .as_ref()
            .map_or(Value::Null, |reason| json!(reason));
    }
    if matches!(
        event.kind,
        IssueEventKind::ReviewRequested | IssueEventKind::ReviewRequestRemoved
    ) {
        row["review_requester"] = actor;
        if let Some(login) = &event.requested_reviewer {
            row["requested_reviewer"] = user(login);
        }
        if let Some(team) = event
            .requested_team
            .and_then(|id| render::referenced_team(state, request, &found.owner, id))
        {
            row["requested_team"] =
                serde_json::to_value(full_team(&team)).expect("teams serialize to JSON");
        }
    }
    row
}
