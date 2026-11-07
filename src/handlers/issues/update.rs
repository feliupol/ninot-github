//! `issues/update`: edit an issue, including closing and reopening it.
//!
//! Pull requests are not updated here. Label, milestone and assignee keys are ignored for
//! callers who can edit the issue but do not have push access.

use std::sync::Mutex;

use serde_json::{Map, Value};

use super::super::Handler;
use crate::api::{ApiRequest, ApiResponse, ApiResult, Caller};
use crate::handlers::repos::{Found, find_repo};
use crate::operation::ops;
use crate::render;
use crate::state::{State, lock};
use crate::types::{self, IssueStateReason};

struct Loaded {
    state: String,
    labels: Vec<String>,
}

struct Transition {
    state: String,
    reason: IssueStateReason,
    closing: bool,
}

struct Change {
    title: Option<String>,
    body: Option<Option<String>>,
    labels: Option<Vec<types::Label>>,
    assignees: Option<Vec<types::SimpleUser>>,
    milestone: Option<Option<types::NullableMilestone>>,
    transition: Option<Transition>,
}

impl Handler for ops::IssuesUpdate {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = request.require_scope(&["repo", "public_repo"])?;
        let body = super::object_body(request)?;
        let mut state = lock(state);
        let found = find_repo(&state, request)?;
        let number = super::issue_number(request)?;
        let loaded = load_issue(&state, &found, &caller.login, number)?;
        let change = parse_change(&state, request, &found, &body, &loaded, number, caller)?;
        let issue = apply_change(&mut state, request, &found, &caller.login, number, change)?;
        Ok(ApiResponse::typed(200, &issue))
    }
}

fn load_issue(
    state: &State,
    found: &Found,
    login: &str,
    number: u64,
) -> Result<Loaded, ApiResponse> {
    let issue = super::repo(state, found)
        .issues
        .get(&number)
        .ok_or_else(ApiResponse::not_found)?;
    let author =
        super::author_login(&issue.data).is_some_and(|author| author.eq_ignore_ascii_case(login));
    if !found.permission.push && !author {
        return Err(ApiResponse::error(
            403,
            "Must have push access to this repository",
        ));
    }
    Ok(Loaded {
        state: issue.data.state.clone(),
        labels: super::label_names(&issue.data),
    })
}

fn parse_change(
    state: &State,
    request: &ApiRequest,
    found: &Found,
    body: &Map<String, Value>,
    loaded: &Loaded,
    number: u64,
    caller: &Caller,
) -> Result<Change, ApiResponse> {
    Ok(Change {
        title: parse_title(body)?,
        body: parse_body(body)?,
        labels: parse_labels(state, found, body, loaded)?,
        assignees: parse_assignees(state, request, found, body)?,
        milestone: parse_milestone(state, request, found, body)?,
        transition: parse_transition(state, caller, body, loaded, found, number)?,
    })
}

fn parse_title(body: &Map<String, Value>) -> Result<Option<String>, ApiResponse> {
    let Some(value) = body.get("title") else {
        return Ok(None);
    };
    let text = match value {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        Value::Null => {
            return Err(ApiResponse::validation_failed(
                "Issue",
                "title",
                "missing_field",
            ));
        }
        _ => return Err(ApiResponse::validation_failed("Issue", "title", "invalid")),
    };
    if text.trim().is_empty() {
        return Err(ApiResponse::validation_failed(
            "Issue",
            "title",
            "missing_field",
        ));
    }
    Ok(Some(text))
}

fn parse_body(body: &Map<String, Value>) -> Result<Option<Option<String>>, ApiResponse> {
    match body.get("body") {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(Value::String(text)) => Ok(Some(Some(text.clone()))),
        Some(_) => Err(ApiResponse::validation_failed("Issue", "body", "invalid")),
    }
}

fn parse_labels(
    state: &State,
    found: &Found,
    body: &Map<String, Value>,
    loaded: &Loaded,
) -> Result<Option<Vec<types::Label>>, ApiResponse> {
    if !found.permission.push {
        return Ok(None);
    }
    let Some(value) = body.get("labels") else {
        return Ok(None);
    };
    let names = label_names(value)?;
    let repo = super::repo(state, found);
    super::labels::require_labels(repo, &names, &loaded.labels).map(Some)
}

fn label_names(value: &Value) -> Result<Vec<String>, ApiResponse> {
    let Some(items) = value.as_array() else {
        return Err(ApiResponse::validation_failed("Issue", "labels", "invalid"));
    };
    items.iter().map(label_name).collect()
}

