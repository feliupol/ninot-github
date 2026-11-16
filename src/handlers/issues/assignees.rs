//! Issue assignee operations.
//!
//! Callers without push access receive the issue unchanged: GitHub drops assignee changes
//! instead of answering 403. A user who cannot be assigned is 422 when the caller has push
//! access, and the check endpoints answer 404.

#![expect(
    clippy::disallowed_types,
    clippy::disallowed_methods,
    reason = "untyped JSON; remove when this module is typed"
)]

use std::sync::Mutex;

use serde_json::Value;

use super::super::Handler;
use crate::api::{ApiRequest, ApiResponse, ApiResult, paginate};
use crate::handlers::repos::find_repo;
use crate::operation::ops;
use crate::render;
use crate::state::{Account, Name, State, lock};
use crate::types;
use crate::webhooks::events;

/// Source: <https://docs.github.com/en/rest/issues/assignees#add-assignees-to-an-issue>
const MAX_ASSIGNEES: usize = 10;

impl Handler for ops::IssuesAddAssignees {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = request.require_scope(&["repo", "public_repo"])?;
        let body = super::object_body(request)?;
        let mut state = lock(state);
        let found = find_repo(&state, request)?;
        let number = super::issue_number(request)?;
        // Without push access the assignees are silently ignored.
        // Source: <https://docs.github.com/en/rest/issues/assignees#add-assignees-to-an-issue>
        if !found.permission.push {
            return current_issue(&state, request, &found, number)
                .map(|issue| ApiResponse::typed(201, &issue));
        }
        let logins = match body.get("assignees") {
            Some(value) => logins_from(value)?,
            None => Vec::new(),
        };
        let added = resolve_assignees(&state, request, &found.owner, &logins)?;
        let stored = super::rendered_issue(&state, request, &found, number);
        let already = stored
            .as_ref()
            .map(|issue| issue.assignees.clone())
            .unwrap_or_default();
        let newly: Vec<types::SimpleUser> = added
            .iter()
            .filter(|user| {
                !already
                    .iter()
                    .any(|current| current.login.eq_ignore_ascii_case(&user.login))
            })
            .cloned()
            .collect();
        let before = super::record::snapshot(&state, request, &found, number)?;
        let actor = super::caller_user(&state, request, &caller.login)?;
        let projected = super::with_subject(
            found.repo_mut(&mut state),
            number,
            |issue| {
                issue.assignees = merge_names(&issue.assignees, &added)?;
                issue.updated_at = crate::time::now_utc();
                Ok(None)
            },
            |pull| {
                let merged = merge_assignees(&pull.assignees, added.clone())?;
                write_assignees(&mut pull.assignee, &mut pull.assignees, merged);
                pull.updated_at = crate::time::now_utc();
                Ok(Some(render::issue_from_pull(pull)))
            },
        )?;
        let issue = current(&state, request, &found, number, projected);
        if let Some(stored) = super::rendered_issue(&state, request, &found, number) {
            let mut ctx = super::webhook_ctx(&mut state, request, &found);
            for user in &newly {
                events::issues::assigned(&mut ctx, &stored, user)?;
            }
        }
        let after = super::record::snapshot(&state, request, &found, number)?;
        super::record::record_changes(&mut state, &found, number, &actor, &before, &after);
        Ok(ApiResponse::typed(201, &issue))
    }
}

