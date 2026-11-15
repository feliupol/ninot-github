//! Whether a pull request is merged, and bringing its head branch up to date with the base.

use std::process::Command;
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
        if pull.merged {
            Ok(ApiResponse::no_content())
        } else {
            Err(ApiResponse::not_found())
        }
    }
}

/// Runs `git` against `git_dir` with the user's and system's configuration ignored.
fn run(git_dir: &std::path::Path, args: &[&str]) -> std::io::Result<std::process::Output> {
    Command::new("git")
        .arg("--git-dir")
        .arg(git_dir)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", super::BOT.name)
        .env("GIT_AUTHOR_EMAIL", super::BOT.email)
        .env("GIT_COMMITTER_NAME", super::BOT.name)
        .env("GIT_COMMITTER_EMAIL", super::BOT.email)
        .stdin(std::process::Stdio::null())
        .output()
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
        if !found.permission.push {
            return Err(ApiResponse::error(
                403,
                "Resource not accessible by personal access token",
            ));
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
            return Err(ApiResponse::unprocessable("pull request is not open"));
        }
        if data.head.user.login != found.owner || data.base.user.login != found.owner {
            return Err(ApiResponse::unprocessable(
                "only pull requests whose head branch is in this repository can be updated",
            ));
        }
        let (head_ref, base_ref) = (data.head.ref_.clone(), data.base.ref_.clone());
        let head = git::resolve_commit(&git_dir, &format!("refs/heads/{head_ref}"))
            .map_err(ApiResponse::internal)?
            .ok_or_else(|| ApiResponse::unprocessable("head branch does not exist"))?;
        let base = git::resolve_commit(&git_dir, &format!("refs/heads/{base_ref}"))
            .map_err(ApiResponse::internal)?
            .ok_or_else(|| ApiResponse::unprocessable("base branch does not exist"))?;
        if expected.is_some_and(|sha| sha != head) {
            return Err(ApiResponse::unprocessable(
                "expected head sha didn't match current head ref.",
            ));
        }
        if git::new_commits(&git_dir, &base, &[&head])
            .map_err(ApiResponse::internal)?
            .is_empty()
        {
            return Err(ApiResponse::unprocessable(
                "There are no new commits on the base branch.",
            ));
        }
        let merged = run(&git_dir, &["merge-tree", "--write-tree", &head, &base])
            .map_err(ApiResponse::internal)?;
        if !merged.status.success() {
            return Err(ApiResponse::unprocessable(
                "merge conflict between base and head",
            ));
        }
        let output = String::from_utf8_lossy(&merged.stdout);
        let tree = output.lines().next().unwrap_or_default().trim().to_owned();
        let message = format!("Merge branch '{base_ref}' into {head_ref}");
        let commit = run(
            &git_dir,
            &[
                "commit-tree",
                &tree,
                "-p",
                &head,
                "-p",
                &base,
                "-m",
                &message,
            ],
        )
        .map_err(ApiResponse::internal)?;
        if !commit.status.success() {
            return Err(ApiResponse::internal(
                String::from_utf8_lossy(&commit.stderr).trim(),
            ));
        }
        let sha = String::from_utf8_lossy(&commit.stdout).trim().to_owned();
        git::create_ref(
            &git_dir,
            &format!("refs/heads/{head_ref}"),
            &sha,
            &super::BOT,
        )
        .map_err(ApiResponse::internal)?;
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
            );
        }
        Ok(ApiResponse::json(
            202,
            json!({
                "message": "Updating pull request branch.",
                "url": format!("{}/repos/{}/{}/pulls/{number}", request.api_url, found.owner, found.name),
            }),
        ))
    }
}
