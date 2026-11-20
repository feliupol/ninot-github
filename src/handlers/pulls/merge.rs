//! `PUT /repos/{owner}/{repo}/pulls/{pull_number}/merge`.

use std::sync::Mutex;

use super::super::Handler;
use super::super::repos::find_repo;
use super::caller_user;
use crate::api::{ApiRequest, ApiResponse, ApiResult};
use crate::handlers::pull_git::{self, LandMethod};
use crate::operation::ops;
use crate::state::{NewTimelineEvent, State, TimelineEventKind, lock};
use crate::types;
use crate::webhooks::events::{self, Ctx};

/// How to land a pull request, shared by REST and GraphQL.
pub(crate) struct LandRequest<'a> {
    /// Merge, squash or rebase.
    pub method: LandMethod,
    /// Commit title.
    pub headline: &'a str,
    /// Commit body.
    pub body: &'a str,
    /// When set, the head sha must match or the merge answers 409.
    pub expected_head: Option<&'a str>,
}

/// Lands `owner/name#number` and returns the merged pull request.
///
/// # Errors
///
/// 403 without push, 404 when the pull request is missing, 409 when `expected_head` differs,
/// 405 when the pull request or the chosen method cannot merge.
pub(crate) fn land_pull(
    state: &mut State,
    request: &ApiRequest,
    owner: &str,
    name: &str,
    number: u64,
    spec: LandRequest<'_>,
) -> Result<types::PullRequest, ApiResponse> {
    let permission = state
        .permission(request.caller.as_ref(), owner, name)
        .ok_or_else(ApiResponse::not_found)?;
    // The status only. Source: OpenAPI pulls/merge 403
    if !permission.push {
        return Err(ApiResponse::error(
            403,
            "Resource not accessible by personal access token",
        ));
    }
    let (head, base_ref, git_dir) = {
        let repo = state
            .account(owner)
            .and_then(|account| account.repo(name))
            .ok_or_else(ApiResponse::not_found)?;
        let allowed = match spec.method {
            LandMethod::Merge => repo.settings.allow_merge_commit,
            LandMethod::Squash => repo.settings.allow_squash_merge,
            LandMethod::Rebase => repo.settings.allow_rebase_merge,
        };
        if !allowed {
            return Err(ApiResponse::error(405, "merge method is not allowed"));
        }
        let data = repo
            .pull_requests
            .get(&number)
            .and_then(|pull| pull.data.as_ref())
            .ok_or_else(ApiResponse::not_found)?;
        // Source: OpenAPI pulls/merge 409
        if spec.expected_head.is_some_and(|sha| data.head.sha != sha) {
            return Err(ApiResponse::error(409, "head SHA does not match"));
        }
        // Source: OpenAPI pulls/merge 405
        if data.state != types::PullRequestState::Open
            || data.mergeable == Some(false)
            || data.draft == Some(true)
        {
            return Err(ApiResponse::error(405, "pull request is not mergeable"));
        }
        (
            data.head.sha.clone(),
            data.base.ref_.clone(),
            repo.git_dir.clone(),
        )
    };
    let sha = match git_dir {
        Some(git_dir) => pull_git::land(
            &git_dir,
            spec.method,
            &base_ref,
            &head,
            spec.headline,
            spec.body,
        )
        .map_err(|fail| match fail {
            pull_git::GitFail::Conflict => ApiResponse::error(405, "pull request is not mergeable"),
            other => other.rest(),
        })?,
        None => head,
    };
    let login = request
        .caller
        .as_ref()
        .map(|caller| caller.login.as_str())
        .unwrap_or("");
    let actor = caller_user(state, request, login)?;
    let (merged_id, closed_id) = (state.next_id(), state.next_id());
    let now = crate::time::now_utc();
    let pull = state
        .account_mut(owner)
        .and_then(|account| account.repo_mut(name))
        .and_then(|repo| repo.pull_requests.get_mut(&number))
        .ok_or_else(ApiResponse::not_found)?;
    for (id, kind, commit_id) in [
        (merged_id, TimelineEventKind::Merged, Some(sha)),
        (closed_id, TimelineEventKind::Closed, None),
    ] {
        pull.apply_event(NewTimelineEvent {
            id,
            kind,
            actor: actor.clone(),
            created_at: now,
            commit_id,
            requested_reviewer: None,
            requested_team: None,
        })
        .map_err(|message| ApiResponse::error(405, &message))?;
    }
    let data = pull
        .data
        .clone()
        .ok_or_else(|| ApiResponse::internal("Pull request REST model is not seeded"))?;
    let mut ctx = Ctx::new(state, request, owner, name);
    events::pull_request::closed(&mut ctx, &data)?;
    Ok(data)
}

impl Handler for ops::PullsMerge {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        request.require_scope(&["repo", "public_repo"])?;
        let body: Option<types::PullsMergeRequestBody> = request.json_as()?;
        let body = body.unwrap_or_default();
        let mut state = lock(state);
        let found = find_repo(&state, request)?;
        let number = request
            .param("pull_number")
            .parse::<u64>()
            .map_err(|_| ApiResponse::not_found())?;
        let data = state
            .account(&found.owner)
            .and_then(|account| account.repo(&found.name))
            .and_then(|repo| repo.pull_requests.get(&number))
            .and_then(|pull| pull.data.clone())
            .ok_or_else(ApiResponse::not_found)?;
        let method = land_method(body.merge_method);
        let headline = body
            .commit_title
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| default_headline(method, &data, &found.owner));
        let message = body
            .commit_message
            .filter(|text| !text.is_empty())
            .or(data.body.clone())
            .unwrap_or_default();
        let merged = land_pull(
            &mut state,
            request,
            &found.owner,
            &found.name,
            number,
            LandRequest {
                method,
                headline: &headline,
                body: &message,
                expected_head: body.sha.as_deref(),
            },
        )?;
        let sha = merged.merge_commit_sha.clone().unwrap_or(merged.head.sha);
        Ok(ApiResponse::typed(
            200,
            &types::PullRequestMergeResult {
                merged: true,
                message: "Pull Request successfully merged".to_owned(),
                sha,
            },
        ))
    }
}

fn land_method(method: Option<types::PullsMergeRequestBodyMergeMethod>) -> LandMethod {
    match method {
        Some(types::PullsMergeRequestBodyMergeMethod::Squash) => LandMethod::Squash,
        Some(types::PullsMergeRequestBodyMergeMethod::Rebase) => LandMethod::Rebase,
        _ => LandMethod::Merge,
    }
}

/// Commit title when the request omits one: the pull request title, or the merge headline.
pub(crate) fn default_headline(
    method: LandMethod,
    data: &types::PullRequest,
    owner: &str,
) -> String {
    match method {
        LandMethod::Squash | LandMethod::Rebase => data.title.clone(),
        LandMethod::Merge => format!(
            "Merge pull request #{} from {owner}/{}",
            data.number, data.head.ref_
        ),
    }
}
