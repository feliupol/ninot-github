//! Issue comment read, update, delete, and the repository-wide comment list.
//!
//! Comments on issues and on pull requests are included. Review comments are not.
//! The author or a repository admin may edit or delete a comment. Push access is not enough.

#![expect(
    clippy::disallowed_types,
    reason = "untyped JSON; remove when this module is typed"
)]

use std::sync::Mutex;

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::super::Handler;
use crate::api::{ApiRequest, ApiResponse, ApiResult, paginate};
use crate::handlers::repos::{Found, find_repo};
use crate::operation::ops;
use crate::render;
use crate::state::{Repo, State, lock};
use crate::types;
use crate::webhooks::events;

#[derive(Clone, Copy)]
enum Place {
    Issue { number: u64, index: usize },
    Pull { number: u64, index: usize },
}

impl Handler for ops::IssuesGetComment {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let found = find_repo(&state, request)?;
        let id = super::comment_id(request)?;
        let comment = locate(found.repo(&state), id)
            .map(|place| rendered(&state, request, &found, &place))
            .ok_or_else(ApiResponse::not_found)?;
        Ok(ApiResponse::ok_typed(&comment))
    }
}

impl Handler for ops::IssuesUpdateComment {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = request.require_scope(&["repo", "public_repo"])?;
        let body = super::object_body(request)?;
        let text = comment_body(body.get("body"))?;
        let mut state = lock(state);
        let found = find_repo(&state, request)?;
        let id = super::comment_id(request)?;
        let place = locate(found.repo(&state), id).ok_or_else(ApiResponse::not_found)?;
        let before = rendered(&state, request, &found, &place);
        let author = before.user.as_ref().map(|user| user.login.clone());
        if !may_change(&found, author.as_deref(), &caller.login) {
            return Err(crate::errors::GithubError::AdminRequired.to_response());
        }
        let number = place_number(&place);
        let old_body = before.body.unwrap_or_default();
        write_body(found.repo_mut(&mut state), &place, text);
        let comment = rendered(&state, request, &found, &place);
        let mut ctx = super::webhook_ctx(&mut state, request, &found);
        events::issue_comment::edited(&mut ctx, number, &comment, &old_body)?;
        Ok(ApiResponse::ok_typed(&comment))
    }
}

impl Handler for ops::IssuesDeleteComment {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = request.require_scope(&["repo", "public_repo"])?;
        let mut state = lock(state);
        let found = find_repo(&state, request)?;
        let id = super::comment_id(request)?;
        let place = locate(found.repo(&state), id).ok_or_else(ApiResponse::not_found)?;
        let comment = rendered(&state, request, &found, &place);
        let author = comment.user.as_ref().map(|user| user.login.clone());
        if !may_change(&found, author.as_deref(), &caller.login) {
            return Err(crate::errors::GithubError::AdminRequired.to_response());
        }
        let number = place_number(&place);
        remove_comment(found.repo_mut(&mut state), &place);
        let mut ctx = super::webhook_ctx(&mut state, request, &found);
        events::issue_comment::deleted(&mut ctx, number, &comment)?;
        Ok(ApiResponse::no_content())
    }
}

impl Handler for ops::IssuesListCommentsForRepo {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let found = find_repo(&state, request)?;
        let since = match request.query("since") {
            Some(value) => Some(super::parse_timestamp(value, "Issue", "since")?),
            None => None,
        };
        let mut comments = gather(&state, request, &found);
        if let Some(since) = since {
            comments.retain(|comment| comment.updated_at > since);
        }
        sort_comments(request, &mut comments)?;
        Ok(paginate(request, &request.path, comments))
    }
}

fn comment_body(value: Option<&Value>) -> Result<String, ApiResponse> {
    match value {
        Some(Value::String(text)) => Ok(text.clone()),
        Some(_) => Err(ApiResponse::validation_failed(
            "IssueComment",
            "body",
            "invalid",
        )),
        None => Err(ApiResponse::validation_failed(
            "IssueComment",
            "body",
            "missing_field",
        )),
    }
}

fn may_change(found: &crate::handlers::repos::Found, author: Option<&str>, login: &str) -> bool {
    found.permission.admin || author.is_some_and(|author| author.eq_ignore_ascii_case(login))
}

