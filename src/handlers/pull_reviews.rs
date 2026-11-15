//! Pull request reviews: read, update, delete while pending, submit, dismiss and list comments.
//!
//! A pending review is visible only to its author. Only its author edits, deletes or submits it.
//! Dismissing needs push access and fires no webhook, because the pinned webhook schemas carry
//! only `submitted`; the dismissal message is validated but not stored, because
//! the review model has no field for it.

use std::sync::Mutex;

use serde_json::{Map, Value};

use super::Handler;
use super::pull_review_comment_model::{Anchor, NewComment, build};
use super::repos::{Found, find_repo};
use crate::api::{ApiRequest, ApiResponse, ApiResult, paginate};
use crate::operation::ops;
use crate::state::{self, State, lock};
use crate::types;
use crate::webhooks::events::{self, Ctx};

fn number(request: &ApiRequest) -> Result<u64, ApiResponse> {
    request
        .param("pull_number")
        .parse()
        .map_err(|_| ApiResponse::not_found())
}

fn review_id(request: &ApiRequest) -> Result<i64, ApiResponse> {
    request
        .param("review_id")
        .parse()
        .map_err(|_| ApiResponse::not_found())
}

fn field(name: &str, code: &str) -> ApiResponse {
    ApiResponse::validation_failed("PullRequestReview", name, code)
}

fn body_object(request: &ApiRequest) -> Result<Map<String, Value>, ApiResponse> {
    request.json_as::<Map<String, Value>>()
}

fn text(body: &Map<String, Value>, name: &str) -> Result<Option<String>, ApiResponse> {
    match body.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        Some(_) => Err(field(name, "invalid")),
    }
}

fn authored_by(review: &types::PullRequestReview, login: &str) -> bool {
    review
        .user
        .as_ref()
        .is_some_and(|user| user.login.eq_ignore_ascii_case(login))
}

/// Index of review `id` on the pull request, or 404 when it is absent or another user's pending
/// review.
fn locate(pull: &state::PullRequest, id: i64, login: &str) -> Result<usize, ApiResponse> {
    pull.reviews
        .iter()
        .position(|review| {
            review.id == id && (review.state != "PENDING" || authored_by(review, login))
        })
        .ok_or_else(ApiResponse::not_found)
}

fn forbidden() -> ApiResponse {
    ApiResponse::error(403, "Resource not accessible by personal access token")
}

impl Handler for ops::PullsGetReview {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let login = request.caller.as_ref().map(|caller| caller.login.clone());
        let mut state = lock(state);
        let found = find_repo(&state, request)?;
        let pull = found.pull_mut(&mut state, number(request)?)?;
        let index = locate(pull, review_id(request)?, login.as_deref().unwrap_or(""))?;
        Ok(ApiResponse::typed(200, &pull.reviews[index]))
    }
}

impl Handler for ops::PullsUpdateReview {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = request.require_scope(&["repo", "public_repo"])?;
        let body = body_object(request)?;
        let text = text(&body, "body")?.ok_or_else(|| field("body", "missing_field"))?;
        let mut state = lock(state);
        let found = find_repo(&state, request)?;
        let pull = found.pull_mut(&mut state, number(request)?)?;
        let index = locate(pull, review_id(request)?, &caller.login)?;
        let review = &mut pull.reviews[index];
        if !authored_by(review, &caller.login) {
            return Err(forbidden());
        }
        review.body = text;
        Ok(ApiResponse::typed(200, review))
    }
}

impl Handler for ops::PullsDeletePendingReview {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = request.require_scope(&["repo", "public_repo"])?;
        let mut state = lock(state);
        let found = find_repo(&state, request)?;
        let pull = found.pull_mut(&mut state, number(request)?)?;
        let id = review_id(request)?;
        let index = locate(pull, id, &caller.login)?;
        // Submitted reviews cannot be deleted.
        // Source: <https://docs.github.com/en/rest/pulls/reviews#delete-a-pending-review-for-a-pull-request>
        if pull.reviews[index].state != "PENDING" {
            return Err(ApiResponse::unprocessable(
                "Can not delete a non-pending pull request review",
            ));
        }
        let review = pull.reviews.remove(index);
        pull.review_comments
            .retain(|comment| comment.pull_request_review_id != Some(id));
        if let Some(data) = &mut pull.data {
            data.review_comments = pull.review_comments.len() as i64;
        }
        Ok(ApiResponse::typed(200, &review))
    }
}

impl Handler for ops::PullsListCommentsForReview {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let login = request.caller.as_ref().map(|caller| caller.login.clone());
        let mut state = lock(state);
        let found = find_repo(&state, request)?;
        let pull = found.pull_mut(&mut state, number(request)?)?;
        let id = review_id(request)?;
        locate(pull, id, login.as_deref().unwrap_or(""))?;
        let comments = pull
            .review_comments
            .iter()
            .filter(|comment| comment.pull_request_review_id == Some(id))
            .cloned()
            .collect::<Vec<_>>();
        Ok(paginate(request, &request.path, comments))
    }
}

