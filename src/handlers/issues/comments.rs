//! Issue comment read, update, delete, and the repository-wide comment list.
//!
//! Comments on issues and on pull requests are included. Review comments are not.
//! The author or a repository admin may edit or delete a comment. Push access is not enough.

use std::sync::Mutex;

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::super::Handler;
use crate::api::{ApiRequest, ApiResponse, ApiResult, paginate};
use crate::handlers::repos::find_repo;
use crate::operation::ops;
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
        let comment = locate(super::repo(&state, &found), id)
            .map(|place| comment_at(super::repo(&state, &found), &place).clone())
            .ok_or_else(ApiResponse::not_found)?;
        Ok(ApiResponse::typed(200, &comment))
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
        let place = locate(super::repo(&state, &found), id).ok_or_else(ApiResponse::not_found)?;
        let author = comment_at(super::repo(&state, &found), &place)
            .user
            .as_ref()
            .map(|user| user.login.clone());
        if !may_change(&found, author.as_deref(), &caller.login) {
            return Err(ApiResponse::error(
                403,
                "Must have admin rights to Repository.",
            ));
        }
        let number = place_number(&place);
        let old_body = comment_at(super::repo(&state, &found), &place)
            .body
            .clone()
            .unwrap_or_default();
        let comment = write_body(super::repo_mut(&mut state, &found), &place, text);
        let mut ctx = super::webhook_ctx(&mut state, request, &found);
        events::issue_comment::edited(&mut ctx, number, &comment, &old_body);
        Ok(ApiResponse::typed(200, &comment))
    }
}

impl Handler for ops::IssuesDeleteComment {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = request.require_scope(&["repo", "public_repo"])?;
        let mut state = lock(state);
        let found = find_repo(&state, request)?;
        let id = super::comment_id(request)?;
        let place = locate(super::repo(&state, &found), id).ok_or_else(ApiResponse::not_found)?;
        let author = comment_at(super::repo(&state, &found), &place)
            .user
            .as_ref()
            .map(|user| user.login.clone());
        if !may_change(&found, author.as_deref(), &caller.login) {
            return Err(ApiResponse::error(
                403,
                "Must have admin rights to Repository.",
            ));
        }
        let number = place_number(&place);
        let comment = comment_at(super::repo(&state, &found), &place).clone();
        remove_comment(super::repo_mut(&mut state, &found), &place);
        let mut ctx = super::webhook_ctx(&mut state, request, &found);
        events::issue_comment::deleted(&mut ctx, number, &comment);
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
        let mut comments = gather(super::repo(&state, &found));
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
        if let Some(index) = issue.comments.iter().position(|comment| comment.id == id) {
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

fn comment_at<'a>(repo: &'a Repo, place: &Place) -> &'a types::IssueComment {
    match place {
        Place::Issue { number, index } => &repo.issues[number].comments[*index],
        Place::Pull { number, index } => &repo.pull_requests[number].issue_comments[*index],
    }
}

fn write_body(repo: &mut Repo, place: &Place, text: String) -> types::IssueComment {
    let now = super::now();
    match place {
        Place::Issue { number, index } => {
            let issue = repo.issues.get_mut(number).expect("comment located");
            issue.data.updated_at = now;
            let comment = &mut issue.comments[*index];
            comment.body = Some(text);
            comment.updated_at = now;
            comment.clone()
        }
        Place::Pull { number, index } => {
            let pull = repo.pull_requests.get_mut(number).expect("comment located");
            if let Some(data) = &mut pull.data {
                data.updated_at = now;
            }
            let comment = &mut pull.issue_comments[*index];
            comment.body = Some(text);
            comment.updated_at = now;
            comment.clone()
        }
    }
}

fn remove_comment(repo: &mut Repo, place: &Place) {
    let now = super::now();
    match place {
        Place::Issue { number, index } => {
            let issue = repo.issues.get_mut(number).expect("comment located");
            issue.comments.remove(*index);
            issue.data.comments = issue.comments.len() as i64;
            issue.data.updated_at = now;
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

fn gather(repo: &Repo) -> Vec<types::IssueComment> {
    let mut comments = Vec::new();
    for issue in repo.issues.values() {
        comments.extend(issue.comments.iter().cloned());
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