fn label_name(value: &Value) -> Result<String, ApiResponse> {
    let name = match value {
        Value::String(name) => name.clone(),
        Value::Object(object) => object
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned(),
        _ => return Err(ApiResponse::validation_failed("Issue", "labels", "invalid")),
    };
    let name = name.trim().to_owned();
    if name.is_empty() {
        return Err(ApiResponse::validation_failed(
            "Label",
            "name",
            "missing_field",
        ));
    }
    Ok(name)
}

fn parse_assignees(
    state: &State,
    request: &ApiRequest,
    found: &Found,
    body: &Map<String, Value>,
) -> Result<Option<Vec<types::SimpleUser>>, ApiResponse> {
    if !found.permission.push {
        return Ok(None);
    }
    if let Some(value) = body.get("assignees") {
        let logins = super::assignees::logins_from(value)?;
        return super::assignees::resolve_assignees(state, request, &found.owner, &logins)
            .map(Some);
    }
    if let Some(value) = body.get("assignee") {
        return single_assignee(state, request, &found.owner, value).map(Some);
    }
    Ok(None)
}

fn single_assignee(
    state: &State,
    request: &ApiRequest,
    owner: &str,
    value: &Value,
) -> Result<Vec<types::SimpleUser>, ApiResponse> {
    match value {
        Value::Null => Ok(Vec::new()),
        Value::String(login) => {
            super::assignees::resolve_assignees(state, request, owner, std::slice::from_ref(login))
        }
        _ => Err(ApiResponse::validation_failed(
            "Issue", "assignee", "invalid",
        )),
    }
}

fn parse_milestone(
    state: &State,
    request: &ApiRequest,
    found: &Found,
    body: &Map<String, Value>,
) -> Result<Option<Option<types::NullableMilestone>>, ApiResponse> {
    if !found.permission.push {
        return Ok(None);
    }
    let Some(value) = body.get("milestone") else {
        return Ok(None);
    };
    let Some(number) = milestone_number(value)? else {
        return Ok(Some(None));
    };
    let repo = super::repo(state, found);
    let Some(source) = repo.milestones.get(&number).cloned() else {
        return Err(ApiResponse::validation_failed(
            "Issue",
            "milestone",
            "invalid",
        ));
    };
    Ok(Some(Some(super::render_stored(
        repo,
        request,
        &found.owner,
        &found.name,
        &source,
    ))))
}

fn milestone_number(value: &Value) -> Result<Option<u64>, ApiResponse> {
    match value {
        Value::Null => Ok(None),
        Value::Number(number) => json_u64(number)
            .map(Some)
            .ok_or_else(|| ApiResponse::validation_failed("Issue", "milestone", "invalid")),
        Value::String(text) => text
            .trim()
            .parse()
            .map(Some)
            .map_err(|_| ApiResponse::validation_failed("Issue", "milestone", "invalid")),
        _ => Err(ApiResponse::validation_failed(
            "Issue",
            "milestone",
            "invalid",
        )),
    }
}

fn json_u64(number: &serde_json::Number) -> Option<u64> {
    number
        .as_u64()
        .or_else(|| number.as_i64().and_then(|value| u64::try_from(value).ok()))
}

fn parse_transition(
    state: &State,
    caller: &Caller,
    body: &Map<String, Value>,
    loaded: &Loaded,
    found: &Found,
    number: u64,
) -> Result<Option<Transition>, ApiResponse> {
    let Some(next) = body.get("state").and_then(Value::as_str) else {
        return Ok(None);
    };
    if next != "open" && next != "closed" {
        return Err(ApiResponse::validation_failed("Issue", "state", "invalid"));
    }
    // An unchanged state does not rewrite closed_at or state_reason. An explicit reason that
    // cannot apply to that state is still rejected, matching GitHub.
    if next == loaded.state {
        let explicit = explicit_reason(body)?;
        if explicit.is_some() {
            reason_for(next, explicit)?;
        }
        return Ok(None);
    }
    let reason = reason_for(next, explicit_reason(body)?)?;
    if reason == IssueStateReason::Duplicate {
        require_duplicate(state, caller, body, found, number)?;
    }
    Ok(Some(Transition {
        state: next.to_owned(),
        reason,
        closing: next == "closed",
    }))
}

fn explicit_reason(body: &Map<String, Value>) -> Result<Option<&str>, ApiResponse> {
    match body.get("state_reason") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.as_str())),
        Some(_) => Err(ApiResponse::validation_failed(
            "Issue",
            "state_reason",
            "invalid",
        )),
    }
}

