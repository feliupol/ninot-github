//! `PATCH /repos/{owner}/{repo}/pulls/{pull_number}`.

#![expect(
    clippy::disallowed_types,
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    reason = "untyped JSON; remove when this module is typed"
)]

use std::sync::Mutex;

use super::super::Handler;
use super::super::repos::find_repo;
use super::caller_user;
use crate::api::{ApiRequest, ApiResponse, ApiResult};
use crate::operation::ops;
use crate::state::{NewTimelineEvent, State, TimelineEventKind, lock};
use crate::types;
use crate::webhooks::events::{self, Ctx};

impl Handler for ops::PullsUpdate {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = request.require_scope(&["repo", "public_repo"])?;
        let body: types::PullsUpdateRequest = request.json_as()?;
        if body
            .title
            .as_ref()
            .is_some_and(|title| title.trim().is_empty())
        {
            return Err(ApiResponse::validation_failed(
                "PullRequest",
                "title",
                "missing_field",
            ));
        }
        let number = request
            .param("pull_number")
            .parse::<u64>()
            .map_err(|_| ApiResponse::not_found())?;
        let mut state = lock(state);
        let found = find_repo(&state, request)?;
        let repo = state
            .account(&found.owner)
            .and_then(|account| account.repo(&found.name))
            .ok_or_else(ApiResponse::not_found)?;
        let data = repo
            .pull_requests
            .get(&number)
            .and_then(|pull| pull.data.as_ref())
            .ok_or_else(ApiResponse::not_found)?;
        if !found.permission.push && !data.user.login.eq_ignore_ascii_case(&caller.login) {
            return Err(ApiResponse::error(
                403,
                "Must have write access or be the pull request author",
            ));
        }
        // The status only. Source: OpenAPI pulls/update 422
        if data.merged && matches!(body.state, Some(types::PullsUpdateRequestState::Open)) {
            return Err(ApiResponse::validation_failed(
                "PullRequest",
                "state",
                "invalid",
            ));
        }
        let old_title = data.title.clone();
        let old_body = data.body.clone();
        let old_base_ref = data.base.ref_.clone();
        let old_base_sha = data.base.sha.clone();
        let base_sha = body
            .base
            .as_ref()
            .map(|branch| {
                let git_dir = repo
                    .git_dir
                    .as_ref()
                    .ok_or_else(|| ApiResponse::internal("repository has no git storage"))?;
                ninot_common::git::resolve_commit(git_dir, &format!("refs/heads/{branch}"))
                    .map_err(ApiResponse::internal)?
                    .ok_or_else(|| ApiResponse::validation_failed("PullRequest", "base", "invalid"))
            })
            .transpose()?;
        let transition = match (&body.state, &data.state) {
            (Some(types::PullsUpdateRequestState::Open), types::PullRequestState::Closed) => {
                Some(TimelineEventKind::Reopened)
            }
            (Some(types::PullsUpdateRequestState::Closed), types::PullRequestState::Open) => {
                Some(TimelineEventKind::Closed)
            }
            _ => None,
        };
        let actor = caller_user(&state, request, &caller.login)?;
        let event_id = state.next_id();
        let pull = state
            .account_mut(&found.owner)
            .and_then(|account| account.repo_mut(&found.name))
            .and_then(|repo| repo.pull_requests.get_mut(&number))
            .ok_or_else(ApiResponse::not_found)?;
        let data = pull.data.as_mut().ok_or_else(ApiResponse::not_found)?;
        if let Some(title) = body.title {
            data.title = title;
        }
        if let Some(body) = body.body {
            data.body = Some(body);
        }
        if let Some(branch) = body.base {
            data.base.label = format!("{}:{branch}", found.owner);
            data.base.ref_ = branch;
            if let Some(sha) = base_sha {
                data.base.sha = sha;
            }
        }
        if let Some(allowed) = body.maintainer_can_modify {
            data.maintainer_can_modify = allowed;
        }
        let now = crate::time::now_utc();
        data.updated_at = now;
        if let Some(kind) = transition {
            pull.apply_event(NewTimelineEvent {
                id: event_id,
                kind,
                actor,
                created_at: now,
                commit_id: None,
                requested_reviewer: None,
                requested_team: None,
            })
            .map_err(|message| ApiResponse::unprocessable(&message))?;
        }
        let data = pull.data.clone().expect("checked above");
        let mut changes = serde_json::Map::new();
        if data.title != old_title {
            changes.insert("title".to_owned(), serde_json::json!({ "from": old_title }));
        }
        if data.body != old_body {
            changes.insert(
                "body".to_owned(),
                serde_json::json!({ "from": old_body.unwrap_or_default() }),
            );
        }
        if data.base.ref_ != old_base_ref || data.base.sha != old_base_sha {
            changes.insert(
                "base".to_owned(),
                serde_json::json!({
                    "ref": { "from": old_base_ref },
                    "sha": { "from": old_base_sha },
                }),
            );
        }
        let mut ctx = Ctx::new(&mut state, request, &found.owner, &found.name);
        match transition {
            Some(TimelineEventKind::Closed) => events::pull_request::closed(&mut ctx, &data)?,
            Some(TimelineEventKind::Reopened) => events::pull_request::reopened(&mut ctx, &data)?,
            _ => {}
        }
        events::pull_request::edited(&mut ctx, &data, changes)?;
        Ok(ApiResponse::typed(200, &types::PullsUpdateResponse(data)))
    }
}
