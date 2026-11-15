//! Pull request review comments: repository-wide list, read, create, reply, update and delete.
//!
//! The author of a comment or a repository admin may edit or delete it. Comments that belong to
//! another user's pending review are invisible, as they are on GitHub. Only `created` fires a
//! webhook: the pinned webhook schemas carry no other `pull_request_review_comment` action.

use std::sync::Mutex;

use chrono::{DateTime, Utc};
use serde_json::{Map, Value};

use super::pull_review_comment_model::{Anchor, NewComment, build, invalid, missing, render_now};

use super::Handler;
use super::repos::{Found, find_repo};
use crate::api::{ApiRequest, ApiResponse, ApiResult, paginate};
use crate::operation::ops;
use crate::state::{self, State, lock};
use crate::types;
use crate::webhooks::events::{self, Ctx};

fn comment_id(request: &ApiRequest) -> Result<i64, ApiResponse> {
    request
        .param("comment_id")
        .parse()
        .map_err(|_| ApiResponse::not_found())
}

fn pull_number(request: &ApiRequest) -> Result<u64, ApiResponse> {
    request
        .param("pull_number")
        .parse()
        .map_err(|_| ApiResponse::not_found())
}

fn object_body(request: &ApiRequest) -> Result<Map<String, Value>, ApiResponse> {
    request.json_as::<Map<String, Value>>()
}

fn comment_text(body: &Map<String, Value>) -> Result<String, ApiResponse> {
    match body.get("body") {
        Some(Value::String(text)) if !text.is_empty() => Ok(text.clone()),
        Some(Value::String(_)) | None => Err(missing("body")),
        Some(_) => Err(invalid("body")),
    }
}

fn repo<'a>(state: &'a State, found: &Found) -> &'a state::Repo {
    state
        .account(&found.owner)
        .and_then(|account| account.repo(&found.name))
        .expect("find_repo found the repository")
}

fn pull_mut<'a>(
    state: &'a mut State,
    found: &Found,
    number: u64,
) -> Result<&'a mut state::PullRequest, ApiResponse> {
    state
        .account_mut(&found.owner)
        .and_then(|account| account.repo_mut(&found.name))
        .and_then(|repo| repo.pull_requests.get_mut(&number))
        .ok_or_else(ApiResponse::not_found)
}

/// Whether `comment` belongs to a pending review that `login` does not own.
fn hidden(
    pull: &state::PullRequest,
    comment: &types::PullRequestReviewComment,
    login: &str,
) -> bool {
    let Some(review_id) = comment.pull_request_review_id else {
        return false;
    };
    pull.reviews.iter().any(|review| {
        review.id == review_id
            && review.state == "PENDING"
            && !review
                .user
                .as_ref()
                .is_some_and(|user| user.login.eq_ignore_ascii_case(login))
    })
}

fn caller_login(request: &ApiRequest) -> &str {
    request
        .caller
        .as_ref()
        .map_or("", |caller| caller.login.as_str())
}

/// The pull request number and index of the comment `id`, when the caller can see it.
fn locate(state: &State, found: &Found, request: &ApiRequest, id: i64) -> Option<(u64, usize)> {
    let login = caller_login(request);
    repo(state, found)
        .pull_requests
        .iter()
        .find_map(|(number, pull)| {
            let index = pull.review_comments.iter().position(|c| c.id == id)?;
            (!hidden(pull, &pull.review_comments[index], login)).then_some((*number, index))
        })
}

