//! Reviews, edits, closing and reopening pull requests.

use async_graphql::Error;
use serde::Deserialize;

use super::issue_actions::webhook_err;
use super::pull_fields::owned_pull;
use super::pulls::review_objects;
use super::types::PullRequestReviewEvent;
use super::{Env, Json, Map, MutationArgs, Output, not_found, read, typed_error};
use crate::api::{ApiResponse, Body};
use crate::handlers::Handler;
use crate::render::node_id;
use crate::state::lock;
use crate::webhooks::events::{self, Ctx};

/// Create, review, edit, close and reopen resolvers registered from this module.
pub(super) const RESOLVERS: &[(&str, super::Resolver)] = &[
    ("Mutation.createPullRequest", create_pull),
    ("Mutation.addPullRequestReview", add_review),
    ("Mutation.updatePullRequest", update_pull),
    ("Mutation.closePullRequest", close_pull),
    ("Mutation.reopenPullRequest", reopen_pull),
];

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateInput {
    repository_id: String,
    base_ref_name: String,
    head_ref_name: String,
    title: String,
    body: Option<String>,
    draft: Option<bool>,
    maintainer_can_modify: Option<bool>,
    client_mutation_id: Option<String>,
}

/// Opens a pull request through the REST create handler so both APIs share one model.
fn create_pull(env: &Env, _: &Map, args: &Map) -> Result<Output, Error> {
    let MutationArgs { input } = read::<MutationArgs<CreateInput>>(args)?;
    let state = lock(&env.state);
    let (owner, name) = super::issue_actions::repo_location(&state, &input.repository_id)
        .ok_or_else(|| not_found("Could not resolve to a Repository.".to_owned()))?;
    drop(state);
    let spec = crate::types::PullsCreateRequest {
        base: input.base_ref_name,
        body: input.body,
        draft: input.draft,
        head: input.head_ref_name,
        head_repo: None,
        issue: None,
        maintainer_can_modify: input.maintainer_can_modify,
        title: Some(input.title),
    };
    let mut request = env.request();
    request.params.insert("owner".to_owned(), owner.clone());
    request.params.insert("repo".to_owned(), name.clone());
    request.body = serde_json::to_vec(&spec)
        .map_err(|error| Error::new(error.to_string()))?
        .into();
    let response =
        crate::operation::ops::PullsCreate::handle(&env.state, &request).map_err(rest_error)?;
    let id = match response.body {
        crate::api::Body::Json(value) => value
            .get("node_id")
            .and_then(Json::as_str)
            .unwrap_or("")
            .to_owned(),
        _ => String::new(),
    };
    Output::of(PullPayload {
        typename: "CreatePullRequestPayload",
        client_mutation_id: input.client_mutation_id,
        pull_request: pull_by_id(env, &id)?,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReviewInput {
    pull_request_id: String,
    body: Option<String>,
    #[serde(rename = "commitOID")]
    commit_oid: Option<String>,
    event: Option<PullRequestReviewEvent>,
    client_mutation_id: Option<String>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ReviewPayload {
    #[serde(rename = "__typename")]
    typename: &'static str,
    client_mutation_id: Option<String>,
    pull_request_review: super::objects::Review,
}

fn add_review(env: &Env, _: &Map, args: &Map) -> Result<Output, Error> {
    let MutationArgs { input } = read::<MutationArgs<ReviewInput>>(args)?;
    let event = match input.event {
        Some(PullRequestReviewEvent::Approve) => "APPROVE",
        Some(PullRequestReviewEvent::RequestChanges) => "REQUEST_CHANGES",
        Some(PullRequestReviewEvent::Comment) => "COMMENT",
        Some(PullRequestReviewEvent::Dismiss) => {
            return Err(typed_error(
                "dismiss a review through its own mutation",
                "UNPROCESSABLE",
            ));
        }
        None => "PENDING",
    };
    if !matches!(event, "APPROVE" | "PENDING") && input.body.as_deref().unwrap_or("").is_empty() {
        return Err(typed_error(
            "body is required for this review event",
            "UNPROCESSABLE",
        ));
    }
    let mut state = lock(&env.state);
    let (owner, name, number) = locate(&state, &input.pull_request_id)?;
    visible(&state, env, &owner, &name)?;
    let request = env.request();
    let author = state
        .account(&env.caller.login)
        .map(|account| crate::render::simple_user(&request, account))
        .ok_or_else(|| {
            not_found(format!(
                "Could not resolve to a User '{}'",
                env.caller.login
            ))
        })?;
    let id = state.next_id();
    let review_state = match event {
        "APPROVE" => "APPROVED",
        "REQUEST_CHANGES" => "CHANGES_REQUESTED",
        "COMMENT" => "COMMENTED",
        _ => "PENDING",
    };
    let submitted = (event != "PENDING").then(crate::state::now);
    let head = {
        let account = state.account(&owner).expect("located");
        let repo = account.repo(&name).expect("located");
        let pull = repo.pull_requests.get(&number).expect("located");
        let data = pull
            .data
            .as_ref()
            .ok_or_else(|| not_found("pull request REST model does not exist".to_owned()))?;
        if matches!(event, "APPROVE" | "REQUEST_CHANGES")
            && data.user.login.eq_ignore_ascii_case(&env.caller.login)
        {
            return Err(typed_error(
                "A pull request author cannot approve or request changes on their own pull request",
                "UNPROCESSABLE",
            ));
        }
        data.head.sha.clone()
    };
    let urls = request.urls().repo(&owner, &name).pull(number);
    let html_url = urls.review_html(id);
    let review = crate::types::PullRequestReview {
        author_association: crate::types::AuthorAssociation::Owner,
        body: input.body.unwrap_or_default(),
        body_html: None,
        body_text: None,
        commit_id: Some(input.commit_oid.unwrap_or_else(|| head.clone())),
        html_url: html_url.clone(),
        id: crate::render::id(id),
        links: crate::types::PullRequestReviewLinks {
            html: crate::types::PullRequestReviewLinksHtml { href: html_url },
            pull_request: crate::types::PullRequestReviewLinksPullRequest {
                href: urls.api.clone(),
            },
        },
        node_id: node_id("PullRequestReview", id),
        pull_request_url: urls.api,
        state: review_state.to_owned(),
        submitted_at: submitted
            .as_ref()
            .map(|time| crate::render::timestamp(time)),
        user: Some(author.into()),
    };
    {
        let pull = state
            .account_mut(&owner)
            .and_then(|account| account.repo_mut(&name))
            .and_then(|repo| repo.pull_requests.get_mut(&number))
            .expect("located");
        if let Some(data) = pull.data.as_mut() {
            if let Some(time) = submitted {
                data.updated_at = crate::render::timestamp(&time);
            }
            if event != "PENDING" {
                data.requested_reviewers
                    .retain(|reviewer| !reviewer.login.eq_ignore_ascii_case(&env.caller.login));
            }
        }
        pull.reviews.push(review.clone());
    }
    let (pull_request_review, pull_data) = {
        let account = state.account(&owner).expect("located");
        let repo = account.repo(&name).expect("located");
        let pull = repo.pull_requests.get(&number).expect("located");
        let (mut reviews, _) = review_objects(env, account, repo, number, pull, &head)?;
        let stored = reviews
            .pop()
            .ok_or_else(|| Error::new("review was not stored"))?;
        (stored, pull.data.clone())
    };
    if let Some(pull_data) = pull_data {
        let mut ctx = Ctx::new(&mut state, &request, &owner, &name);
        events::pull_request_review::submitted(&mut ctx, &pull_data, &review)
            .map_err(webhook_err)?;
    }
    Output::of(ReviewPayload {
        typename: "AddPullRequestReviewPayload",
        client_mutation_id: input.client_mutation_id,
        pull_request_review,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateInput {
    pull_request_id: String,
    title: Option<String>,
    body: Option<String>,
    base_ref_name: Option<String>,
    client_mutation_id: Option<String>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct PullPayload {
    #[serde(rename = "__typename")]
    typename: &'static str,
    client_mutation_id: Option<String>,
    pull_request: super::objects::PullRequest,
}

fn update_pull(env: &Env, _: &Map, args: &Map) -> Result<Output, Error> {
    let MutationArgs { input } = read::<MutationArgs<UpdateInput>>(args)?;
    if input
        .title
        .as_ref()
        .is_some_and(|title| title.trim().is_empty())
    {
        return Err(typed_error("title is required", "UNPROCESSABLE"));
    }
    let mut state = lock(&env.state);
    let (owner, name, number) = locate(&state, &input.pull_request_id)?;
    let request = env.request();
    let _actor = actor_user(&state, env, &request)?;
    let (old_title, old_body, old_base, old_sha, base_sha) = {
        let repo = state
            .account(&owner)
            .and_then(|account| account.repo(&name))
            .expect("located");
        let data = repo
            .pull_requests
            .get(&number)
            .and_then(|pull| pull.data.as_ref())
            .expect("located");
        author_or_push(&state, env, &owner, &name, &data.user.login)?;
        let old = (
            data.title.clone(),
            data.body.clone(),
            data.base.ref_.clone(),
            data.base.sha.clone(),
        );
        let base_sha = input
            .base_ref_name
            .as_ref()
            .map(|branch| {
                let git_dir = repo
                    .git_dir
                    .as_deref()
                    .ok_or_else(|| typed_error("repository has no git storage", "UNPROCESSABLE"))?;
                git_tip(git_dir, branch)
            })
            .transpose()?;
        (old.0, old.1, old.2, old.3, base_sha)
    };
    let pull = state
        .account_mut(&owner)
        .and_then(|account| account.repo_mut(&name))
        .and_then(|repo| repo.pull_requests.get_mut(&number))
        .expect("located");
    let data = pull.data.as_mut().expect("located");
    if let Some(title) = input.title.clone() {
        data.title = title;
    }
    if let Some(body) = input.body.clone() {
        data.body = Some(body);
    }
    if let Some(branch) = input.base_ref_name.clone() {
        data.base.label = format!("{owner}:{branch}");
        data.base.ref_ = branch;
        if let Some(sha) = base_sha {
            data.base.sha = sha;
        }
    }
    data.updated_at = crate::time::now_utc();
    let data = pull.data.clone().expect("located");
    let mut changes = Map::new();
    if data.title != old_title {
        changes.insert("title".to_owned(), from_value(old_title));
    }
    if data.body != old_body {
        changes.insert("body".to_owned(), from_value(old_body.unwrap_or_default()));
    }
    if data.base.ref_ != old_base || data.base.sha != old_sha {
        let mut base = Map::new();
        base.insert("ref".to_owned(), from_value(old_base));
        base.insert("sha".to_owned(), from_value(old_sha));
        changes.insert("base".to_owned(), Json::Object(base));
    }
    let mut ctx = Ctx::new(&mut state, &request, &owner, &name);
    events::pull_request::edited(&mut ctx, &data, changes).map_err(webhook_err)?;
    drop(state);
    Output::of(PullPayload {
        typename: "UpdatePullRequestPayload",
        client_mutation_id: input.client_mutation_id,
        pull_request: pull_by_id(env, &input.pull_request_id)?,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdInput {
    pull_request_id: String,
    client_mutation_id: Option<String>,
}

fn close_pull(env: &Env, _: &Map, args: &Map) -> Result<Output, Error> {
    set_closed(env, args, true, "ClosePullRequestPayload")
}

fn reopen_pull(env: &Env, _: &Map, args: &Map) -> Result<Output, Error> {
    set_closed(env, args, false, "ReopenPullRequestPayload")
}

fn set_closed(env: &Env, args: &Map, close: bool, typename: &'static str) -> Result<Output, Error> {
    let MutationArgs { input } = read::<MutationArgs<IdInput>>(args)?;
    let mut state = lock(&env.state);
    let (owner, name, number) = locate(&state, &input.pull_request_id)?;
    let request = env.request();
    let actor = actor_user(&state, env, &request)?;
    let login = state
        .account(&owner)
        .and_then(|account| account.repo(&name))
        .and_then(|repo| repo.pull_requests.get(&number))
        .and_then(|pull| pull.data.as_ref())
        .map(|data| data.user.login.clone())
        .ok_or_else(|| not_found("pull request does not exist".to_owned()))?;
    author_or_push(&state, env, &owner, &name, &login)?;
    let event_id = state.next_id();
    let pull = state
        .account_mut(&owner)
        .and_then(|account| account.repo_mut(&name))
        .and_then(|repo| repo.pull_requests.get_mut(&number))
        .expect("located");
    pull.apply_event(crate::state::NewTimelineEvent {
        id: event_id,
        kind: if close {
            crate::state::TimelineEventKind::Closed
        } else {
            crate::state::TimelineEventKind::Reopened
        },
        actor,
        created_at: crate::time::now_utc(),
        commit_id: None,
        requested_reviewer: None,
        requested_team: None,
    })
    .map_err(|message| typed_error(message, "UNPROCESSABLE"))?;
    let data = pull.data.clone().expect("located");
    let mut ctx = Ctx::new(&mut state, &request, &owner, &name);
    let result = if close {
        events::pull_request::closed(&mut ctx, &data)
    } else {
        events::pull_request::reopened(&mut ctx, &data)
    };
    result.map_err(webhook_err)?;
    drop(state);
    Output::of(PullPayload {
        typename,
        client_mutation_id: input.client_mutation_id,
        pull_request: pull_by_id(env, &input.pull_request_id)?,
    })
}

fn pull_by_id(env: &Env, id: &str) -> Result<super::objects::PullRequest, Error> {
    let state = lock(&env.state);
    let (owner, name, number) = locate(&state, id)?;
    drop(state);
    let mut owner_map = Map::new();
    owner_map.insert("login".to_owned(), Json::String(owner));
    let mut repository = Map::new();
    repository.insert("name".to_owned(), Json::String(name));
    repository.insert("owner".to_owned(), Json::Object(owner_map));
    let mut parent = Map::new();
    parent.insert("number".to_owned(), Json::from(number));
    parent.insert("repository".to_owned(), Json::Object(repository));
    owned_pull(env, &parent)
}

fn locate(state: &crate::state::State, id: &str) -> Result<(String, String, u64), Error> {
    super::pulls::pull_location(state, id).ok_or_else(|| {
        not_found(format!(
            "Could not resolve to a node with the global id of '{id}'"
        ))
    })
}

fn visible(state: &crate::state::State, env: &Env, owner: &str, name: &str) -> Result<(), Error> {
    state
        .permission(Some(&env.caller), owner, name)
        .map(|_| ())
        .ok_or_else(|| typed_error("Resource not accessible by integration", "FORBIDDEN"))
}

fn author_or_push(
    state: &crate::state::State,
    env: &Env,
    owner: &str,
    name: &str,
    author: &str,
) -> Result<(), Error> {
    let permission = state
        .permission(Some(&env.caller), owner, name)
        .ok_or_else(|| typed_error("Resource not accessible by integration", "FORBIDDEN"))?;
    if permission.push || author.eq_ignore_ascii_case(&env.caller.login) {
        Ok(())
    } else {
        Err(typed_error(
            "Must have write access or be the pull request author",
            "FORBIDDEN",
        ))
    }
}

fn actor_user(
    state: &crate::state::State,
    env: &Env,
    request: &crate::api::ApiRequest,
) -> Result<crate::types::SimpleUser, Error> {
    state
        .account(&env.caller.login)
        .map(|account| crate::render::simple_user(request, account))
        .ok_or_else(|| {
            not_found(format!(
                "Could not resolve to a User '{}'",
                env.caller.login
            ))
        })
}

fn git_tip(git_dir: &std::path::Path, branch: &str) -> Result<String, Error> {
    ninot_common::git::resolve_commit(git_dir, &format!("refs/heads/{branch}"))
        .map_err(|error| Error::new(error.to_string()))?
        .ok_or_else(|| typed_error("base branch does not exist", "UNPROCESSABLE"))
}

fn rest_error(response: ApiResponse) -> Error {
    let message = match response.body {
        Body::Json(value) => value
            .get("message")
            .and_then(Json::as_str)
            .unwrap_or("request failed")
            .to_owned(),
        _ => "request failed".to_owned(),
    };
    let kind = match response.status {
        403 => "FORBIDDEN",
        404 => "NOT_FOUND",
        _ => "UNPROCESSABLE",
    };
    typed_error(message, kind)
}

fn from_value(text: String) -> Json {
    let mut map = Map::new();
    map.insert("from".to_owned(), Json::String(text));
    Json::Object(map)
}