impl Handler for ops::IssuesRemoveAssignees {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = request.require_scope(&["repo", "public_repo"])?;
        let body = super::object_body(request)?;
        let mut state = lock(state);
        let found = find_repo(&state, request)?;
        let number = super::issue_number(request)?;
        // Without push access the assignees are silently ignored.
        // Source: <https://docs.github.com/en/rest/issues/assignees#remove-assignees-from-an-issue>
        if !found.permission.push {
            return current_issue(&state, request, &found, number)
                .map(|issue| ApiResponse::typed(200, &issue));
        }
        let logins = match body.get("assignees") {
            Some(value) => string_logins(value)?,
            None => Vec::new(),
        };
        let removed = super::rendered_issue(&state, request, &found, number)
            .map(|issue| without_kept(&issue.assignees, &logins))
            .unwrap_or_default();
        let before = super::record::snapshot(&state, request, &found, number)?;
        let actor = super::caller_user(&state, request, &caller.login)?;
        let projected = super::with_subject(
            found.repo_mut(&mut state),
            number,
            |issue| {
                issue
                    .assignees
                    .retain(|user| !logins.iter().any(|login| user.matches(login)));
                issue.updated_at = crate::time::now_utc();
                Ok(None)
            },
            |pull| {
                let remaining = without_logins(&pull.assignees, &logins);
                write_assignees(&mut pull.assignee, &mut pull.assignees, remaining);
                pull.updated_at = crate::time::now_utc();
                Ok(Some(render::issue_from_pull(pull)))
            },
        )?;
        let issue = current(&state, request, &found, number, projected);
        if let Some(stored) = super::rendered_issue(&state, request, &found, number) {
            let mut ctx = super::webhook_ctx(&mut state, request, &found);
            for user in &removed {
                events::issues::unassigned(&mut ctx, &stored, user)?;
            }
        }
        let after = super::record::snapshot(&state, request, &found, number)?;
        super::record::record_changes(&mut state, &found, number, &actor, &before, &after);
        Ok(ApiResponse::typed(200, &issue))
    }
}

impl Handler for ops::IssuesListAssignees {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let found = find_repo(&state, request)?;
        let users = super::assignable(&state, &found.owner)
            .into_iter()
            .map(|account| render::simple_user(request, account))
            .collect::<Vec<_>>();
        Ok(paginate(request, &request.path, users))
    }
}

impl Handler for ops::IssuesCheckUserCanBeAssigned {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let found = find_repo(&state, request)?;
        let assignee = super::decode_param(request.param("assignee"));
        if super::can_be_assigned(&state, &found.owner, &assignee) {
            Ok(ApiResponse::no_content())
        } else {
            Err(ApiResponse::not_found())
        }
    }
}

impl Handler for ops::IssuesCheckUserCanBeAssignedToIssue {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let found = find_repo(&state, request)?;
        let number = super::issue_number(request)?;
        let assignee = super::decode_param(request.param("assignee"));
        let repo = found.repo(&state);
        let exists = repo.issues.contains_key(&number) || repo.pull_requests.contains_key(&number);
        if exists && super::can_be_assigned(&state, &found.owner, &assignee) {
            Ok(ApiResponse::no_content())
        } else {
            Err(ApiResponse::not_found())
        }
    }
}

/// Reads assignee logins from a string or `{login}` array.
pub(super) fn logins_from(value: &Value) -> Result<Vec<String>, ApiResponse> {
    let Some(items) = value.as_array() else {
        return Err(ApiResponse::validation_failed(
            "Issue",
            "assignees",
            "invalid",
        ));
    };
    items.iter().map(login_value).collect()
}

fn string_logins(value: &Value) -> Result<Vec<String>, ApiResponse> {
    let Some(items) = value.as_array() else {
        return Err(ApiResponse::validation_failed(
            "Issue",
            "assignees",
            "invalid",
        ));
    };
    items
        .iter()
        .map(|item| {
            item.as_str()
                .filter(|login| !login.trim().is_empty())
                .map(str::to_owned)
                .ok_or_else(|| ApiResponse::validation_failed("Issue", "assignees", "invalid"))
        })
        .collect()
}

fn login_value(value: &Value) -> Result<String, ApiResponse> {
    let login = match value {
        Value::String(login) => login.clone(),
        Value::Object(object) => object
            .get("login")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned(),
        _ => {
            return Err(ApiResponse::validation_failed(
                "Issue",
                "assignees",
                "invalid",
            ));
        }
    };
    if login.trim().is_empty() {
        return Err(ApiResponse::validation_failed(
            "Issue",
            "assignees",
            "invalid",
        ));
    }
    Ok(login)
}