impl Handler for ops::PullsListReviewCommentsForRepo {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let found = find_repo(&state, request)?;
        let login = caller_login(request);
        let since = match request.query("since") {
            Some(value) => Some(
                DateTime::parse_from_rfc3339(value)
                    .map(|time| time.with_timezone(&Utc))
                    .map_err(|_| ApiResponse::validation_failed("Pull", "since", "invalid"))?,
            ),
            None => None,
        };
        let mut comments = repo(&state, &found)
            .pull_requests
            .values()
            .flat_map(|pull| {
                pull.review_comments
                    .iter()
                    .filter(|comment| !hidden(pull, comment, login))
            })
            .filter(|comment| since.is_none_or(|since| comment.updated_at > since))
            .cloned()
            .collect::<Vec<_>>();
        let sort = request.query("sort").unwrap_or("created");
        if !["created", "updated", "created_at"].contains(&sort) {
            return Err(ApiResponse::validation_failed("Pull", "sort", "invalid"));
        }
        let descending = match request.query("direction") {
            None | Some("asc") => false,
            Some("desc") => true,
            Some(_) => {
                return Err(ApiResponse::validation_failed(
                    "Pull",
                    "direction",
                    "invalid",
                ));
            }
        };
        comments.sort_by(|left, right| {
            let order = if sort == "updated" {
                left.updated_at.cmp(&right.updated_at)
            } else {
                left.created_at.cmp(&right.created_at)
            }
            .then(left.id.cmp(&right.id));
            if descending { order.reverse() } else { order }
        });
        Ok(paginate(request, &request.path, comments))
    }
}

impl Handler for ops::PullsGetReviewComment {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let found = find_repo(&state, request)?;
        let id = comment_id(request)?;
        let (number, index) =
            locate(&state, &found, request, id).ok_or_else(ApiResponse::not_found)?;
        let comment = repo(&state, &found).pull_requests[&number].review_comments[index].clone();
        Ok(ApiResponse::typed(200, &comment))
    }
}

impl Handler for ops::PullsUpdateReviewComment {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = request.require_scope(&["repo", "public_repo"])?;
        let body = object_body(request)?;
        let text = comment_text(&body)?;
        let mut state = lock(state);
        let found = find_repo(&state, request)?;
        let id = comment_id(request)?;
        let (number, index) =
            locate(&state, &found, request, id).ok_or_else(ApiResponse::not_found)?;
        let pull = pull_mut(&mut state, &found, number)?;
        let comment = &mut pull.review_comments[index];
        if !may_change(&found, comment, &caller.login) {
            return Err(ApiResponse::error(
                403,
                "Must have admin rights to Repository.",
            ));
        }
        let now = render_now();
        comment.body = text;
        comment.updated_at = now;
        let comment = comment.clone();
        if let Some(data) = &mut pull.data {
            data.updated_at = now;
        }
        Ok(ApiResponse::typed(200, &comment))
    }
}

impl Handler for ops::PullsDeleteReviewComment {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = request.require_scope(&["repo", "public_repo"])?;
        let mut state = lock(state);
        let found = find_repo(&state, request)?;
        let id = comment_id(request)?;
        let (number, index) =
            locate(&state, &found, request, id).ok_or_else(ApiResponse::not_found)?;
        let pull = pull_mut(&mut state, &found, number)?;
        if !may_change(&found, &pull.review_comments[index], &caller.login) {
            return Err(ApiResponse::error(
                403,
                "Must have admin rights to Repository.",
            ));
        }
        pull.review_comments.remove(index);
        if let Some(data) = &mut pull.data {
            data.review_comments = pull.review_comments.len() as i64;
            data.updated_at = render_now();
        }
        Ok(ApiResponse::no_content())
    }
}

fn may_change(found: &Found, comment: &types::PullRequestReviewComment, login: &str) -> bool {
    found.permission.admin
        || comment
            .user
            .as_ref()
            .is_some_and(|user| user.login.eq_ignore_ascii_case(login))
}

