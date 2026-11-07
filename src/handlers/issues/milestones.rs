//! Milestone list, create, read, update, delete, and the labels on a milestone.
//!
//! Open and closed counts count issues and pull requests. Updating or deleting a milestone
//! rewrites every embedded copy.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::super::Handler;
use crate::api::{ApiRequest, ApiResponse, ApiResult, paginate};
use crate::handlers::repos::find_repo;
use crate::operation::ops;
use crate::state::{Milestone, Repo, State, lock};
use crate::types::{self, NullableMilestoneState};

impl Handler for ops::IssuesListMilestones {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state_filter = super::query_choice(
            request,
            "Milestone",
            "state",
            &["open", "closed", "all"],
            "open",
        )?;
        let sort = super::query_choice(
            request,
            "Milestone",
            "sort",
            &["due_on", "completeness"],
            "due_on",
        )?;
        let ascending =
            super::query_choice(request, "Milestone", "direction", &["asc", "desc"], "asc")?
                == "asc";
        let state = lock(state);
        let found = find_repo(&state, request)?;
        let repo = super::repo(&state, &found);
        let stored = repo.milestones.values().cloned().collect::<Vec<_>>();
        let mut rows = stored
            .iter()
            .map(|milestone| {
                super::render_stored(repo, request, &found.owner, &found.name, milestone)
            })
            .filter(|milestone| {
                state_filter == "all" || milestone.state.to_string() == state_filter
            })
            .collect::<Vec<_>>();
        rows.sort_by(|left, right| match sort {
            "completeness" => by_completeness(left, right, ascending),
            _ => by_due(left, right, ascending),
        });
        Ok(paginate(request, &request.path, rows))
    }
}

impl Handler for ops::IssuesCreateMilestone {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = request.require_scope(&["repo", "public_repo"])?;
        let body = super::object_body(request)?;
        let title = required_title(body.get("title"))?;
        let milestone_state =
            optional_state(body.get("state"))?.unwrap_or(NullableMilestoneState::Open);
        let description = optional_description(body.get("description"))?;
        let due_on = optional_due(body.get("due_on"))?;
        let mut state = lock(state);
        let found = find_repo(&state, request)?;
        super::require_push(&found)?;
        let id = state.next_id();
        let creator = super::caller_user(&state, request, &caller.login)?;
        let now = super::now();
        let repo = super::repo_mut(&mut state, &found);
        if title_taken(repo, &title, None) {
            return Err(ApiResponse::validation_failed(
                "Milestone",
                "title",
                "already_exists",
            ));
        }
        let number = repo.milestones.keys().next_back().copied().unwrap_or(0) + 1;
        let milestone = Milestone {
            id,
            number,
            title,
            description,
            state: milestone_state,
            creator: creator.into(),
            created_at: now,
            updated_at: now,
            closed_at: (milestone_state == NullableMilestoneState::Closed).then_some(now),
            due_on,
        };
        repo.milestones.insert(number, milestone);
        let rendered = render_number(repo, request, &found.owner, &found.name, number);
        Ok(ApiResponse::typed(201, &rendered).with_header("location", rendered.url.clone()))
    }
}

impl Handler for ops::IssuesGetMilestone {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let found = find_repo(&state, request)?;
        let number = super::milestone_number(request)?;
        let repo = super::repo(&state, &found);
        let milestone = repo
            .milestones
            .get(&number)
            .ok_or_else(ApiResponse::not_found)?;
        let rendered = super::render_stored(repo, request, &found.owner, &found.name, milestone);
        Ok(ApiResponse::typed(200, &rendered))
    }
}