/// Resolves `logins` to assignable users of `owner`, preserving order and dropping duplicates.
pub(super) fn resolve_assignees(
    state: &State,
    request: &ApiRequest,
    owner: &str,
    logins: &[String],
) -> Result<Vec<types::SimpleUser>, ApiResponse> {
    let candidates = super::assignable(state, owner);
    let mut users = Vec::new();
    for login in logins {
        let Some(account) = find_candidate(&candidates, login) else {
            return Err(ApiResponse::validation_failed(
                "Issue",
                "assignees",
                "invalid",
            ));
        };
        if users
            .iter()
            .any(|user: &types::SimpleUser| user.login.eq_ignore_ascii_case(&account.login))
        {
            continue;
        }
        users.push(render::simple_user(request, account));
    }
    if users.len() > MAX_ASSIGNEES {
        return Err(ApiResponse::validation_failed(
            "Issue",
            "assignees",
            "invalid",
        ));
    }
    Ok(users)
}

fn find_candidate<'a>(candidates: &[&'a Account], login: &str) -> Option<&'a Account> {
    candidates
        .iter()
        .copied()
        .find(|account| account.login.eq_ignore_ascii_case(login))
}

/// Replaces `assignee` and `assignees` with `users`, keeping the first user as `assignee`.
pub(super) fn write_assignees(
    assignee: &mut Option<types::NullableSimpleUser>,
    assignees: &mut Vec<types::SimpleUser>,
    users: Vec<types::SimpleUser>,
) {
    *assignee = users.first().cloned().map(Into::into);
    *assignees = users;
}

/// Users already assigned are not replaced.
/// Source: <https://docs.github.com/en/rest/issues/assignees#add-assignees-to-an-issue>
fn merge_assignees(
    existing: &[types::SimpleUser],
    added: Vec<types::SimpleUser>,
) -> Result<Vec<types::SimpleUser>, ApiResponse> {
    let mut users = existing.to_vec();
    for user in added {
        if users
            .iter()
            .any(|current| current.login.eq_ignore_ascii_case(&user.login))
        {
            continue;
        }
        users.push(user);
    }
    if users.len() > MAX_ASSIGNEES {
        return Err(ApiResponse::validation_failed(
            "Issue",
            "assignees",
            "invalid",
        ));
    }
    Ok(users)
}

/// `existing` followed by the users of `added` it does not name, at most [`MAX_ASSIGNEES`].
fn merge_names(existing: &[Name], added: &[types::SimpleUser]) -> Result<Vec<Name>, ApiResponse> {
    let mut names = existing.to_vec();
    for user in added {
        if !names.iter().any(|name| name.matches(&user.login)) {
            names.push(Name::new(user.login.clone()));
        }
    }
    if names.len() > MAX_ASSIGNEES {
        return Err(ApiResponse::validation_failed(
            "Issue",
            "assignees",
            "invalid",
        ));
    }
    Ok(names)
}

/// The issue a pull-request edit projected, or the stored issue rendered after an issue edit.
fn current(
    state: &State,
    request: &ApiRequest,
    found: &crate::handlers::repos::Found,
    number: u64,
    projected: Option<types::Issue>,
) -> types::Issue {
    projected.unwrap_or_else(|| {
        super::rendered_issue(state, request, found, number).expect("the edited issue is stored")
    })
}

fn without_kept(existing: &[types::SimpleUser], logins: &[String]) -> Vec<types::SimpleUser> {
    existing
        .iter()
        .filter(|user| {
            logins
                .iter()
                .any(|login| user.login.eq_ignore_ascii_case(login))
        })
        .cloned()
        .collect()
}

fn without_logins(existing: &[types::SimpleUser], logins: &[String]) -> Vec<types::SimpleUser> {
    existing
        .iter()
        .filter(|user| {
            !logins
                .iter()
                .any(|login| user.login.eq_ignore_ascii_case(login))
        })
        .cloned()
        .collect()
}

fn current_issue(
    state: &State,
    request: &ApiRequest,
    found: &crate::handlers::repos::Found,
    number: u64,
) -> Result<types::Issue, ApiResponse> {
    let repo = found.repo(state);
    if let Some(pull) = repo.pull_requests.get(&number) {
        let data = pull
            .data
            .as_ref()
            .ok_or_else(|| ApiResponse::internal("Pull request REST model is not seeded"))?;
        return Ok(render::issue_from_pull(data));
    }
    super::rendered_issue(state, request, found, number).ok_or_else(ApiResponse::not_found)
}