fn reason_for(next: &str, explicit: Option<&str>) -> Result<IssueStateReason, ApiResponse> {
    match (next, explicit) {
        ("closed", None | Some("completed")) => Ok(IssueStateReason::Completed),
        ("closed", Some("not_planned")) => Ok(IssueStateReason::NotPlanned),
        ("closed", Some("duplicate")) => Ok(IssueStateReason::Duplicate),
        ("open", None | Some("reopened")) => Ok(IssueStateReason::Reopened),
        _ => Err(ApiResponse::validation_failed(
            "Issue",
            "state_reason",
            "invalid",
        )),
    }
}

fn require_duplicate(
    state: &State,
    caller: &Caller,
    body: &Map<String, Value>,
    found: &Found,
    number: u64,
) -> Result<(), ApiResponse> {
    let Some(id) = body.get("duplicate_issue_id").and_then(Value::as_i64) else {
        return Err(ApiResponse::validation_failed(
            "Issue",
            "duplicate_issue_id",
            "missing_field",
        ));
    };
    // The same numeric id on another repository is a valid duplicate target. Only this issue
    // itself, matched by owner, repository and number, is rejected.
    if !duplicate_visible(state, caller, found, number, id) {
        return Err(ApiResponse::validation_failed(
            "Issue",
            "duplicate_issue_id",
            "invalid",
        ));
    }
    Ok(())
}

fn duplicate_visible(state: &State, caller: &Caller, found: &Found, number: u64, id: i64) -> bool {
    let mut hits = Vec::new();
    for account in state.accounts() {
        for repo in account.repos() {
            if other_issue(account, repo, found, number, id) || other_pull(repo, id) {
                hits.push((account.login.clone(), repo.name.clone()));
            }
        }
    }
    hits.into_iter()
        .any(|(owner, name)| state.permission(Some(caller), &owner, &name).is_some())
}

fn other_issue(
    account: &crate::state::Account,
    repo: &crate::state::Repo,
    found: &Found,
    number: u64,
    id: i64,
) -> bool {
    let same_repo = account.login.eq_ignore_ascii_case(&found.owner)
        && repo.name.eq_ignore_ascii_case(&found.name);
    repo.issues
        .values()
        .any(|issue| issue.data.id == id && !(same_repo && issue.data.number == render::id(number)))
}

fn other_pull(repo: &crate::state::Repo, id: i64) -> bool {
    repo.pull_requests
        .values()
        .filter_map(|pull| pull.data.as_ref())
        .any(|pull| pull.id == id)
}

fn apply_change(
    state: &mut State,
    request: &ApiRequest,
    found: &Found,
    login: &str,
    number: u64,
    change: Change,
) -> Result<types::Issue, ApiResponse> {
    let actor = actor_for_close(state, request, login, &change)?;
    let repo = super::repo_mut(state, found);
    if let Some(issue) = repo.issues.get_mut(&number) {
        apply_fields(&mut issue.data, &change, actor);
    }
    super::sync_milestones(repo, request, &found.owner, &found.name);
    Ok(repo.issues.get(&number).expect("issue exists").data.clone())
}

fn actor_for_close(
    state: &State,
    request: &ApiRequest,
    login: &str,
    change: &Change,
) -> Result<Option<types::SimpleUser>, ApiResponse> {
    if change
        .transition
        .as_ref()
        .is_some_and(|transition| transition.closing)
    {
        return Ok(Some(super::caller_user(state, request, login)?));
    }
    Ok(None)
}

fn apply_fields(issue: &mut types::Issue, change: &Change, actor: Option<types::SimpleUser>) {
    if let Some(title) = &change.title {
        issue.title.clone_from(title);
    }
    if let Some(body) = &change.body {
        issue.body.clone_from(body);
    }
    if let Some(labels) = &change.labels {
        issue.labels = labels.iter().map(render::label_on_issue).collect();
    }
    if let Some(users) = &change.assignees {
        super::assignees::write_assignees(&mut issue.assignee, &mut issue.assignees, users.clone());
    }
    if let Some(milestone) = &change.milestone {
        issue.milestone.clone_from(milestone);
    }
    if let Some(transition) = &change.transition {
        issue.state.clone_from(&transition.state);
        issue.state_reason = Some(transition.reason);
        if transition.closing {
            issue.closed_at = Some(super::now());
            issue.closed_by = actor.map(Into::into);
        } else {
            issue.closed_at = None;
            issue.closed_by = None;
        }
    }
    issue.updated_at = super::now();
}