impl Handler for ops::IssuesUpdateMilestone {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        request.require_scope(&["repo", "public_repo"])?;
        let body = super::object_body(request)?;
        let title = optional_title(body.get("title"))?;
        let milestone_state = optional_state(body.get("state"))?;
        let description = body.get("description").map(description_value).transpose()?;
        let due_on = body.get("due_on").map(due_value).transpose()?;
        let mut state = lock(state);
        let found = find_repo(&state, request)?;
        let number = super::milestone_number(request)?;
        if !super::repo(&state, &found).milestones.contains_key(&number) {
            return Err(ApiResponse::not_found());
        }
        super::require_push(&found)?;
        let repo = super::repo_mut(&mut state, &found);
        if title
            .as_ref()
            .is_some_and(|title| title_taken(repo, title, Some(number)))
        {
            return Err(ApiResponse::validation_failed(
                "Milestone",
                "title",
                "already_exists",
            ));
        }
        let milestone = repo.milestones.get_mut(&number).expect("milestone exists");
        if let Some(title) = title {
            milestone.title = title;
        }
        if let Some(description) = description {
            milestone.description = description;
        }
        if let Some(due_on) = due_on {
            milestone.due_on = Some(due_on);
        }
        if let Some(milestone_state) = milestone_state {
            apply_state(milestone, milestone_state);
        }
        milestone.updated_at = super::now();
        super::sync_milestones(repo, request, &found.owner, &found.name);
        let rendered = render_number(repo, request, &found.owner, &found.name, number);
        Ok(ApiResponse::typed(200, &rendered))
    }
}

impl Handler for ops::IssuesDeleteMilestone {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        request.require_scope(&["repo", "public_repo"])?;
        let mut state = lock(state);
        let found = find_repo(&state, request)?;
        let number = super::milestone_number(request)?;
        if !super::repo(&state, &found).milestones.contains_key(&number) {
            return Err(ApiResponse::not_found());
        }
        super::require_push(&found)?;
        let repo = super::repo_mut(&mut state, &found);
        repo.milestones.remove(&number);
        super::sync_milestones(repo, request, &found.owner, &found.name);
        Ok(ApiResponse::no_content())
    }
}

impl Handler for ops::IssuesListLabelsForMilestone {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let found = find_repo(&state, request)?;
        let number = super::milestone_number(request)?;
        let repo = super::repo(&state, &found);
        if !repo.milestones.contains_key(&number) {
            return Err(ApiResponse::not_found());
        }
        Ok(paginate(
            request,
            &request.path,
            milestone_labels(repo, number),
        ))
    }
}

fn render_number(
    repo: &Repo,
    request: &ApiRequest,
    owner: &str,
    name: &str,
    number: u64,
) -> types::NullableMilestone {
    let milestone = repo
        .milestones
        .get(&number)
        .expect("milestone exists")
        .clone();
    super::render_stored(repo, request, owner, name, &milestone)
}

fn milestone_labels(repo: &Repo, number: u64) -> Vec<types::Label> {
    let target = crate::render::id(number);
    let catalog = super::catalog(repo);
    let mut found = BTreeMap::new();
    for issue in repo.issues.values() {
        if issue
            .data
            .milestone
            .as_ref()
            .is_some_and(|milestone| milestone.number == target)
        {
            for label in &issue.data.labels {
                if let Some(label) = super::label_from_issue_item(label) {
                    found.entry(super::label_key(&label.name)).or_insert(label);
                }
            }
        }
    }
    for pull in repo
        .pull_requests
        .values()
        .filter_map(|pull| pull.data.as_ref())
    {
        if pull
            .milestone
            .as_ref()
            .is_some_and(|milestone| milestone.number == target)
        {
            for label in &pull.labels {
                let label = super::label_from_pull(label);
                found.entry(super::label_key(&label.name)).or_insert(label);
            }
        }
    }
    found
        .into_iter()
        .map(|(key, label)| catalog.get(&key).cloned().unwrap_or(label))
        .collect()
}

fn required_title(value: Option<&Value>) -> Result<String, ApiResponse> {
    match value {
        Some(Value::String(title)) => trimmed_title(title),
        _ => Err(ApiResponse::validation_failed(
            "Milestone",
            "title",
            "missing_field",
        )),
    }
}

fn optional_title(value: Option<&Value>) -> Result<Option<String>, ApiResponse> {
    match value {
        None => Ok(None),
        Some(Value::String(title)) => trimmed_title(title).map(Some),
        Some(_) => Err(ApiResponse::validation_failed(
            "Milestone",
            "title",
            "missing_field",
        )),
    }
}