impl Handler for ops::PullsCreateReviewComment {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = request.require_scope(&["repo", "public_repo"])?;
        let body = object_body(request)?;
        let text = comment_text(&body)?;
        let reply_to = match body.get("in_reply_to") {
            None | Some(Value::Null) => None,
            Some(value) => Some(value.as_i64().ok_or_else(|| invalid("in_reply_to"))?),
        };
        let mut state = lock(state);
        let found = find_repo(&state, request)?;
        let number = pull_number(request)?;
        if let Some(parent) = reply_to {
            return reply(
                &mut state,
                request,
                &found,
                number,
                parent,
                text,
                &caller.login,
            );
        }
        let anchor = Anchor::from_map(&body)?;
        let commit_override = match body.get("commit_id") {
            None | Some(Value::Null) => None,
            Some(Value::String(sha)) => Some(sha.clone()),
            Some(_) => return Err(invalid("commit_id")),
        };
        let author = author(&state, request, &caller.login)?;
        let id = state.next_id();
        let pull = pull_mut(&mut state, &found, number)?;
        let data = pull
            .data
            .as_mut()
            .ok_or_else(|| ApiResponse::error(500, "Pull request REST model is not seeded"))?;
        let head = data.head.sha.clone();
        if commit_override.as_deref().is_some_and(|sha| sha != head) {
            return Err(ApiResponse::validation_failed(
                "PullRequestReviewComment",
                "commit_id",
                "invalid",
            ));
        }
        let comment = build(
            request,
            &found,
            number,
            &author,
            NewComment {
                id,
                body: text,
                anchor,
                commit_id: &head,
                review_id: None,
                in_reply_to: None,
                diff_hunk: String::new(),
            },
        );
        store(&mut state, request, &found, number, &comment)?;
        Ok(ApiResponse::typed(201, &comment))
    }
}

impl Handler for ops::PullsCreateReplyForReviewComment {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = request.require_scope(&["repo", "public_repo"])?;
        let body = object_body(request)?;
        let text = comment_text(&body)?;
        let mut state = lock(state);
        let found = find_repo(&state, request)?;
        let number = pull_number(request)?;
        let parent = comment_id(request)?;
        reply(
            &mut state,
            request,
            &found,
            number,
            parent,
            text,
            &caller.login,
        )
    }
}

fn author(
    state: &State,
    request: &ApiRequest,
    login: &str,
) -> Result<types::SimpleUser, ApiResponse> {
    state
        .account(login)
        .map(|account| crate::render::simple_user(request, account))
        .ok_or_else(ApiResponse::not_found)
}

/// Appends `comment` to the pull request and queues its webhook.
fn store(
    state: &mut State,
    request: &ApiRequest,
    found: &Found,
    number: u64,
    comment: &types::PullRequestReviewComment,
) -> Result<(), ApiResponse> {
    let pull = pull_mut(state, found, number)?;
    pull.review_comments.push(comment.clone());
    let data = pull.data.as_mut().map(|data| {
        data.review_comments = pull.review_comments.len() as i64;
        data.updated_at = comment.created_at;
        data.clone()
    });
    if let Some(data) = data {
        let mut ctx = Ctx::new(state, request, &found.owner, &found.name);
        events::pull_request_review_comment::created(&mut ctx, &data, comment);
    }
    Ok(())
}

fn reply(
    state: &mut State,
    request: &ApiRequest,
    found: &Found,
    number: u64,
    parent: i64,
    text: String,
    login: &str,
) -> ApiResult {
    let author = author(state, request, login)?;
    let id = state.next_id();
    let pull = pull_mut(state, found, number)?;
    let parent = pull
        .review_comments
        .iter()
        .find(|comment| comment.id == parent && !hidden(pull, comment, login))
        .cloned()
        .ok_or_else(ApiResponse::not_found)?;
    if parent.in_reply_to_id.is_some() {
        return Err(ApiResponse::validation_failed(
            "PullRequestReviewComment",
            "in_reply_to",
            "invalid",
        ));
    }
    let comment = build(
        request,
        found,
        number,
        &author,
        NewComment {
            id,
            body: text,
            anchor: Anchor {
                path: parent.path.clone(),
                line: parent.line,
                position: parent.position,
                side: Some(parent.side.to_string()),
                start_line: parent.start_line,
                start_side: parent.start_side.map(|side| side.to_string()),
                file: parent.subject_type == Some(types::PullRequestReviewCommentSubjectType::File),
            },
            commit_id: &parent.commit_id,
            review_id: None,
            in_reply_to: Some(parent.id),
            diff_hunk: parent.diff_hunk.clone(),
        },
    );
    store(state, request, found, number, &comment)?;
    Ok(ApiResponse::typed(201, &comment))
}
