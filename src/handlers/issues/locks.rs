//! Lock and unlock an issue or pull request conversation.
//!
//! Push access is required. Locking a conversation that is already locked is rejected.
//! Unlocking one that is already unlocked succeeds and leaves it unchanged.

#![expect(
    clippy::disallowed_types,
    reason = "untyped JSON; remove when this module is typed"
)]

use std::sync::Mutex;

use serde_json::Value;

use super::super::Handler;
use crate::api::{ApiRequest, ApiResponse, ApiResult};
use crate::handlers::repos::find_repo;
use crate::operation::ops;
use crate::state::{Issue, LockReason, Repo, State, lock};
use crate::types;
use crate::webhooks::events;

impl Handler for ops::IssuesLock {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = request.require_scope(&["repo", "public_repo"])?;
        let body = super::json_body(request)?;
        let mut state = lock(state);
        let found = find_repo(&state, request)?;
        let number = super::issue_number(request)?;
        ensure_subject(found.repo(&state), number)?;
        // Source: <https://docs.github.com/en/rest/issues/issues#lock-an-issue>
        super::require_push(&found)?;
        let reason = lock_reason(&body)?;
        let actor = super::caller_user(&state, request, &caller.login)?;
        super::with_subject(
            found.repo_mut(&mut state),
            number,
            |issue| lock_issue(issue, reason),
            |pull| lock_pull(pull, reason),
        )?;
        let reason = reason.map(|reason| reason.as_str().to_owned());
        super::record::record_locked(&mut state, &found, number, actor, reason);
        if let Some(issue) = super::rendered_issue(&state, request, &found, number) {
            let mut ctx = super::webhook_ctx(&mut state, request, &found);
            events::issues::locked(&mut ctx, &issue)?;
        }
        Ok(ApiResponse::no_content())
    }
}

impl Handler for ops::IssuesUnlock {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = request.require_scope(&["repo", "public_repo"])?;
        let mut state = lock(state);
        let found = find_repo(&state, request)?;
        let number = super::issue_number(request)?;
        ensure_subject(found.repo(&state), number)?;
        // Source: <https://docs.github.com/en/rest/issues/issues#unlock-an-issue>
        super::require_push(&found)?;
        let was_locked = super::record::snapshot(&state, request, &found, number)?.locked;
        let actor = super::caller_user(&state, request, &caller.login)?;
        super::with_subject(
            found.repo_mut(&mut state),
            number,
            unlock_issue,
            unlock_pull,
        )?;
        if was_locked {
            super::record::record_unlocked(&mut state, &found, number, actor);
            if let Some(issue) = super::rendered_issue(&state, request, &found, number) {
                let mut ctx = super::webhook_ctx(&mut state, request, &found);
                events::issues::unlocked(&mut ctx, &issue)?;
            }
        }
        Ok(ApiResponse::no_content())
    }
}

fn ensure_subject(repo: &Repo, number: u64) -> Result<(), ApiResponse> {
    if let Some(pull) = repo.pull_requests.get(&number) {
        if pull.data.is_none() {
            return Err(ApiResponse::internal(
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

fn lock_reason(body: &Value) -> Result<Option<LockReason>, ApiResponse> {
    let value = match body {
        Value::Null => return Ok(None),
        Value::Object(object) => object.get("lock_reason"),
        _ => return Err(ApiResponse::unprocessable("Invalid request body")),
    };
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(reason)) if LockReason::parse(reason).is_some() => {
            Ok(LockReason::parse(reason))
        }
        Some(_) => Err(ApiResponse::validation_failed(
            "Issue",
            "lock_reason",
            "invalid",
        )),
    }
}

fn lock_issue(issue: &mut Issue, reason: Option<LockReason>) -> Result<(), ApiResponse> {
    if issue.locked {
        return Err(ApiResponse::validation_failed("Issue", "locked", "invalid"));
    }
    issue.locked = true;
    issue.lock_reason = reason;
    issue.updated_at = crate::time::now_utc();
    Ok(())
}

fn lock_pull(pull: &mut types::PullRequest, reason: Option<LockReason>) -> Result<(), ApiResponse> {
    if pull.locked {
        return Err(ApiResponse::validation_failed("Issue", "locked", "invalid"));
    }
    pull.locked = true;
    pull.active_lock_reason = reason.map(|reason| reason.as_str().to_owned());
    pull.updated_at = crate::time::now_utc();
    Ok(())
}

fn unlock_issue(issue: &mut Issue) -> Result<(), ApiResponse> {
    if issue.locked || issue.lock_reason.is_some() {
        issue.locked = false;
        issue.lock_reason = None;
        issue.updated_at = crate::time::now_utc();
    }
    Ok(())
}

fn unlock_pull(pull: &mut types::PullRequest) -> Result<(), ApiResponse> {
    if pull.locked || pull.active_lock_reason.is_some() {
        pull.locked = false;
        pull.active_lock_reason = None;
        pull.updated_at = crate::time::now_utc();
    }
    Ok(())
}
