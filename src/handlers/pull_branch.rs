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

/// Merges `base` into `head` and returns the merge commit sha.
///
/// A content or tree conflict answers 422, matching `git merge-tree --write-tree` failing.
fn merge_into_head(
    git_dir: &std::path::Path,
    head: &str,
    base: &str,
    head_ref: &str,
    base_ref: &str,
) -> Result<String, ApiResponse> {
    use gix::bstr::ByteSlice;
    let repo = super::git_store::open_repo(git_dir)?;
    let head_id = gix::ObjectId::from_hex(head.as_bytes()).map_err(ApiResponse::internal)?;
    let base_id = gix::ObjectId::from_hex(base.as_bytes()).map_err(ApiResponse::internal)?;
    let labels = gix::merge::blob::builtin_driver::text::Labels {
        ancestor: Some("merge-base".as_bytes().as_bstr()),
        current: Some(head_ref.as_bytes().as_bstr()),
        other: Some(base_ref.as_bytes().as_bstr()),
    };
    let options = repo
        .tree_merge_options()
        .map_err(ApiResponse::internal)?
        .into();
    let mut outcome = repo
        .merge_commits(head_id, base_id, labels, options)
        .map_err(|_| ApiResponse::unprocessable("merge conflict between base and head"))?;
    if outcome.tree_merge.failed_on_first_unresolved_conflict
        || outcome
            .tree_merge
            .has_unresolved_conflicts(gix::merge::tree::TreatAsUnresolved::default())
    {
        return Err(ApiResponse::unprocessable(
            "merge conflict between base and head",
        ));
    }
    let tree = outcome
        .tree_merge
        .tree
        .write()
        .map_err(ApiResponse::internal)?
        .detach();
    let signature = gix::actor::Signature {
        name: super::BOT.name.into(),
        email: super::BOT.email.into(),
        time: gix::date::Time::now_local_or_utc(),
    };
    let commit = gix::objs::Commit {
        tree,
        parents: vec![head_id, base_id].into(),
        author: signature.clone(),
        committer: signature,
        encoding: None,
        message: format!("Merge branch '{base_ref}' into {head_ref}\n").into(),
        extra_headers: Vec::new(),
    };
    let sha = repo
        .write_object(&commit)
        .map_err(ApiResponse::internal)?
        .detach()
        .to_string();
    drop(repo);
    git::create_ref(
        git_dir,
        &format!("refs/heads/{head_ref}"),
        &sha,
        &super::BOT,
    )
    .map_err(ApiResponse::internal)?;
    Ok(sha)
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
        // Source: <https://docs.github.com/en/rest/pulls/pulls#update-a-pull-request-branch>
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
        let sha = merge_into_head(&git_dir, &head, &base, &head_ref, &base_ref)?;
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
        Ok(ApiResponse::json(
            202,
            json!({
                "message": "Updating pull request branch.",
                "url": request.urls().repo(&found.owner, &found.name).pull(number).api,
            }),
        ))
    }
}
