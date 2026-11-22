//! Merging, updating and reverting pull requests.

use async_graphql::Error;
use serde::Deserialize;

use super::issue_actions::webhook_err;
use super::pull_fields::owned_pull;
use super::types::PullRequestMergeMethod;
use super::{Env, Json, Map, MutationArgs, Output, read, typed_error};
use crate::api::{ApiResponse, Body};
use crate::handlers::pull_git::{self, LandMethod};
use crate::handlers::pulls::merge::{LandRequest, default_headline, land_pull};
use crate::state::lock;
use crate::webhooks::events::{self, Ctx};

/// Merge, update-branch and revert resolvers registered from this module.
pub(super) const RESOLVERS: &[(&str, super::Resolver)] = &[
    ("Mutation.mergePullRequest", merge_pull),
    ("Mutation.updatePullRequestBranch", update_branch),
    ("Mutation.revertPullRequest", revert_pull),
];

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MergeInput {
    pull_request_id: String,
    merge_method: Option<PullRequestMergeMethod>,
    commit_headline: Option<String>,
    commit_body: Option<String>,
    expected_head_oid: Option<String>,
    client_mutation_id: Option<String>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ClientPayload {
    #[serde(rename = "__typename")]
    typename: &'static str,
    client_mutation_id: Option<String>,
}

fn merge_pull(env: &Env, _: &Map, args: &Map) -> Result<Output, Error> {
    let MutationArgs { input } = read::<MutationArgs<MergeInput>>(args)?;
    let mut state = lock(&env.state);
    let (owner, name, number) = locate(&state, &input.pull_request_id)?;
    let data = state
        .account(&owner)
        .and_then(|account| account.repo(&name))
        .and_then(|repo| repo.pull_requests.get(&number))
        .and_then(|pull| pull.data.clone())
        .ok_or_else(super::errors::pull_not_found)?;
    let method = match input.merge_method {
        Some(PullRequestMergeMethod::Squash) => LandMethod::Squash,
        Some(PullRequestMergeMethod::Rebase) => LandMethod::Rebase,
        _ => LandMethod::Merge,
    };
    let headline = input
        .commit_headline
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| default_headline(method, &data, &owner));
    let body = input
        .commit_body
        .filter(|text| !text.is_empty())
        .or(data.body)
        .unwrap_or_default();
    land_pull(
        &mut state,
        &env.request(),
        &owner,
        &name,
        number,
        LandRequest {
            method,
            headline: &headline,
            body: &body,
            expected_head: input.expected_head_oid.as_deref(),
        },
    )
    .map_err(rest_error)?;
    Output::of(ClientPayload {
        typename: "MergePullRequestPayload",
        client_mutation_id: input.client_mutation_id,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateBranchInput {
    pull_request_id: String,
    expected_head_oid: Option<String>,
    client_mutation_id: Option<String>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct BranchPayload {
    #[serde(rename = "__typename")]
    typename: &'static str,
    client_mutation_id: Option<String>,
    pull_request: super::objects::PullRequest,
}

fn update_branch(env: &Env, _: &Map, args: &Map) -> Result<Output, Error> {
    let MutationArgs { input } = read::<MutationArgs<UpdateBranchInput>>(args)?;
    let mut state = lock(&env.state);
    let (owner, name, number) = locate(&state, &input.pull_request_id)?;
    let request = env.request();
    require_push(&state, env, &owner, &name)?;
    let actor = state
        .account(&env.caller.login)
        .map(|account| crate::render::simple_user(&request, account))
        .ok_or_else(|| super::errors::resolve_user(&env.caller.login))?;
    let (git_dir, head_ref, base_ref) = {
        let repo = state
            .account(&owner)
            .and_then(|account| account.repo(&name))
            .expect("located");
        let git_dir = repo
            .git_dir
            .clone()
            .ok_or_else(super::errors::no_git_storage_unprocessable)?;
        let data = repo
            .pull_requests
            .get(&number)
            .and_then(|pull| pull.data.as_ref())
            .ok_or_else(super::errors::pull_not_found)?;
        if data.state != crate::types::PullRequestState::Open {
            return Err(super::errors::pull_not_open());
        }
        (git_dir, data.head.ref_.clone(), data.base.ref_.clone())
    };
    let head = tip(&git_dir, &head_ref)?;
    let base = tip(&git_dir, &base_ref)?;
    if input
        .expected_head_oid
        .as_ref()
        .is_some_and(|sha| sha != &head)
    {
        return Err(super::errors::head_sha_not_current());
    }
    if ninot_common::git::new_commits(&git_dir, &base, &[&head])
        .map_err(|error| Error::new(error.to_string()))?
        .is_empty()
    {
        return Err(super::errors::no_new_commits());
    }
    let sha = pull_git::merge_base_into_head(&git_dir, &head, &base, &head_ref, &base_ref)
        .map_err(git_error)?;
    let moved = crate::handlers::timeline::branch_moved(
        &mut state,
        &owner,
        &name,
        &head_ref,
        &sha,
        crate::time::now_utc(),
        &actor,
    )
    .map_err(|error| Error::new(error.to_string()))?;
    let mut ctx = Ctx::new(&mut state, &request, &owner, &name);
    for item in &moved {
        events::pull_request::synchronize(&mut ctx, &item.pull, &item.before, &item.after, &actor)
            .map_err(webhook_err)?;
    }
    drop(state);
    Output::of(BranchPayload {
        typename: "UpdatePullRequestBranchPayload",
        client_mutation_id: input.client_mutation_id,
        pull_request: pull_by_id(env, &input.pull_request_id)?,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RevertInput {
    pull_request_id: String,
    title: Option<String>,
    body: Option<String>,
    draft: Option<bool>,
    client_mutation_id: Option<String>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct RevertPayload {
    #[serde(rename = "__typename")]
    typename: &'static str,
    client_mutation_id: Option<String>,
    pull_request: super::objects::PullRequest,
    revert_pull_request: super::objects::PullRequest,
}

fn revert_pull(env: &Env, _: &Map, args: &Map) -> Result<Output, Error> {
    let MutationArgs { input } = read::<MutationArgs<RevertInput>>(args)?;
    let mut state = lock(&env.state);
    let (owner, name, number) = locate(&state, &input.pull_request_id)?;
    require_push(&state, env, &owner, &name)?;
    let request = env.request();
    let (data, git_dir, repository_id, new_number) = {
        let repo = state
            .account(&owner)
            .and_then(|account| account.repo(&name))
            .expect("located");
        let data = repo
            .pull_requests
            .get(&number)
            .and_then(|pull| pull.data.clone())
            .ok_or_else(super::errors::pull_not_found)?;
        if !data.merged {
            return Err(super::errors::only_merged_can_revert());
        }
        let git_dir = repo
            .git_dir
            .clone()
            .ok_or_else(super::errors::no_git_storage_unprocessable)?;
        let repository_id = repo.id;
        let new_number = crate::handlers::issues::next_subject_number(repo);
        (data, git_dir, repository_id, new_number)
    };
    let merge_sha = data
        .merge_commit_sha
        .clone()
        .ok_or_else(super::errors::no_merge_commit)?;
    let base_tip = tip(&git_dir, &data.base.ref_)?;
    let title = input
        .title
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| format!("Revert \"{}\"", data.title));
    let body = input
        .body
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| format!("Reverts {owner}/{name}#{number}"));
    let message = format!("{title}\n\n{body}\n");
    let sha =
        pull_git::revert_commit(&git_dir, &base_tip, &merge_sha, &message).map_err(git_error)?;
    let head_ref = format!("revert-{number}-{}", data.head.ref_);
    ninot_common::git::create_ref(
        &git_dir,
        &format!("refs/heads/{head_ref}"),
        &sha,
        &crate::handlers::BOT,
    )
    .map_err(|error| Error::new(error.to_string()))?;
    let user = state
        .account(&env.caller.login)
        .map(|account| crate::render::simple_user(&request, account))
        .ok_or_else(|| super::errors::resolve_user(&env.caller.login))?;
    let mut created = data;
    let urls = request.urls().repo(&owner, &name);
    let pull_urls = urls.pull(new_number);
    let now = crate::time::now_utc();
    created.number = crate::render::id(new_number);
    created.id = crate::render::id(new_number);
    created.node_id = crate::ids::pull_request_node_id(repository_id, new_number);
    created.title = title;
    created.body = Some(body);
    created.state = crate::types::PullRequestState::Open;
    created.draft = Some(input.draft.unwrap_or(false));
    created.merged = false;
    created.merged_at = None;
    created.merged_by = None;
    created.merge_commit_sha = None;
    created.closed_at = None;
    created.mergeable = Some(true);
    created.mergeable_state = "clean".to_owned();
    created.created_at = now;
    created.updated_at = now;
    created.comments = 0;
    created.user = user;
    created.html_url = pull_urls.html.clone();
    created.url = pull_urls.api.clone();
    created.issue_url = pull_urls.issue.api.clone();
    created.head.ref_ = head_ref.clone();
    created.head.label = format!("{owner}:{head_ref}");
    created.head.sha = sha;
    created.base.sha = base_tip;
    state.add_pull_request(&owner, &name, new_number, created.clone());
    let mut ctx = Ctx::new(&mut state, &request, &owner, &name);
    events::pull_request::opened(&mut ctx, &created).map_err(webhook_err)?;
    drop(state);
    Output::of(RevertPayload {
        typename: "RevertPullRequestPayload",
        client_mutation_id: input.client_mutation_id,
        pull_request: pull_by_id(env, &input.pull_request_id)?,
        revert_pull_request: pull_by_number(env, &owner, &name, new_number)?,
    })
}

fn pull_by_number(
    env: &Env,
    owner: &str,
    name: &str,
    number: u64,
) -> Result<super::objects::PullRequest, Error> {
    let mut parent = Map::new();
    let mut owner_map = Map::new();
    owner_map.insert("login".to_owned(), Json::String(owner.to_owned()));
    let mut repository = Map::new();
    repository.insert("name".to_owned(), Json::String(name.to_owned()));
    repository.insert("owner".to_owned(), Json::Object(owner_map));
    parent.insert("number".to_owned(), Json::from(number));
    parent.insert("repository".to_owned(), Json::Object(repository));
    owned_pull(env, &parent)
}

fn pull_by_id(env: &Env, id: &str) -> Result<super::objects::PullRequest, Error> {
    let state = lock(&env.state);
    let (owner, name, number) = locate(&state, id)?;
    drop(state);
    pull_by_number(env, &owner, &name, number)
}

fn locate(state: &crate::state::State, id: &str) -> Result<(String, String, u64), Error> {
    super::pulls::pull_location(state, id).ok_or_else(|| super::errors::resolve_node(id))
}

fn require_push(
    state: &crate::state::State,
    env: &Env,
    owner: &str,
    name: &str,
) -> Result<(), Error> {
    let permission = state
        .permission(Some(&env.caller), owner, name)
        .ok_or_else(super::errors::repository_not_found)?;
    if permission.push {
        Ok(())
    } else {
        Err(super::errors::pat_forbidden())
    }
}

fn tip(git_dir: &std::path::Path, branch: &str) -> Result<String, Error> {
    ninot_common::git::resolve_commit(git_dir, &format!("refs/heads/{branch}"))
        .map_err(|error| Error::new(error.to_string()))?
        .ok_or_else(|| super::errors::branch_missing(branch))
}

fn git_error(fail: pull_git::GitFail) -> Error {
    let conflict = matches!(fail, pull_git::GitFail::Conflict);
    let message = fail.text();
    if conflict {
        typed_error(message, "UNPROCESSABLE")
    } else {
        Error::new(message)
    }
}

fn rest_error(response: ApiResponse) -> Error {
    let message = match response.body {
        Body::Json(value) => value
            .get("message")
            .and_then(|message| message.as_str())
            .unwrap_or("request failed")
            .to_owned(),
        _ => "request failed".to_owned(),
    };
    let kind = super::errors::rest_kind(response.status);
    typed_error(message, kind)
}
