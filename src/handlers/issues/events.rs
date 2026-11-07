//! `issues/list-events`, `issues/list-events-for-repo` and `issues/get-event`.
//!
//! The issue-event stream is the stored timeline events of a pull request plus the label,
//! assignee, lock, milestone and state events issue mutations record, rendered as the spec's
//! `issue-event` object. Repository and single-event reads embed the current issue.

use std::sync::Mutex;

use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{Value, json};

use super::super::Handler;
use crate::api::{ApiRequest, ApiResponse, ApiResult, paginate};
use crate::handlers::pulls::full_team;
use crate::handlers::repos::find_repo;
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
        let repo = super::repo(&state, &found);
        let api = format!("{}/repos/{}/{}", request.api_url, found.owner, found.name);
        let events = events_for(repo, number)?;
        let rows = events
            .iter()
            .map(|event| render_event(&api, event, None))
            .collect();
        Ok(paginate(request, &request.path, rows))
    }
}

impl Handler for ops::IssuesListEventsForRepo {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let found = find_repo(&state, request)?;
        let repo = super::repo(&state, &found);
        let api = format!("{}/repos/{}/{}", request.api_url, found.owner, found.name);
        let rows = repo_events(repo)
            .into_iter()
            .map(|(event, issue)| render_event(&api, &event, Some(&issue)))
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
        let repo = super::repo(&state, &found);
        let api = format!("{}/repos/{}/{}", request.api_url, found.owner, found.name);
        let (event, issue) = repo_events(repo)
            .into_iter()
            .find(|(event, _)| event.id == id)
            .ok_or_else(ApiResponse::not_found)?;
        Ok(ApiResponse::json(
            200,
            render_event(&api, &event, Some(&issue)),
        ))
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

fn repo_events(repo: &Repo) -> Vec<(IssueEvent, types::Issue)> {
    let mut rows = Vec::new();
    for issue in repo.issues.values() {
        for event in &issue.events {
            rows.push((event.clone(), issue.data.clone()));
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
    converted.requested_team.clone_from(&event.requested_team);
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

fn render_event(api: &str, event: &IssueEvent, issue: Option<&types::Issue>) -> Value {
    let created_at = text(&event.created_at);
    let commit_url = event
        .commit_id
        .as_ref()
        .map(|sha| format!("{api}/commits/{sha}"));
    let mut row = json!({
        "id": render::id(event.id),
        "node_id": render::node_id("IssueEvent", event.id),
        "url": format!("{api}/issues/events/{}", event.id),
        "actor": event.actor,
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
        row["assignee"] = serde_json::to_value(assignee).expect("users serialize to JSON");
        row["assigner"] = serde_json::to_value(&event.actor).expect("users serialize to JSON");
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
        row["review_requester"] =
            serde_json::to_value(&event.actor).expect("users serialize to JSON");
        if let Some(user) = &event.requested_reviewer {
            row["requested_reviewer"] =
                serde_json::to_value(user).expect("users serialize to JSON");
        }
        if let Some(team) = &event.requested_team {
            row["requested_team"] =
                serde_json::to_value(full_team(team)).expect("teams serialize to JSON");
        }
    }
    row
}

fn text(time: &DateTime<Utc>) -> String {
    time.to_rfc3339_opts(SecondsFormat::Secs, true)
}