fn locate(repo: &Repo, id: i64) -> Option<Place> {
    for (number, issue) in &repo.issues {
        if let Some(index) = u64::try_from(id)
            .ok()
            .and_then(|id| issue.comment_index(id))
        {
            return Some(Place::Issue {
                number: *number,
                index,
            });
        }
    }
    for (number, pull) in &repo.pull_requests {
        if let Some(index) = pull
            .issue_comments
            .iter()
            .position(|comment| comment.id == id)
        {
            return Some(Place::Pull {
                number: *number,
                index,
            });
        }
    }
    None
}

fn place_number(place: &Place) -> u64 {
    match place {
        Place::Issue { number, .. } | Place::Pull { number, .. } => *number,
    }
}

/// The comment at `place` as the REST API returns it.
fn rendered(
    state: &State,
    request: &ApiRequest,
    found: &Found,
    place: &Place,
) -> types::IssueComment {
    let context = super::context(state, request, found);
    match place {
        Place::Issue { number, index } => render::issue_comment(
            context,
            *number,
            &context.repo.issues[number].comments[*index],
        ),
        Place::Pull { number, index } => {
            context.repo.pull_requests[number].issue_comments[*index].clone()
        }
    }
}

fn write_body(repo: &mut Repo, place: &Place, text: String) {
    let now = crate::time::now_utc();
    match place {
        Place::Issue { number, index } => {
            let issue = repo.issues.get_mut(number).expect("comment located");
            issue.updated_at = now;
            let comment = &mut issue.comments[*index];
            comment.body = text;
            comment.updated_at = now;
        }
        Place::Pull { number, index } => {
            let pull = repo.pull_requests.get_mut(number).expect("comment located");
            if let Some(data) = &mut pull.data {
                data.updated_at = now;
            }
            let comment = &mut pull.issue_comments[*index];
            comment.body = Some(text);
            comment.updated_at = now;
        }
    }
}

fn remove_comment(repo: &mut Repo, place: &Place) {
    let now = crate::time::now_utc();
    match place {
        Place::Issue { number, index } => {
            let issue = repo.issues.get_mut(number).expect("comment located");
            issue.comments.remove(*index);
            issue.updated_at = now;
        }
        Place::Pull { number, index } => {
            let pull = repo.pull_requests.get_mut(number).expect("comment located");
            pull.issue_comments.remove(*index);
            if let Some(data) = &mut pull.data {
                data.comments = pull.issue_comments.len() as i64;
                data.updated_at = now;
            }
        }
    }
}

fn gather(state: &State, request: &ApiRequest, found: &Found) -> Vec<types::IssueComment> {
    let context = super::context(state, request, found);
    let repo = context.repo;
    let mut comments = Vec::new();
    for (number, issue) in &repo.issues {
        comments.extend(
            issue
                .comments
                .iter()
                .map(|comment| render::issue_comment(context, *number, comment)),
        );
    }
    for pull in repo.pull_requests.values() {
        comments.extend(pull.issue_comments.iter().cloned());
    }
    comments
}

fn sort_comments(
    request: &ApiRequest,
    comments: &mut [types::IssueComment],
) -> Result<(), ApiResponse> {
    // Without `sort`, comments are ordered by ascending ID.
    // Source: <https://docs.github.com/en/rest/issues/comments#list-issue-comments-for-a-repository>
    let Some(sort) = request.query("sort") else {
        comments.sort_by_key(|comment| comment.id);
        return Ok(());
    };
    if sort != "created" && sort != "updated" {
        return Err(ApiResponse::validation_failed("Issue", "sort", "invalid"));
    }
    let ascending = match request.query("direction") {
        None | Some("asc") => true,
        Some("desc") => false,
        Some(_) => {
            return Err(ApiResponse::validation_failed(
                "Issue",
                "direction",
                "invalid",
            ));
        }
    };
    comments.sort_by(|left, right| {
        let order = timestamp(left, sort)
            .cmp(&timestamp(right, sort))
            .then(left.id.cmp(&right.id));
        if ascending { order } else { order.reverse() }
    });
    Ok(())
}

fn timestamp(comment: &types::IssueComment, sort: &str) -> DateTime<Utc> {
    if sort == "updated" {
        comment.updated_at
    } else {
        comment.created_at
    }
}
