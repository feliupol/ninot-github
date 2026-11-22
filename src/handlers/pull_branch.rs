//! Whether a pull request is merged, and bringing its head branch up to date with the base.

#![expect(
    clippy::disallowed_types,
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    reason = "untyped JSON; remove when this module is typed"
)]

use std::sync::Mutex;

use serde_json::{Map, Value, json};

use super::Handler;
use super::repos::find_repo;
use crate::api::{ApiRequest, ApiResponse, ApiResult};
use crate::operation::ops;
use crate::state::{State, lock};
use crate::webhooks::events::{self, Ctx};
use ninot_common::git;

impl Handler for ops::PullsCheckIfMerged {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let found = find_repo(&state, request)?;
        let number = request
            .param("pull_number")
            .parse::<u64>()
            .map_err(|_| ApiResponse::not_found())?;
        let pull = state
            .account(&found.owner)
            .and_then(|account| account.repo(&found.name))
            .and_then(|repo| repo.pull_requests.get(&number))
            .and_then(|pull| pull.data.as_ref())
            .ok_or_else(ApiResponse::not_found)?;
        // Source: OpenAPI pulls/check-if-merged 204
        // Source: OpenAPI pulls/check-if-merged 404
        if pull.merged {
            Ok(ApiResponse::no_content())
        } else {
            Err(ApiResponse::not_found())
        }
    }
}

impl Handler for ops::PullsUpdateBranch {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = request.require_scope(&["repo", "public_repo"])?;
        let body = request.json_as::<Map<String, Value>>()?;
        let expected = match body.get("expected_head_sha") {
            None | Some(Value::Null) => None,
            Some(Value::String(sha)) => Some(sha.clone()),
            Some(_) => {
                return Err(ApiResponse::validation_failed(
                    "PullRequest",
                    "expected_head_sha",
                    "invalid",
                ));
            }
        };
        let mut state = lock(state);
        let found = find_repo(&state, request)?;
        // The status only. Source: OpenAPI pulls/update-branch 403
        if !found.permission.push {
            return Err(crate::errors::GithubError::PatForbidden.to_response());
        }
        let number = request
            .param("pull_number")
            .parse::<u64>()
            .map_err(|_| ApiResponse::not_found())?;
        let repo = state
            .account(&found.owner)
            .and_then(|account| account.repo(&found.name))
            .expect("find_repo found the repository");
        let git_dir = repo
            .git_dir
            .clone()
            .ok_or_else(|| ApiResponse::internal("no git store"))?;
        let data = repo
            .pull_requests
            .get(&number)
            .and_then(|pull| pull.data.as_ref())
            .ok_or_else(ApiResponse::not_found)?;
        if data.state != crate::types::PullRequestState::Open {
            return Err(crate::errors::GithubError::PullNotOpen.to_response());
        }
        if data.head.user.login != found.owner || data.base.user.login != found.owner {
            return Err(crate::errors::GithubError::HeadMustBeInRepository.to_response());
        }
        let (head_ref, base_ref) = (data.head.ref_.clone(), data.base.ref_.clone());
        let head = git::resolve_commit(&git_dir, &format!("refs/heads/{head_ref}"))
            .map_err(ApiResponse::internal)?
            .ok_or_else(|| crate::errors::GithubError::HeadBranchMissing.to_response())?;
        let base = git::resolve_commit(&git_dir, &format!("refs/heads/{base_ref}"))
            .map_err(ApiResponse::internal)?
            .ok_or_else(|| crate::errors::GithubError::BaseBranchMissing.to_response())?;
        // Source: <https://docs.github.com/en/rest/pulls/pulls#update-a-pull-request-branch>
        if expected.is_some_and(|sha| sha != head) {
            return Err(crate::errors::GithubError::HeadShaNotCurrent.to_response());
        }
        if git::new_commits(&git_dir, &base, &[&head])
            .map_err(ApiResponse::internal)?
            .is_empty()
        {
            return Err(crate::errors::GithubError::NoNewCommits.to_response());
        }
        let sha =
            super::pull_git::merge_base_into_head(&git_dir, &head, &base, &head_ref, &base_ref)
                .map_err(|fail| match fail {
                    // A content or tree conflict answers 422, matching `git merge-tree --write-tree` failing.
                    super::pull_git::GitFail::Conflict => {
                        crate::errors::GithubError::MergeConflictBaseHead.to_response()
                    }
                    other => ApiResponse::unprocessable(&other.text()),
                })?;
        let actor = state
            .account(&caller.login)
            .map(|account| crate::render::simple_user(request, account))
            .ok_or_else(ApiResponse::not_found)?;
        let moved = super::timeline::branch_moved(
            &mut state,
            &found.owner,
            &found.name,
            &head_ref,
            &sha,
            crate::time::now_utc(),
            &actor,
        )
        .map_err(ApiResponse::internal)?;
        let mut ctx = Ctx::new(&mut state, request, &found.owner, &found.name);
        for item in &moved {
            events::pull_request::synchronize(
                &mut ctx,
                &item.pull,
                &item.before,
                &item.after,
                &actor,
            )?;
        }
        Ok(ApiResponse::accepted_json(json!({
            "message": "Updating pull request branch.",
            "url": request.urls().repo(&found.owner, &found.name).pull(number).api,
        })))
    }
}
