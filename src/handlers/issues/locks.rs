//! Lock and unlock an issue or pull request conversation.
//!
//! Push access is required. Locking a conversation that is already locked is rejected.
//! Unlocking one that is already unlocked succeeds and leaves it unchanged.

use std::sync::Mutex;

use serde_json::Value;

use super::super::Handler;
use crate::api::{ApiRequest, ApiResponse, ApiResult};
use crate::handlers::repos::find_repo;
use crate::operation::ops;
use crate::state::{Repo, State, lock};
use crate::types;

const REASONS: [&str; 4] = ["off-topic", "too heated", "resolved", "spam"];

impl Handler for ops::IssuesLock {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        request.require_scope(&["repo", "public_repo"])?;
        let body = super::json_body(request)?;
        let mut state = lock(state);
        let found = find_repo(&state, request)?;
        let number = super::issue_number(request)?;
        ensure_subject(super::repo(&state, &found), number)?;
        super::require_push(&found)?;
        let reason = lock_reason(&body)?;
        super::with_subject(
            super::repo_mut(&mut state, &found),
            number,
            |issue| lock_issue(issue, reason.clone()),
            |pull| lock_pull(pull, reason.clone()),
        )?;
        Ok(ApiResponse::no_content())
    }
}

impl Handler for ops::IssuesUnlock {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        request.require_scope(&["repo", "public_repo"])?;
        let mut state = lock(state);
        let found = find_repo(&state, request)?;
        let number = super::issue_number(request)?;
        ensure_subject(super::repo(&state, &found), number)?;
        super::require_push(&found)?;
        super::with_subject(
            super::repo_mut(&mut state, &found),
            number,
            unlock_issue,
            unlock_pull,
        )?;
        Ok(ApiResponse::no_content())
    }
}

fn ensure_subject(repo: &Repo, number: u64) -> Result<(), ApiResponse> {
    if let Some(pull) = repo.pull_requests.get(&number) {
        if pull.data.is_none() {
            return Err(ApiResponse::error(
                500,
                "Pull request REST model is not seeded",
            ));
        }
        return Ok(());
    }
    if repo.issues.contains_key(&number) {
        Ok(())
    } else {
        Err(ApiResponse::not_found())
    }
}

fn lock_reason(body: &Value) -> Result<Option<String>, ApiResponse> {
    let value = match body {
        Value::Null => return Ok(None),
        Value::Object(object) => object.get("lock_reason"),
        _ => return Err(ApiResponse::error(422, "Invalid request body")),
    };
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(reason)) if REASONS.contains(&reason.as_str()) => {
            Ok(Some(reason.clone()))
        }
        Some(_) => Err(ApiResponse::validation_failed(
            "Issue",
            "lock_reason",
            "invalid",
        )),
    }
}

fn lock_issue(issue: &mut types::Issue, reason: Option<String>) -> Result<(), ApiResponse> {
    if issue.locked {
        return Err(ApiResponse::validation_failed("Issue", "locked", "invalid"));
    }
    issue.locked = true;
    issue.active_lock_reason = reason;
    issue.updated_at = super::now();
    Ok(())
}

fn lock_pull(pull: &mut types::PullRequest, reason: Option<String>) -> Result<(), ApiResponse> {
    if pull.locked {
        return Err(ApiResponse::validation_failed("Issue", "locked", "invalid"));
    }
    pull.locked = true;
    pull.active_lock_reason = reason;
    pull.updated_at = super::now();
    Ok(())
}

fn unlock_issue(issue: &mut types::Issue) -> Result<(), ApiResponse> {
    if issue.locked || issue.active_lock_reason.is_some() {
        issue.locked = false;
        issue.active_lock_reason = None;
        issue.updated_at = super::now();
    }
    Ok(())
}

fn unlock_pull(pull: &mut types::PullRequest) -> Result<(), ApiResponse> {
    if pull.locked || pull.active_lock_reason.is_some() {
        pull.locked = false;
        pull.active_lock_reason = None;
        pull.updated_at = super::now();
    }
    Ok(())
}