fn trimmed_title(title: &str) -> Result<String, ApiResponse> {
    let title = title.trim();
    if title.is_empty() {
        return Err(ApiResponse::validation_failed(
            "Milestone",
            "title",
            "missing_field",
        ));
    }
    Ok(title.to_owned())
}

fn optional_state(value: Option<&Value>) -> Result<Option<NullableMilestoneState>, ApiResponse> {
    match value {
        None => Ok(None),
        Some(value) => parse_state(value).map(Some),
    }
}

fn parse_state(value: &Value) -> Result<NullableMilestoneState, ApiResponse> {
    match value {
        Value::String(text) if text == "open" => Ok(NullableMilestoneState::Open),
        Value::String(text) if text == "closed" => Ok(NullableMilestoneState::Closed),
        _ => Err(ApiResponse::validation_failed(
            "Milestone",
            "state",
            "invalid",
        )),
    }
}

fn optional_description(value: Option<&Value>) -> Result<Option<String>, ApiResponse> {
    match value {
        None => Ok(None),
        Some(value) => description_value(value),
    }
}

fn description_value(value: &Value) -> Result<Option<String>, ApiResponse> {
    match value {
        Value::String(text) => Ok(Some(text.clone())),
        Value::Null => Ok(None),
        _ => Err(ApiResponse::validation_failed(
            "Milestone",
            "description",
            "invalid",
        )),
    }
}

fn optional_due(value: Option<&Value>) -> Result<Option<DateTime<Utc>>, ApiResponse> {
    match value {
        None => Ok(None),
        Some(value) => due_value(value).map(Some),
    }
}

fn due_value(value: &Value) -> Result<DateTime<Utc>, ApiResponse> {
    match value {
        Value::String(text) => super::parse_timestamp(text, "Milestone", "due_on"),
        _ => Err(ApiResponse::validation_failed(
            "Milestone",
            "due_on",
            "invalid",
        )),
    }
}

fn title_taken(repo: &Repo, title: &str, except: Option<u64>) -> bool {
    repo.milestones.values().any(|milestone| {
        except != Some(milestone.number) && milestone.title.eq_ignore_ascii_case(title)
    })
}

fn apply_state(milestone: &mut Milestone, state: NullableMilestoneState) {
    match state {
        NullableMilestoneState::Closed if milestone.closed_at.is_none() => {
            milestone.closed_at = Some(super::now());
        }
        NullableMilestoneState::Open => milestone.closed_at = None,
        NullableMilestoneState::Closed => {}
    }
    milestone.state = state;
}

fn by_due(
    left: &types::NullableMilestone,
    right: &types::NullableMilestone,
    ascending: bool,
) -> Ordering {
    due_rank(left.due_on, !ascending)
        .cmp(&due_rank(right.due_on, !ascending))
        .then(by_number(left.number, right.number, ascending))
}

fn due_rank(due: Option<DateTime<Utc>>, descending: bool) -> (u8, i64) {
    match due {
        Some(time) => {
            let stamp = time.timestamp();
            (
                0,
                if descending {
                    stamp.saturating_neg()
                } else {
                    stamp
                },
            )
        }
        None => (1, 0),
    }
}

fn by_completeness(
    left: &types::NullableMilestone,
    right: &types::NullableMilestone,
    ascending: bool,
) -> Ordering {
    let order = completeness(left).total_cmp(&completeness(right));
    let order = if ascending { order } else { order.reverse() };
    order.then(by_number(left.number, right.number, ascending))
}

fn completeness(milestone: &types::NullableMilestone) -> f64 {
    let total = milestone.open_issues + milestone.closed_issues;
    if total == 0 {
        0.0
    } else {
        milestone.closed_issues as f64 / total as f64
    }
}

fn by_number(left: i64, right: i64, ascending: bool) -> Ordering {
    if ascending {
        left.cmp(&right)
    } else {
        right.cmp(&left)
    }
}