impl Handler for ops::PullsDismissReview {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = request.require_scope(&["repo", "public_repo"])?;
        let body = body_object(request)?;
        text(&body, "message")?.ok_or_else(|| field("message", "missing_field"))?;
        if text(&body, "event")?.is_some_and(|event| event != "DISMISS") {
            return Err(field("event", "invalid"));
        }
        let mut state = lock(state);
        let found = find_repo(&state, request)?;
        if !found.permission.push {
            return Err(forbidden());
        }
        let number = number(request)?;
        let pull = found.pull_mut(&mut state, number)?;
        let index = locate(pull, review_id(request)?, &caller.login)?;
        let review = &mut pull.reviews[index];
        match review.state.as_str() {
            "PENDING" => {
                return Err(ApiResponse::unprocessable(
                    "Can not dismiss a pending pull request review",
                ));
            }
            "COMMENTED" => {
                return Err(ApiResponse::unprocessable(
                    "Can not dismiss a commented pull request review",
                ));
            }
            "DISMISSED" => {
                return Err(ApiResponse::unprocessable(
                    "Can not dismiss a dismissed pull request review",
                ));
            }
            _ => {}
        }
        "DISMISSED".clone_into(&mut review.state);
        let review = review.clone();
        if let Some(data) = &mut pull.data {
            data.updated_at = crate::time::now_utc();
        }
        Ok(ApiResponse::typed(200, &review))
    }
}

impl Handler for ops::PullsSubmitReview {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = request.require_scope(&["repo", "public_repo"])?;
        let body = body_object(request)?;
        // Source: <https://docs.github.com/en/rest/pulls/reviews#submit-a-review-for-a-pull-request>
        let event = text(&body, "event")?.ok_or_else(|| field("event", "missing_field"))?;
        let review_state = match event.as_str() {
            "APPROVE" => "APPROVED",
            "REQUEST_CHANGES" => "CHANGES_REQUESTED",
            "COMMENT" => "COMMENTED",
            _ => return Err(field("event", "invalid")),
        };
        let new_body = text(&body, "body")?;
        let mut state = lock(state);
        let found = find_repo(&state, request)?;
        let number = number(request)?;
        let pull = found.pull_mut(&mut state, number)?;
        let index = locate(pull, review_id(request)?, &caller.login)?;
        let review = &mut pull.reviews[index];
        // The status only. Source: OpenAPI pulls/submit-review 403
        if !authored_by(review, &caller.login) {
            return Err(forbidden());
        }
        if review.state != "PENDING" {
            return Err(ApiResponse::unprocessable(
                "Can not submit a non-pending pull request review",
            ));
        }
        let data = pull
            .data
            .as_mut()
            .ok_or_else(|| ApiResponse::internal("Pull request REST model is not seeded"))?;
        if review_state != "COMMENTED" && data.user.login.eq_ignore_ascii_case(&caller.login) {
            return Err(ApiResponse::unprocessable(
                "A pull request author cannot approve or request changes on their own pull request",
            ));
        }
        let review = &mut pull.reviews[index];
        if let Some(new_body) = new_body {
            review.body = new_body;
        }
        if review_state != "APPROVED" && review.body.is_empty() {
            return Err(ApiResponse::unprocessable(
                "body is required for this review event",
            ));
        }
        let now = crate::time::now_utc();
        review.state = review_state.to_owned();
        review.submitted_at = Some(now);
        let review = review.clone();
        let id = review.id;
        data.updated_at = now;
        // Source: <https://docs.github.com/en/rest/pulls/review-requests#get-all-requested-reviewers-for-a-pull-request>
        data.requested_reviewers
            .retain(|reviewer| !reviewer.login.eq_ignore_ascii_case(&caller.login));
        let data = data.clone();
        let comments = pull
            .review_comments
            .iter()
            .filter(|comment| comment.pull_request_review_id == Some(id))
            .cloned()
            .collect::<Vec<_>>();
        let mut ctx = Ctx::new(&mut state, request, &found.owner, &found.name);
        events::pull_request_review::submitted(&mut ctx, &data, &review)?;
        for comment in &comments {
            events::pull_request_review_comment::created(&mut ctx, &data, comment)?;
        }
        Ok(ApiResponse::typed(200, &review))
    }
}

/// Attaches the inline comments of a new review to the pull request, in request order.
///
/// # Errors
///
/// Returns 422 when a comment has no body, path, or line.
pub(super) fn attach_comments(
    request: &ApiRequest,
    found: &Found,
    number: u64,
    author: &types::SimpleUser,
    review: i64,
    head: &str,
    items: &[(u64, &types::PullsCreateReviewRequestCommentsItem)],
) -> Result<Vec<types::PullRequestReviewComment>, ApiResponse> {
    items
        .iter()
        .map(|(id, item)| {
            if item.body.is_empty() {
                return Err(ApiResponse::validation_failed(
                    "PullRequestReviewComment",
                    "body",
                    "missing_field",
                ));
            }
            Ok(build(
                request,
                found,
                number,
                author,
                NewComment {
                    id: *id,
                    body: item.body.clone(),
                    anchor: Anchor::from_item(item)?,
                    commit_id: head,
                    review_id: Some(review),
                    in_reply_to: None,
                    diff_hunk: String::new(),
                },
            ))
        })
        .collect()
}
