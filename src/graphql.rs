//! GitHub's GraphQL API, served from GitHub's published schema in `spec/schema.docs.graphql`.
//!
//! Every type, field, argument, enum and input of the schema exists. A field listed in
//! [`RESOLVERS`] is computed by its resolver; any other field reads the same-named key of its
//! parent's JSON object, and fails with "Not implemented by ninot-github: Type.field" when the
//! parent has no such key.
//!
//! Gaps against GitHub, from async-graphql's dynamic schema:
//! - It cannot import SDL, so [`build_schema`] converts the parsed document. Applied directives
//!   other than `@deprecated` (`@preview`, `@possibleTypes`, `@docsCategory`) are dropped, as
//!   are directive definitions.
//! - It rejects a field narrower than its interface's, so such a field takes the interface's
//!   type: `User.email` is `String`, not `String!`.
//! - Custom scalars such as `DateTime` and `URI` accept and emit any value.
//! - A failed non-null field is left out of its object instead of nulling the object.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex, OnceLock};

use async_graphql::dynamic::{
    Enum, EnumItem, Field, FieldFuture, FieldValue, InputObject, InputValue, Interface,
    InterfaceField, Object, ResolverContext, Scalar, Schema, TypeRef, Union,
};
use async_graphql::extensions::{
    Extension, ExtensionContext, ExtensionFactory, NextResolve, ResolveInfo,
};
use async_graphql::parser::parse_schema;
use async_graphql::parser::types::{
    BaseType, ConstDirective, FieldDefinition, InputValueDefinition, Type, TypeKind,
    TypeSystemDefinition,
};
use async_graphql::{
    Error, ErrorExtensions, PathSegment, Positioned, QueryPathSegment, ServerResult,
};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value as Json, json};
use winnow::Parser;

use crate::api::{ApiResponse, Caller};
use crate::handlers::repos::init_storage;
use crate::render::{node_id, web_host};
use crate::server::Hit;
use crate::state::{Account, AccountKind, Repo, State, lock};
use ninot_common::git;

mod projects;

/// GitHub's public GraphQL schema, pinned by `scripts/update-graphql-spec.sh`.
const SDL: &str = include_str!("../spec/schema.docs.graphql");

/// A JSON object: a parent value or a field's arguments.
type Map = serde_json::Map<String, Json>;

/// Computes one field from the environment, the parent object and the field's arguments.
///
/// Objects in the returned JSON carry `__typename` and use the schema's field names.
type Resolver = fn(&Env, &Map, &Map) -> Result<Json, Error>;

/// The implemented fields, keyed by `Type.field`.
const RESOLVERS: &[(&str, Resolver)] = &[
    ("Query.viewer", viewer),
    ("Query.node", query_node),
    ("Query.repository", query_repository),
    ("Query.repositoryOwner", repository_owner),
    ("Query.user", user),
    ("Query.organization", organization),
    ("Query.search", search),
    ("Mutation.createRepository", create_repository),
    (
        "Mutation.markPullRequestReadyForReview",
        mark_pull_request_ready,
    ),
    (
        "Mutation.convertPullRequestToDraft",
        convert_pull_request_to_draft,
    ),
    ("Mutation.resolveReviewThread", resolve_review_thread),
    (
        "Mutation.addPullRequestReviewThreadReply",
        reply_to_review_thread,
    ),
    ("Mutation.addComment", add_comment),
    (
        "Mutation.updateProjectV2ItemFieldValue",
        update_project_item_field_value,
    ),
    ("User.repositories", repositories),
    ("Organization.repositories", repositories),
    ("User.repository", owner_repository),
    ("Organization.repository", owner_repository),
    ("Repository.isEmpty", is_empty),
    ("Repository.defaultBranchRef", default_branch_ref),
    ("Repository.viewerPermission", viewer_permission),
    ("Repository.autoMergeAllowed", auto_merge_allowed),
    ("Repository.pullRequest", pull_request),
    (
        "Mutation.enablePullRequestAutoMerge",
        enable_pull_request_auto_merge,
    ),
    (
        "Mutation.disablePullRequestAutoMerge",
        disable_pull_request_auto_merge,
    ),
];

/// What resolvers can see: the state, the authenticated caller and the base URL.
struct Env {
    state: Arc<Mutex<State>>,
    caller: Caller,
    web_url: String,
}

/// GitHub's error for a missing object, with its `NOT_FOUND` type.
fn not_found(message: String) -> Error {
    typed_error(message, "NOT_FOUND")
}

/// An error that GitHub reports with a top-level `type`.
fn typed_error(message: impl Into<String>, kind: &'static str) -> Error {
    Error::new(message).extend_with(|_, extensions| extensions.set("type", kind))
}

/// A string argument the schema declares non-null.
fn string_arg<'a>(args: &'a Map, name: &str) -> &'a str {
    args.get(name).and_then(Json::as_str).unwrap_or_default()
}

/// The `login` of an account object.
fn login(account: &Map) -> &str {
    account
        .get("login")
        .and_then(Json::as_str)
        .unwrap_or_default()
}

/// The owner login and name of a repository object.
fn full_name(repository: &Map) -> (&str, &str) {
    let owner = repository.get("owner").and_then(Json::as_object);
    let name = repository.get("name").and_then(Json::as_str);
    (owner.map_or("", login), name.unwrap_or_default())
}

/// A `User` or `Organization` object.
fn account_json(env: &Env, account: &Account) -> Json {
    let kind = match account.kind {
        AccountKind::User => "User",
        AccountKind::Organization => "Organization",
    };
    json!({
        "__typename": kind,
        "id": node_id(kind, account.id),
        "databaseId": account.id,
        "login": account.login,
        "name": null,
        "url": format!("{}/{}", env.web_url, account.login),
        "createdAt": account.created_at,
    })
}

/// A `Repository` object; fields that need git or permissions have resolvers.
fn repository_json(env: &Env, account: &Account, repo: &Repo) -> Json {
    let host = web_host(&env.web_url);
    json!({
        "__typename": "Repository",
        "id": node_id("Repository", repo.id),
        "databaseId": repo.id,
        "name": repo.name,
        "nameWithOwner": format!("{}/{}", account.login, repo.name),
        "owner": account_json(env, account),
        "description": repo.description,
        "url": format!("{}/{}/{}", env.web_url, account.login, repo.name),
        "sshUrl": format!("git@{host}:{}/{}.git", account.login, repo.name),
        "isPrivate": repo.private,
        "visibility": if repo.private { "PRIVATE" } else { "PUBLIC" },
        "isFork": false,
        "isArchived": repo.settings.archived,
        "isTemplate": repo.settings.is_template,
        "isMirror": false,
        "parent": null,
        "createdAt": repo.created_at,
        "updatedAt": repo.updated_at,
        "pushedAt": repo.pushed_at,
        "hasIssuesEnabled": repo.settings.has_issues,
        "hasWikiEnabled": repo.settings.has_wiki,
        "mergeCommitAllowed": repo.settings.allow_merge_commit,
        "rebaseMergeAllowed": repo.settings.allow_rebase_merge,
        "squashMergeAllowed": repo.settings.allow_squash_merge,
    })
}

/// The repository object if the caller can see it.
fn visible_repository(env: &Env, state: &State, owner: &str, name: &str) -> Option<Json> {
    state.permission(Some(&env.caller), owner, name)?;
    let account = state.account(owner)?;
    Some(repository_json(env, account, account.repo(name)?))
}

/// The account object with `login`, if it is of `kind`.
fn account_of_kind(env: &Env, login: &str, kind: AccountKind) -> Option<Json> {
    let state = lock(&env.state);
    state
        .account(login)
        .filter(|account| account.kind == kind)
        .map(|account| account_json(env, account))
}

/// `Query.viewer`.
fn viewer(env: &Env, _: &Map, _: &Map) -> Result<Json, Error> {
    account_of_kind(env, &env.caller.login, AccountKind::User)
        .ok_or_else(|| Error::new("viewer does not exist"))
}

/// `Query.repository`, following renames and transfers.
fn query_repository(env: &Env, _: &Map, args: &Map) -> Result<Json, Error> {
    let (owner, name) = (string_arg(args, "owner"), string_arg(args, "name"));
    let state = lock(&env.state);
    let (owner, name) = match state.redirect(owner, name) {
        Some((to_owner, to_name))
            if state
                .account(owner)
                .and_then(|account| account.repo(name))
                .is_none() =>
        {
            (to_owner.clone(), to_name.clone())
        }
        _ => (owner.to_owned(), name.to_owned()),
    };
    visible_repository(env, &state, &owner, &name).ok_or_else(|| {
        not_found(format!(
            "Could not resolve to a Repository with the name '{owner}/{name}'."
        ))
    })
}

/// `Query.repositoryOwner`.
fn repository_owner(env: &Env, _: &Map, args: &Map) -> Result<Json, Error> {
    let state = lock(&env.state);
    Ok(state
        .account(string_arg(args, "login"))
        .map_or(Json::Null, |account| account_json(env, account)))
}

/// `Query.user`.
fn user(env: &Env, _: &Map, args: &Map) -> Result<Json, Error> {
    let login = string_arg(args, "login");
    account_of_kind(env, login, AccountKind::User).ok_or_else(|| {
        not_found(format!(
            "Could not resolve to a User with the login of '{login}'."
        ))
    })
}

/// `Query.organization`.
fn organization(env: &Env, _: &Map, args: &Map) -> Result<Json, Error> {
    let login = string_arg(args, "login");
    account_of_kind(env, login, AccountKind::Organization).ok_or_else(|| {
        not_found(format!(
            "Could not resolve to an Organization with the login of '{login}'."
        ))
    })
}

/// `User.repository` and `Organization.repository`.
fn owner_repository(env: &Env, parent: &Map, args: &Map) -> Result<Json, Error> {
    let state = lock(&env.state);
    Ok(
        visible_repository(env, &state, login(parent), string_arg(args, "name"))
            .unwrap_or(Json::Null),
    )
}

/// `User.repositories` and `Organization.repositories`: the owner's repositories visible to the
/// caller, filtered by `privacy` and `isFork`, ordered by `orderBy` and paginated with opaque
/// cursors.
fn repositories(env: &Env, parent: &Map, args: &Map) -> Result<Json, Error> {
    let login = login(parent);
    let state = lock(&env.state);
    let mut repos: Vec<&Repo> = state
        .account(login)
        .map(|account| {
            account
                .repos()
                .filter(|repo| {
                    state
                        .permission(Some(&env.caller), login, &repo.name)
                        .is_some()
                })
                .collect()
        })
        .unwrap_or_default();
    match args.get("privacy").and_then(Json::as_str) {
        Some("PUBLIC") => repos.retain(|repo| !repo.private),
        Some("PRIVATE") => repos.retain(|repo| repo.private),
        _ => {}
    }
    if args.get("isFork") == Some(&Json::Bool(true)) {
        repos.clear();
    }
    if let Some(order) = args.get("orderBy").filter(|order| !order.is_null()) {
        let pushed = |repo: &Repo| {
            repo.pushed_at
                .clone()
                .unwrap_or_else(|| repo.created_at.clone())
        };
        match order["field"].as_str() {
            Some("NAME") => repos.sort_by_key(|repo| repo.name.to_lowercase()),
            Some("CREATED_AT") => {
                repos.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)))
            }
            Some("UPDATED_AT") => {
                repos.sort_by(|a, b| a.updated_at.cmp(&b.updated_at).then(a.id.cmp(&b.id)))
            }
            _ => repos.sort_by(|a, b| pushed(a).cmp(&pushed(b)).then(a.id.cmp(&b.id))),
        }
        if order["direction"] == "DESC" {
            repos.reverse();
        }
    }
    let total = repos.len();
    let start = args
        .get("after")
        .and_then(Json::as_str)
        .and_then(|cursor| STANDARD.decode(cursor).ok())
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .and_then(|text| grammar::cursor.parse(text.as_str()).ok())
        .map_or(0, |index| index + 1);
    let count = args
        .get("first")
        .and_then(Json::as_i64)
        .unwrap_or(30)
        .clamp(0, 100) as usize;
    let end = (start + count).min(total);
    let cursor = |index: usize| STANDARD.encode(format!("cursor:{index}"));
    let account = state.account(login);
    let nodes: Vec<Json> = repos
        .get(start..end)
        .unwrap_or_default()
        .iter()
        .filter_map(|repo| Some(repository_json(env, account?, repo)))
        .collect();
    Ok(json!({
        "__typename": "RepositoryConnection",
        "nodes": nodes,
        "totalCount": total,
        "pageInfo": {
            "__typename": "PageInfo",
            "hasNextPage": end < total,
            "hasPreviousPage": start > 0,
            "endCursor": (end > start).then(|| cursor(end - 1)),
            "startCursor": (end > start).then(|| cursor(start)),
        },
    }))
}

/// The stored repository behind a repository object.
fn stored_repo<'a>(state: &'a State, repository: &Map) -> Option<&'a Repo> {
    let (owner, name) = full_name(repository);
    state.account(owner)?.repo(name)
}

/// `Repository.isEmpty`.
fn is_empty(env: &Env, parent: &Map, _: &Map) -> Result<Json, Error> {
    let git_dir = stored_repo(&lock(&env.state), parent).and_then(|repo| repo.git_dir.clone());
    Ok(Json::Bool(git_dir.is_none_or(|git_dir| {
        git::is_empty(&git_dir).unwrap_or(true)
    })))
}

/// `Repository.defaultBranchRef`: null until the default branch exists.
fn default_branch_ref(env: &Env, parent: &Map, _: &Map) -> Result<Json, Error> {
    let Some((git_dir, default_branch)) = stored_repo(&lock(&env.state), parent)
        .and_then(|repo| Some((repo.git_dir.clone()?, repo.default_branch.clone())))
    else {
        return Ok(Json::Null);
    };
    let name = format!("refs/heads/{default_branch}");
    let exists = git::refs(&git_dir, &name)
        .is_ok_and(|refs| refs.into_iter().any(|found| found.name == name));
    Ok(if exists {
        json!({ "__typename": "Ref", "name": default_branch, "prefix": "refs/heads/" })
    } else {
        Json::Null
    })
}

/// `Repository.viewerPermission`.
fn viewer_permission(env: &Env, parent: &Map, _: &Map) -> Result<Json, Error> {
    let state = lock(&env.state);
    let (owner, name) = full_name(parent);
    let permission = state.permission(Some(&env.caller), owner, name);
    Ok(match permission {
        None => Json::Null,
        Some(permission) if permission.admin => json!("ADMIN"),
        Some(permission) if permission.push => json!("WRITE"),
        Some(_) => json!("READ"),
    })
}

/// `Repository.autoMergeAllowed`.
fn auto_merge_allowed(env: &Env, parent: &Map, _: &Map) -> Result<Json, Error> {
    let allowed = stored_repo(&lock(&env.state), parent)
        .ok_or_else(|| Error::new("repository does not exist"))?
        .settings
        .allow_auto_merge;
    Ok(Json::Bool(allowed))
}

/// Global id for a pull request, combining its repository id and number.
fn pull_id(repository_id: u64, number: u64) -> u64 {
    (repository_id << 32) | (number & u32::MAX as u64)
}

/// `Repository.pullRequest(number)`.
fn pull_request(env: &Env, parent: &Map, args: &Map) -> Result<Json, Error> {
    let (owner, name) = full_name(parent);
    let number = args
        .get("number")
        .and_then(Json::as_u64)
        .unwrap_or_default();
    let state = lock(&env.state);
    let repo = state
        .account(owner)
        .and_then(|account| account.repo(name))
        .ok_or_else(|| Error::new("repository does not exist"))?;
    if !repo.pull_requests.contains_key(&number) {
        return Err(Error::new("pull request does not exist"));
    }
    Ok(json!({
        "__typename": "PullRequest",
        "id": node_id("PullRequest", pull_id(repo.id, number)),
        "databaseId": pull_id(repo.id, number),
        "number": number,
        "repository": parent,
    }))
}

/// Finds the pull request encoded by a Relay global id.
fn pull_location(state: &State, encoded: &str) -> Option<(String, String, u64)> {
    let id = database_id(encoded)?;
    let repository_id = id >> 32;
    let number = id & u32::MAX as u64;
    state.accounts().find_map(|account| {
        account
            .repos()
            .find(|repo| repo.id == repository_id && repo.pull_requests.contains_key(&number))
            .map(|repo| (account.login.clone(), repo.name.clone(), number))
    })
}

/// Enables or disables auto-merge on the persisted pull request.
fn set_pull_request_auto_merge(env: &Env, args: &Map, enabled: bool) -> Result<Json, Error> {
    let input = args
        .get("input")
        .and_then(Json::as_object)
        .cloned()
        .unwrap_or_default();
    let encoded = input
        .get("pullRequestId")
        .and_then(Json::as_str)
        .unwrap_or_default();
    let mut state = lock(&env.state);
    let (owner, repo, number) = pull_location(&state, encoded)
        .ok_or_else(|| not_found("pull request does not exist".to_owned()))?;
    let pull = state
        .account_mut(&owner)
        .and_then(|account| account.repo_mut(&repo))
        .and_then(|repository| repository.pull_requests.get_mut(&number))
        .expect("pull location exists");
    let data = pull
        .data
        .as_mut()
        .ok_or_else(|| not_found("pull request REST model does not exist".to_owned()))?;
    let enabled_at = if enabled {
        let method = match input
            .get("mergeMethod")
            .and_then(Json::as_str)
            .unwrap_or("SQUASH")
            .to_ascii_uppercase()
            .as_str()
        {
            "MERGE" => crate::types::AutoMergeMergeMethod::Merge,
            "REBASE" => crate::types::AutoMergeMergeMethod::Rebase,
            _ => crate::types::AutoMergeMergeMethod::Squash,
        };
        let enabled_at = crate::render::timestamp(&crate::state::now());
        data.auto_merge = Some(crate::types::AutoMerge {
            commit_message: String::new(),
            commit_title: String::new(),
            enabled_by: data.user.clone(),
            merge_method: method,
        });
        pull.auto_merge_enabled_at = Some(enabled_at);
        Some(enabled_at)
    } else {
        data.auto_merge = None;
        pull.auto_merge_enabled_at = None;
        None
    };
    let auto_merge_request = enabled_at
        .map(|time| json!({"enabledAt":time}))
        .unwrap_or(Json::Null);
    Ok(json!({
        "__typename": if enabled { "EnablePullRequestAutoMergePayload" } else { "DisablePullRequestAutoMergePayload" },
        "pullRequest": {
            "__typename": "PullRequest",
            "autoMergeRequest": auto_merge_request,
        },
    }))
}

fn enable_pull_request_auto_merge(env: &Env, _: &Map, args: &Map) -> Result<Json, Error> {
    set_pull_request_auto_merge(env, args, true)
}

fn disable_pull_request_auto_merge(env: &Env, _: &Map, args: &Map) -> Result<Json, Error> {
    set_pull_request_auto_merge(env, args, false)
}

/// Resolves a node id against persisted pull requests and Projects v2 boards.
fn query_node(env: &Env, _: &Map, args: &Map) -> Result<Json, Error> {
    let id = string_arg(args, "id");
    let state = lock(&env.state);
    for account in state.accounts() {
        for repo in account.repos() {
            for (number, pull) in &repo.pull_requests {
                let expected = node_id("PullRequest", pull_id(repo.id, *number));
                if id == expected || pull.data.as_ref().is_some_and(|data| data.node_id == id) {
                    return Ok(graphql_pull_node(env, account, repo, *number, pull));
                }
            }
        }
    }
    if let Some(id) = database_id(id)
        && let Some(project) = state.projects.get(&id)
    {
        return Ok(graphql_project_node(&state, project));
    }
    Ok(Json::Null)
}

fn mark_pull_request_ready(env: &Env, _: &Map, args: &Map) -> Result<Json, Error> {
    set_pull_request_draft(env, args, false)
}

fn convert_pull_request_to_draft(env: &Env, _: &Map, args: &Map) -> Result<Json, Error> {
    set_pull_request_draft(env, args, true)
}

fn set_pull_request_draft(env: &Env, args: &Map, draft: bool) -> Result<Json, Error> {
    let input = args
        .get("input")
        .and_then(Json::as_object)
        .cloned()
        .unwrap_or_default();
    let id = string_arg(&input, "pullRequestId");
    let mut state = lock(&env.state);
    let actor = state.account(&env.caller.login).map(|account| {
        crate::render::simple_user(
            &crate::api::ApiRequest::for_urls(&env.web_url, None),
            account,
        )
    });
    let event_id = state.next_id();
    for account in state.accounts.values_mut() {
        for repo in account.repos.values_mut() {
            for (number, pull) in &mut repo.pull_requests {
                let expected = node_id("PullRequest", pull_id(repo.id, *number));
                if id == expected || pull.data.as_ref().is_some_and(|data| data.node_id == id) {
                    let flips = pull
                        .data
                        .as_ref()
                        .is_some_and(|data| data.draft.unwrap_or(false) != draft);
                    if flips {
                        let actor = actor.clone().ok_or_else(|| {
                            not_found(format!(
                                "Could not resolve to a User '{}'",
                                env.caller.login
                            ))
                        })?;
                        pull.apply_event(crate::state::TimelineEvent {
                            id: event_id,
                            kind: if draft {
                                crate::state::TimelineEventKind::ConvertToDraft
                            } else {
                                crate::state::TimelineEventKind::ReadyForReview
                            },
                            actor,
                            created_at: crate::render::timestamp(&crate::state::now()),
                            commit_id: None,
                            requested_reviewer: None,
                            requested_team: None,
                        })
                        .map_err(|message| typed_error(message, "UNPROCESSABLE"))?;
                    }
                    let typename = if draft {
                        "ConvertPullRequestToDraftPayload"
                    } else {
                        "MarkPullRequestReadyForReviewPayload"
                    };
                    return Ok(
                        json!({"__typename":typename,"pullRequest":{"__typename":"PullRequest","id":expected}}),
                    );
                }
            }
        }
    }
    Err(not_found(format!(
        "Could not resolve to a node with the global id of '{id}'"
    )))
}

fn resolve_review_thread(env: &Env, _: &Map, args: &Map) -> Result<Json, Error> {
    let input = args
        .get("input")
        .and_then(Json::as_object)
        .cloned()
        .unwrap_or_default();
    let id = string_arg(&input, "threadId");
    let mut state = lock(&env.state);
    for account in state.accounts.values_mut() {
        for repo in account.repos.values_mut() {
            for pull in repo.pull_requests.values_mut() {
                for thread in &mut pull.review_threads {
                    if thread.id == id {
                        thread.is_resolved = true;
                        return Ok(
                            json!({"__typename":"ResolveReviewThreadPayload","thread":{"__typename":"PullRequestReviewThread","id":id}}),
                        );
                    }
                }
            }
        }
    }
    Err(not_found(format!("Could not resolve review thread '{id}'")))
}

fn reply_to_review_thread(env: &Env, _: &Map, args: &Map) -> Result<Json, Error> {
    let input = args
        .get("input")
        .and_then(Json::as_object)
        .cloned()
        .unwrap_or_default();
    let id = string_arg(&input, "pullRequestReviewThreadId");
    let body = string_arg(&input, "body");
    let mut state = lock(&env.state);
    for account in state.accounts.values_mut() {
        for repo in account.repos.values_mut() {
            for (number, pull) in &mut repo.pull_requests {
                for (index, thread) in pull.review_threads.iter_mut().enumerate() {
                    if thread.id == id {
                        let comment_id = node_id(
                            "PullRequestReviewComment",
                            pull_id(repo.id, *number)
                                .wrapping_mul(1000)
                                .wrapping_add(index as u64 + 1),
                        );
                        thread.comments.push(crate::state::ReviewThreadComment {
                            id: comment_id.clone(),
                            body: body.to_owned(),
                            updated_at: crate::state::now(),
                        });
                        return Ok(
                            json!({"__typename":"AddPullRequestReviewThreadReplyPayload","comment":{"__typename":"PullRequestReviewComment","id":comment_id}}),
                        );
                    }
                }
            }
        }
    }
    Err(not_found(format!("Could not resolve review thread '{id}'")))
}

fn add_comment(env: &Env, _: &Map, args: &Map) -> Result<Json, Error> {
    let input = args
        .get("input")
        .and_then(Json::as_object)
        .cloned()
        .unwrap_or_default();
    let id = string_arg(&input, "subjectId");
    let body = string_arg(&input, "body");
    let mut state = lock(&env.state);
    for account in state.accounts.values_mut() {
        for repo in account.repos.values_mut() {
            for (number, pull) in &mut repo.pull_requests {
                let expected = node_id("PullRequest", pull_id(repo.id, *number));
                if id == expected || pull.data.as_ref().is_some_and(|data| data.node_id == id) {
                    let comment_id = node_id(
                        "IssueComment",
                        pull_id(repo.id, *number) + pull.issue_comments.len() as u64 + 1,
                    );
                    let Some(data) = pull.data.as_mut() else {
                        return Err(not_found(
                            "pull request REST model does not exist".to_owned(),
                        ));
                    };
                    let created_at = crate::render::timestamp(&crate::state::now());
                    let comment_number =
                        i64::try_from(pull.issue_comments.len() + 1).unwrap_or(i64::MAX);
                    pull.issue_comments.push(crate::types::IssueComment {
                        author_association: None,
                        body: Some(body.to_owned()),
                        body_html: None,
                        body_text: None,
                        created_at,
                        html_url: format!("{}#issuecomment-{comment_number}", data.html_url),
                        id: comment_number,
                        issue_url: data.issue_url.clone(),
                        minimized: None,
                        node_id: comment_id.clone(),
                        performed_via_github_app: None,
                        pin: None,
                        reactions: None,
                        updated_at: created_at,
                        url: format!("{}/comments/{comment_number}", data.issue_url),
                        user: None,
                    });
                    data.comments = pull.issue_comments.len() as i64;
                    return Ok(
                        json!({"__typename":"AddCommentPayload","commentEdge":{"node":{"__typename":"IssueComment","id":comment_id}}}),
                    );
                }
            }
        }
    }
    Err(not_found(format!(
        "Could not resolve comment subject '{id}'"
    )))
}

fn update_project_item_field_value(env: &Env, _: &Map, args: &Map) -> Result<Json, Error> {
    let input = args
        .get("input")
        .and_then(Json::as_object)
        .cloned()
        .unwrap_or_default();
    let project_id = string_arg(&input, "projectId");
    let item_id = string_arg(&input, "itemId");
    let field_id = string_arg(&input, "fieldId");
    let value = input["value"].as_object().cloned().unwrap_or_default();
    let option_id = string_arg(&value, "singleSelectOptionId");
    let (Some(project_id), Some(item_id), Some(field_id), Some(option_id)) = (
        database_id(project_id),
        database_id(item_id),
        database_id(field_id),
        database_id(option_id),
    ) else {
        return Err(not_found(
            "Could not resolve project item field value".to_owned(),
        ));
    };
    let mut state = lock(&env.state);
    let Some(project) = state.projects.get_mut(&project_id) else {
        return Err(not_found(format!(
            "Could not resolve project '{project_id}'"
        )));
    };
    let Some(item) = project.items.get_mut(&item_id) else {
        return Err(not_found(format!(
            "Could not resolve project item '{item_id}'"
        )));
    };
    if !project
        .fields
        .get(&field_id)
        .is_some_and(|field| field.options.contains_key(&option_id))
    {
        return Err(not_found(format!(
            "Could not resolve project option '{option_id}'"
        )));
    }
    item.single_select_values.insert(field_id, option_id);
    Ok(
        json!({"__typename":"UpdateProjectV2ItemFieldValuePayload","projectV2Item":{"__typename":"ProjectV2Item","id":node_id("ProjectV2Item",item_id)}}),
    )
}

fn graphql_project_node(state: &State, project: &crate::state::Project) -> Json {
    let fields: Vec<Json> = project.fields.values().map(|field| json!({
        "__typename":"ProjectV2SingleSelectField",
        "id":node_id("ProjectV2SingleSelectField",field.id),
        "name":field.name,
        "options":field.options.iter().map(|(id,name)| json!({"id":node_id("ProjectV2SingleSelectFieldOption",*id),"name":name})).collect::<Vec<_>>(),
    })).collect();
    let items: Vec<Json> = project.items.values().map(|item| {
        let issue_data = projects::issue_node(state, item);
        let field_values: Vec<Json> = item.single_select_values.iter().filter_map(|(field_id,option_id)| {
            let field = project.fields.get(field_id)?;
            let option = field.options.get(option_id)?;
            Some(json!({"__typename":"ProjectV2ItemFieldSingleSelectValue","id":node_id("ProjectV2ItemFieldSingleSelectValue", item.id.wrapping_shl(32).wrapping_add(*field_id)),"name":option,"optionId":node_id("ProjectV2SingleSelectFieldOption",*option_id),"field":{"__typename":"ProjectV2SingleSelectField","name":field.name}}))
        }).collect();
        json!({"__typename":"ProjectV2Item","id":node_id("ProjectV2Item",item.id),"type":"ISSUE","content":issue_data,"fieldValues":{"__typename":"ProjectV2ItemFieldValueConnection","nodes":field_values},"isArchived":false})
    }).collect();
    json!({
        "__typename":"ProjectV2",
        "id":node_id("ProjectV2",project.id),
        "databaseId":project.id,
        "title":project.title,
        "number":project.number,
        "closed":false,
        "fields":{"__typename":"ProjectV2FieldConnection","nodes":fields},
        "items":{"__typename":"ProjectV2ItemConnection","nodes":items,"pageInfo":{"hasNextPage":false,"hasPreviousPage":false,"startCursor":null,"endCursor":null}},
    })
}

/// `Query.search`, over pull requests and issues seeded in the real repository state.
fn search(env: &Env, _: &Map, args: &Map) -> Result<Json, Error> {
    let query = string_arg(args, "query");
    let tokens = search_tokens(query);
    let qualifiers: Vec<&str> = tokens.iter().map(String::as_str).collect();
    let repositories: Vec<&str> = qualifiers
        .iter()
        .filter_map(|part| part.strip_prefix("repo:"))
        .collect();
    let head = qualifiers
        .iter()
        .find_map(|part| part.strip_prefix("head:"));
    let author_me = qualifiers.contains(&"author:@me");
    let requested_me = qualifiers.contains(&"review-requested:@me");
    let want_open = qualifiers.contains(&"is:open");
    let want_pr = qualifiers.contains(&"is:pr");
    let text_terms: Vec<&str> = qualifiers
        .iter()
        .copied()
        .filter(|term| !term.contains(':'))
        .collect();
    let state = lock(&env.state);
    let mut nodes = Vec::new();
    for account in state.accounts() {
        for repo in account.repos() {
            let full_name = format!("{}/{}", account.login, repo.name);
            if !repositories.is_empty()
                && !repositories
                    .iter()
                    .any(|wanted| wanted.eq_ignore_ascii_case(&full_name))
                || state
                    .permission(Some(&env.caller), &account.login, &repo.name)
                    .is_none()
            {
                continue;
            }
            for (number, pull) in &repo.pull_requests {
                let Some(data) = pull.data.as_ref() else {
                    continue;
                };
                if want_open && data.state != crate::types::PullRequestState::Open {
                    continue;
                }
                if head.is_some_and(|branch| {
                    !data.head.ref_.eq_ignore_ascii_case(branch)
                        && !data.head.label.eq_ignore_ascii_case(branch)
                }) {
                    continue;
                }
                if !text_terms.iter().all(|term| {
                    data.title.to_lowercase().contains(&term.to_lowercase())
                        || data
                            .body
                            .as_deref()
                            .is_some_and(|body| body.to_lowercase().contains(&term.to_lowercase()))
                }) {
                    continue;
                }
                if author_me && !data.user.login.eq_ignore_ascii_case(&env.caller.login) {
                    continue;
                }
                if requested_me
                    && !crate::handlers::search::review_requested(account, data, &env.caller.login)
                {
                    continue;
                }
                nodes.push(graphql_pull_node(env, account, repo, *number, pull));
            }
            if !want_pr {
                for (number, issue) in &repo.issues {
                    if want_open && issue.data.state != "open" {
                        continue;
                    }
                    if !text_terms.iter().all(|term| {
                        issue
                            .data
                            .title
                            .to_lowercase()
                            .contains(&term.to_lowercase())
                            || issue.data.body.as_deref().is_some_and(|body| {
                                body.to_lowercase().contains(&term.to_lowercase())
                            })
                    }) {
                        continue;
                    }
                    if author_me
                        && !issue.data.user.as_ref().is_some_and(|author| {
                            author.login.eq_ignore_ascii_case(&env.caller.login)
                        })
                    {
                        continue;
                    }
                    let mut item =
                        serde_json::to_value(&issue.data).expect("generated issue DTO serializes");
                    item["__typename"] = json!("Issue");
                    item["number"] = json!(number);
                    item["repository"] =
                        json!({"__typename":"Repository", "nameWithOwner":full_name});
                    nodes.push(item);
                }
            }
        }
    }
    let total = nodes.len();
    let first = args
        .get("first")
        .and_then(Json::as_u64)
        .unwrap_or(30)
        .min(100) as usize;
    nodes.truncate(first);
    Ok(json!({
        "__typename": "SearchResultItemConnection",
        "nodes": nodes,
        "totalCount": total,
        "pageInfo": {"hasNextPage": total > first, "hasPreviousPage": false, "startCursor": null, "endCursor": null},
    }))
}

/// Splits GitHub search syntax into words while retaining quoted exact phrases as one token.
fn search_tokens(query: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut token = String::new();
    let mut quoted = false;
    for character in query.chars() {
        match character {
            '"' => quoted = !quoted,
            character if character.is_whitespace() && !quoted => {
                if !token.is_empty() {
                    tokens.push(std::mem::take(&mut token));
                }
            }
            character => token.push(character),
        }
    }
    if !token.is_empty() {
        tokens.push(token);
    }
    tokens
}

fn graphql_pull_node(
    env: &Env,
    account: &Account,
    repo: &Repo,
    number: u64,
    pull: &crate::state::PullRequest,
) -> Json {
    let Some(data) = pull.data.as_ref() else {
        return Json::Null;
    };
    let review_nodes: Vec<Json> = pull
        .reviews
        .iter()
        .enumerate()
        .map(|(index, review)| {
            let id = u64::try_from(review.id).unwrap_or(number * 100 + index as u64 + 1);
            let commit = review.commit_id.as_deref().unwrap_or(&data.head.sha);
            let author = review.user.as_ref().map(|user| json!({
                "login": user.login,
                "__typename": if user.type_ == "Bot" { "Bot" } else { "User" },
            })).unwrap_or(Json::Null);
            json!({"__typename":"PullRequestReview","id":node_id("PullRequestReview",id),"state":review.state,"body":review.body,"submittedAt":review.submitted_at,"commit":{"__typename":"Commit","oid":commit},"author":author})
        })
        .collect();
    let labels: Vec<Json> = data
        .labels
        .iter()
        .map(|label| json!({"name":label.name}))
        .collect();
    let reviewers: Vec<Json> = data
        .requested_reviewers
        .iter()
        .map(|user| json!({"requestedReviewer":{"__typename":if user.type_ == "Bot" {"Bot"} else {"User"}, "login":user.login}}))
        .collect();
    let threads: Vec<Json> = pull.review_threads.iter().map(|thread| {
        let comments: Vec<Json> = thread.comments.iter().map(|comment| {
            json!({"__typename":"PullRequestReviewComment","id":comment.id,"body":comment.body,"updatedAt":comment.updated_at})
        }).collect();
        json!({"__typename":"PullRequestReviewThread","id":thread.id,"isResolved":thread.is_resolved,"path":thread.path,"line":thread.line,"comments":{"__typename":"PullRequestReviewCommentConnection","nodes":comments,"totalCount":comments.len()}})
    }).collect();
    let sha = data.head.sha.as_str();
    let statuses = repo.commit_statuses.get(sha).cloned().unwrap_or_default();
    let status_contexts: Vec<Json> = statuses
        .iter()
        .map(|status| json!({"context":status.context, "state":status.state.to_string().to_ascii_uppercase()}))
        .collect();
    let rollup = if status_contexts.is_empty() {
        Json::Null
    } else if status_contexts
        .iter()
        .any(|status| status["state"] == "FAILURE" || status["state"] == "ERROR")
    {
        json!({"state":"FAILURE"})
    } else if status_contexts
        .iter()
        .all(|status| status["state"] == "SUCCESS")
    {
        json!({"state":"SUCCESS"})
    } else {
        json!({"state":"PENDING"})
    };
    let auto_merge = pull
        .auto_merge_enabled_at
        .as_ref()
        .map(|enabled_at| json!({"enabledAt":enabled_at}))
        .unwrap_or(Json::Null);
    let decision = pull.review_decision.clone().or_else(|| {
        if review_nodes
            .iter()
            .any(|review| review["state"] == "CHANGES_REQUESTED")
        {
            Some("CHANGES_REQUESTED".to_owned())
        } else if review_nodes
            .iter()
            .any(|review| review["state"] == "APPROVED")
        {
            Some("APPROVED".to_owned())
        } else {
            None
        }
    });
    // Report Git commit metadata when stored, or the seeded PR creation time otherwise.
    let (committed_at, parent_count) = repo
        .git_dir
        .as_ref()
        .and_then(|git_dir| git::commit(git_dir, sha).ok())
        .map_or_else(
            || (data.created_at.to_rfc3339(), 1),
            |commit| (commit.committer.date, commit.parents.len()),
        );
    let state = if data.merged {
        "MERGED".to_owned()
    } else {
        data.state.to_string().to_ascii_uppercase()
    };
    let commit_nodes = vec![
        json!({"__typename":"PullRequestCommit","commit":{"__typename":"Commit","oid":sha,"committedDate":committed_at,"parents":{"__typename":"CommitConnection","totalCount":parent_count},"status":{"contexts":status_contexts},"statusCheckRollup":rollup}}),
    ];
    let issue_comments: Vec<Json> = pull
        .issue_comments
        .iter()
        .map(|comment| json!({"__typename":"IssueComment","body":comment.body}))
        .collect();
    json!({
        "__typename":"PullRequest",
        "id":node_id("PullRequest", pull_id(repo.id, number)),
        "databaseId":pull_id(repo.id, number),
        "number":number,
        "title":data.title,
        "isDraft":data.draft.unwrap_or(false),
        "state":state,
        "url":data.html_url,
        "headRefName":data.head.ref_,
        "baseRefName":data.base.ref_,
        "mergeable":match data.mergeable {Some(true)=>"MERGEABLE", Some(false)=>"CONFLICTING", None=>"UNKNOWN"},
        "autoMergeRequest":auto_merge,
        "author":{"__typename":if data.user.type_ == "Bot" {"Bot"} else {"User"}, "login":data.user.login},
        "repository":repository_json(env, account, repo),
        "labels":{"__typename":"LabelConnection", "nodes":labels},
        "reviews":{"__typename":"PullRequestReviewConnection", "totalCount":review_nodes.len(), "nodes":review_nodes,"pageInfo":{"hasPreviousPage":false,"hasNextPage":false,"startCursor":null,"endCursor":null}},
        "reviewRequests":{"__typename":"ReviewRequestConnection", "totalCount":reviewers.len(), "nodes":reviewers},
        "reviewThreads":{"__typename":"PullRequestReviewThreadConnection", "nodes":threads,"totalCount":pull.review_threads.len(),"pageInfo":{"hasPreviousPage":false,"hasNextPage":false,"startCursor":null,"endCursor":null}},
        "reviewDecision":decision,
        "comments":{"__typename":"IssueCommentConnection","nodes":issue_comments,"totalCount":pull.issue_comments.len(),"pageInfo":{"hasPreviousPage":false,"hasNextPage":false,"startCursor":null,"endCursor":null}},
        "commits":{"__typename":"PullRequestCommitConnection", "nodes":commit_nodes},
        "repositoryUrl":format!("{}/{}",env.web_url,account.login),
    })
}

/// The numeric id inside a node id produced by [`node_id`].
pub(crate) fn database_id(node: &str) -> Option<u64> {
    let decoded = String::from_utf8(STANDARD.decode(node).ok()?).ok()?;
    grammar::node_payload.parse(decoded.as_str()).ok()
}

/// `Mutation.createRepository`, with GitHub's scope and permission checks.
fn create_repository(env: &Env, _: &Map, args: &Map) -> Result<Json, Error> {
    let input = args.get("input").cloned().unwrap_or_default();
    let name = input["name"].as_str().unwrap_or_default();
    let private = input["visibility"] != "PUBLIC";
    let scopes: &[&str] = if private {
        &["repo"]
    } else {
        &["repo", "public_repo"]
    };
    if !scopes.iter().any(|scope| env.caller.has_scope(scope)) {
        return Err(typed_error(
            "Your token has not been granted the required scopes to execute this query.",
            "INSUFFICIENT_SCOPES",
        ));
    }
    let mut state = lock(&env.state);
    let owner = match input["ownerId"].as_str() {
        None => env.caller.login.clone(),
        Some(owner_id) => {
            let unresolved = || {
                not_found(format!(
                    "Could not resolve to a node with the global id of '{owner_id}'"
                ))
            };
            let id = database_id(owner_id).ok_or_else(unresolved)?;
            state
                .accounts()
                .find(|account| account.id == id)
                .map(|account| account.login.clone())
                .ok_or_else(unresolved)?
        }
    };
    if !state
        .account(&owner)
        .is_some_and(|account| account.is_administered_by(&env.caller.login))
    {
        return Err(typed_error(
            format!(
                "{} does not have the correct permissions to execute `CreateRepository`",
                env.caller.login
            ),
            "FORBIDDEN",
        ));
    }
    let description = input["description"].as_str().map(str::to_owned);
    let repo = state
        .create_repo(&owner, name, private, description)
        .map_err(|_| typed_error("Name already exists on this account", "UNPROCESSABLE"))?;
    let storage = repo
        .git_dir
        .clone()
        .map(|git_dir| (git_dir, repo.default_branch.clone()));
    drop(state);
    if let Some((git_dir, branch)) = &storage {
        init_storage(&env.state, &owner, name, git_dir, branch, false)
            .map_err(|error| Error::new(error.to_string()))?;
    }
    let state = lock(&env.state);
    let repository = visible_repository(env, &state, &owner, name)
        .expect("the creator can see the repository it created");
    Ok(json!({
        "__typename": "CreateRepositoryPayload",
        "repository": repository,
        "clientMutationId": input["clientMutationId"],
    }))
}

/// How a field's named type turns JSON into a field value.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// A scalar or enum: the JSON is the value.
    Leaf,
    /// An object: the JSON object is the parent of its fields.
    Object,
    /// An interface or union: like [`Shape::Object`], typed by its `__typename`.
    Abstract,
}

/// Converts a resolved JSON value into a field value of the given shape.
fn field_value(value: Json, shape: Shape) -> Option<FieldValue<'static>> {
    match (value, shape) {
        (Json::Null, _) => None,
        (Json::Array(items), _) => {
            Some(FieldValue::list(items.into_iter().map(|item| {
                field_value(item, shape).unwrap_or(FieldValue::NULL)
            })))
        }
        (Json::Object(object), Shape::Object) => Some(FieldValue::owned_any(object)),
        (Json::Object(object), Shape::Abstract) => {
            let type_name = object
                .get("__typename")
                .and_then(Json::as_str)
                .unwrap_or_default()
                .to_owned();
            Some(FieldValue::owned_any(object).with_type(type_name))
        }
        (leaf, _) => Some(FieldValue::value(
            async_graphql::Value::from_json(leaf).expect("JSON converts to a GraphQL value"),
        )),
    }
}

/// Resolves the field `key` (`Type.field`) of the parent in `ctx`.
fn resolve(
    ctx: &ResolverContext<'_>,
    key: &str,
    shape: Shape,
    resolver: Option<Resolver>,
) -> Result<Option<FieldValue<'static>>, Error> {
    let empty = Map::new();
    let parent = ctx.parent_value.downcast_ref::<Map>().unwrap_or(&empty);
    let (_, name) = grammar::field_key.parse(key).unwrap_or(("", key));
    let value = match resolver {
        Some(resolver) => {
            let args: Map = ctx
                .args
                .iter()
                .map(|(argument, value)| {
                    let value = value.as_value().clone().into_json();
                    let value = value.expect("arguments parsed from JSON convert back to JSON");
                    (argument.to_string(), value)
                })
                .collect();
            resolver(ctx.data_unchecked::<Env>(), parent, &args)?
        }
        None => parent
            .get(name)
            .cloned()
            .ok_or_else(|| Error::new(format!("Not implemented by ninot-github: {key}")))?,
    };
    Ok(field_value(value, shape))
}

/// The dynamic type of `ty`.
fn type_ref(ty: &Type) -> TypeRef {
    let base = match &ty.base {
        BaseType::Named(name) => TypeRef::Named(name.to_string().into()),
        BaseType::List(item) => TypeRef::List(Box::new(type_ref(item))),
    };
    if ty.nullable {
        base
    } else {
        TypeRef::NonNull(Box::new(base))
    }
}

/// The named type inside any list and non-null wrappers of `ty`.
fn base_name(ty: &Type) -> &str {
    match &ty.base {
        BaseType::Named(name) => name.as_str(),
        BaseType::List(item) => base_name(item),
    }
}

/// The reason of a `@deprecated` directive: `None` when not deprecated.
fn deprecation(directives: &[Positioned<ConstDirective>]) -> Option<Option<String>> {
    directives
        .iter()
        .find(|directive| directive.node.name.node == "deprecated")
        .map(|directive| {
            directive
                .node
                .get_argument("reason")
                .and_then(|reason| match &reason.node {
                    async_graphql::Value::String(reason) => Some(reason.clone()),
                    _ => None,
                })
        })
}

/// Applies an optional description and deprecation to a dynamic item.
macro_rules! annotate {
    ($item:expr, $description:expr) => {
        match $description {
            Some(description) => $item.description(description.node),
            None => $item,
        }
    };
    ($item:expr, $description:expr, $directives:expr) => {{
        let item = annotate!($item, $description);
        match deprecation($directives) {
            Some(reason) => item.deprecation(reason.as_deref()),
            None => item,
        }
    }};
}

/// An argument or input field.
fn input_value(definition: InputValueDefinition) -> InputValue {
    let mut value = InputValue::new(definition.name.node.as_str(), type_ref(&definition.ty.node));
    if let Some(default) = definition.default_value {
        value = value.default_value(default.node);
    }
    annotate!(value, definition.description, &definition.directives)
}

/// An object field, resolved by `resolver` or read from the parent object.
fn object_field(
    type_name: &str,
    definition: FieldDefinition,
    shapes: &HashMap<String, Shape>,
    resolver: Option<Resolver>,
) -> Field {
    let key: Arc<str> = format!("{type_name}.{}", definition.name.node).into();
    let shape = shapes
        .get(base_name(&definition.ty.node))
        .copied()
        .unwrap_or(Shape::Leaf);
    let nullable = definition.ty.node.nullable;
    let mut field = Field::new(
        definition.name.node.as_str(),
        type_ref(&definition.ty.node),
        move |ctx| {
            let result = match resolve(&ctx, &key, shape, resolver) {
                // async-graphql drops a failed field from its object; GitHub makes it null.
                Err(error) if nullable => {
                    ctx.add_error(ctx.set_error_path(error.into_server_error(ctx.item.pos)));
                    Ok(None)
                }
                result => result,
            };
            FieldFuture::new(async move { result })
        },
    );
    for argument in definition.arguments {
        field = field.argument(input_value(argument.node));
    }
    annotate!(field, definition.description, &definition.directives)
}

/// An interface field.
fn interface_field(definition: FieldDefinition) -> InterfaceField {
    let mut field =
        InterfaceField::new(definition.name.node.as_str(), type_ref(&definition.ty.node));
    for argument in definition.arguments {
        field = field.argument(input_value(argument.node));
    }
    annotate!(field, definition.description, &definition.directives)
}

/// Builds the schema from [`SDL`], wiring [`RESOLVERS`] into their fields.
///
/// # Errors
///
/// Fails if the SDL does not parse or async-graphql rejects the resulting schema, or if a
/// resolver names a field the SDL lacks.
fn build_schema() -> Result<Schema, String> {
    let document = parse_schema(SDL).map_err(|error| error.to_string())?;
    let types: Vec<_> = document
        .definitions
        .into_iter()
        .filter_map(|definition| match definition {
            TypeSystemDefinition::Type(definition) => Some(definition.node),
            _ => None,
        })
        .collect();
    let shapes: HashMap<String, Shape> = types
        .iter()
        .filter_map(|definition| {
            let shape = match definition.kind {
                TypeKind::Object(_) => Shape::Object,
                TypeKind::Interface(_) | TypeKind::Union(_) => Shape::Abstract,
                _ => return None,
            };
            Some((definition.name.node.to_string(), shape))
        })
        .collect();
    let interface_fields: HashMap<(String, String), Type> = types
        .iter()
        .filter_map(|definition| match &definition.kind {
            TypeKind::Interface(interface) => Some((&definition.name.node, &interface.fields)),
            _ => None,
        })
        .flat_map(|(name, fields)| {
            fields.iter().map(|field| {
                let key = (name.to_string(), field.node.name.node.to_string());
                (key, field.node.ty.node.clone())
            })
        })
        .collect();
    let mut resolvers: HashMap<&str, Resolver> = RESOLVERS.iter().copied().collect();
    let mut builder = Schema::build("Query", Some("Mutation"), None);
    for definition in types {
        let name = definition.name.node.as_str();
        builder = match definition.kind {
            TypeKind::Scalar => {
                builder.register(annotate!(Scalar::new(name), definition.description))
            }
            TypeKind::Object(object) => {
                let mut dynamic = Object::new(name);
                for interface in &object.implements {
                    dynamic = dynamic.implement(interface.node.as_str());
                }
                for mut field in object.fields {
                    // async-graphql rejects a field narrower than its interface's, such as
                    // `User.email: String!` for `ProfileOwner.email: String`.
                    if let Some(ty) = object.implements.iter().find_map(|interface| {
                        interface_fields
                            .get(&(interface.node.to_string(), field.node.name.node.to_string()))
                    }) {
                        field.node.ty.node = ty.clone();
                    }
                    let resolver =
                        resolvers.remove(format!("{name}.{}", field.node.name.node).as_str());
                    dynamic = dynamic.field(object_field(name, field.node, &shapes, resolver));
                }
                builder.register(annotate!(dynamic, definition.description))
            }
            TypeKind::Interface(interface) => {
                let mut dynamic = Interface::new(name);
                for implemented in interface.implements {
                    dynamic = dynamic.implement(implemented.node.as_str());
                }
                for field in interface.fields {
                    dynamic = dynamic.field(interface_field(field.node));
                }
                builder.register(annotate!(dynamic, definition.description))
            }
            TypeKind::Union(union) => {
                let mut dynamic = Union::new(name);
                for member in union.members {
                    dynamic = dynamic.possible_type(member.node.as_str());
                }
                builder.register(annotate!(dynamic, definition.description))
            }
            TypeKind::Enum(enumeration) => {
                let mut dynamic = Enum::new(name);
                for value in enumeration.values {
                    let value = value.node;
                    dynamic = dynamic.item(annotate!(
                        EnumItem::new(value.value.node.as_str()),
                        value.description,
                        &value.directives
                    ));
                }
                builder.register(annotate!(dynamic, definition.description))
            }
            TypeKind::InputObject(input) => {
                let mut dynamic = InputObject::new(name);
                for field in input.fields {
                    dynamic = dynamic.field(input_value(field.node));
                }
                builder.register(annotate!(dynamic, definition.description))
            }
        };
    }
    if let Some(key) = resolvers.keys().next() {
        return Err(format!("resolver {key} names a field the schema lacks"));
    }
    builder
        .extension(ErrorPaths)
        .extension(RecordFields)
        .finish()
        .map_err(|error| error.to_string())
}

/// Sets the `path` of errors from non-null fields, which async-graphql's dynamic schema leaves
/// empty.
struct ErrorPaths;

impl ExtensionFactory for ErrorPaths {
    fn create(&self) -> Arc<dyn Extension> {
        Arc::new(ErrorPaths)
    }
}

#[async_graphql::async_trait::async_trait]
impl Extension for ErrorPaths {
    async fn resolve(
        &self,
        ctx: &ExtensionContext<'_>,
        info: ResolveInfo<'_>,
        next: NextResolve<'_>,
    ) -> ServerResult<Option<async_graphql::Value>> {
        let node = info.path_node;
        next.run(ctx, info).await.map_err(|mut error| {
            if error.path.is_empty() {
                let mut path: Vec<PathSegment> = std::iter::once(node)
                    .chain(node.parents())
                    .map(|node| match node.segment {
                        QueryPathSegment::Index(index) => PathSegment::Index(index),
                        QueryPathSegment::Name(name) => PathSegment::Field(name.to_owned()),
                    })
                    .collect();
                path.reverse();
                error.path = path;
            }
            error
        })
    }
}

/// The schema, built on first use and shared by every server in the process.
fn schema() -> &'static Schema {
    static SCHEMA: OnceLock<Schema> = OnceLock::new();
    SCHEMA.get_or_init(|| build_schema().expect("GitHub's GraphQL schema builds"))
}

/// Where [`RecordFields`] puts the fields a request resolved.
struct Hits(Arc<Mutex<BTreeSet<Hit>>>);

/// Records each field of an object or interface that resolves without error as a
/// [`Hit::GraphQl`], leaving out introspection and the items of lists.
struct RecordFields;

impl ExtensionFactory for RecordFields {
    fn create(&self) -> Arc<dyn Extension> {
        Arc::new(RecordFields)
    }
}

#[async_graphql::async_trait::async_trait]
impl Extension for RecordFields {
    async fn resolve(
        &self,
        ctx: &ExtensionContext<'_>,
        info: ResolveInfo<'_>,
        next: NextResolve<'_>,
    ) -> ServerResult<Option<async_graphql::Value>> {
        let recorded = !info.is_for_introspection
            && !info.name.starts_with("__")
            && !info.parent_type.starts_with('[');
        let field = recorded.then(|| format!("{}.{}", info.parent_type, info.name));
        let result = next.run(ctx, info).await;
        if let (Some(field), Ok(_), Some(Hits(hits))) = (field, &result, ctx.data_opt::<Hits>()) {
            hits.lock().unwrap().insert(Hit::GraphQl { field });
        }
        result
    }
}

/// Every field of the schema's object and interface types, as `Type.field`.
pub fn fields() -> Vec<String> {
    let query = r#"{"query": "{ __schema { types { name kind fields { name } } } }"}"#;
    let request: async_graphql::Request = serde_json::from_str(query).unwrap();
    let response = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("cannot start a runtime")
        .block_on(schema().execute(request));
    let body = serde_json::to_value(&response.data).unwrap();
    let mut fields = Vec::new();
    for kind in body["__schema"]["types"].as_array().into_iter().flatten() {
        let name = kind["name"].as_str().unwrap_or_default();
        if name.starts_with("__") || !matches!(kind["kind"].as_str(), Some("OBJECT" | "INTERFACE"))
        {
            continue;
        }
        for field in kind["fields"].as_array().into_iter().flatten() {
            fields.push(format!(
                "{name}.{}",
                field["name"].as_str().unwrap_or_default()
            ));
        }
    }
    fields.sort();
    fields
}

/// The `Type.field` of an `Unknown field "field" on type "Type".` validation error.
fn unknown_field(message: &str) -> Option<String> {
    let (type_name, field) = grammar::unknown_field(&mut &*message).ok()?;
    Some(format!("{type_name}.{field}"))
}

/// Executes one GraphQL request body and returns GitHub's response shape: errors carry their
/// `type` at the top level rather than under `extensions`.
pub fn execute(
    state: &Arc<Mutex<State>>,
    hits: &Arc<Mutex<BTreeSet<Hit>>>,
    caller: Option<Caller>,
    web_url: &str,
    body: &[u8],
) -> ApiResponse {
    let Some(caller) = caller else {
        return ApiResponse::error(401, "This endpoint requires you to be authenticated.");
    };
    let request: async_graphql::Request = match serde_json::from_slice(body) {
        Ok(request) => request,
        Err(error) => return ApiResponse::error(400, &format!("Problems parsing JSON: {error}")),
    };
    let request = request
        .data(Env {
            state: state.clone(),
            caller,
            web_url: web_url.to_owned(),
        })
        .data(Hits(hits.clone()));
    let response = tokio::runtime::Handle::current().block_on(schema().execute(request));
    // Errors without a path come from parsing or validation; resolvers' errors are answers.
    for error in response.errors.iter().filter(|error| error.path.is_empty()) {
        hits.lock().unwrap().insert(Hit::GraphQlError {
            field: unknown_field(&error.message),
            message: error.message.clone(),
        });
    }
    let mut body = serde_json::to_value(&response).unwrap_or(Json::Null);
    if let Some(errors) = body.get_mut("errors").and_then(Json::as_array_mut) {
        for error in errors {
            if let Some(kind) = error.pointer("/extensions/type").cloned() {
                error["type"] = kind;
            }
        }
    }
    ApiResponse::ok(body)
}

/// The grammars the resolvers and the error mapping parse, each as GitHub or async-graphql
/// writes it. None of them reports why a parse failed: every caller only asks whether the
/// text matched.
mod grammar {
    use std::str::FromStr;

    use winnow::ascii::digit1;
    use winnow::combinator::opt;
    use winnow::error::{ContextError, ErrMode, ModalResult};
    use winnow::prelude::*;
    use winnow::token::{literal, take, take_until, take_while};

    /// A decimal number as `usize::from_str` reads one: an optional `+`, then digits.
    fn decimal<T: FromStr>(input: &mut &str) -> ModalResult<T> {
        let (_, digits) = (opt('+'), digit1).parse_next(input)?;
        digits
            .parse()
            .map_err(|_| ErrMode::<ContextError>::Backtrack(ContextError::new()))
    }

    /// The offset of a `cursor:<offset>` pagination cursor.
    pub fn cursor(input: &mut &str) -> ModalResult<usize> {
        literal("cursor:").void().parse_next(input)?;
        decimal.parse_next(input)
    }

    /// The id in the `04:User42` payload of a node id made by [`node_id`](super::node_id).
    pub fn node_payload(input: &mut &str) -> ModalResult<u64> {
        let kind_length: usize = decimal.parse_next(input)?;
        (':'.void(), take(kind_length).void()).parse_next(input)?;
        decimal.parse_next(input)
    }

    /// The `Type` and `field` of a `Type.field` key, both GraphQL names.
    pub fn field_key<'s>(input: &mut &'s str) -> ModalResult<(&'s str, &'s str)> {
        let type_name = take_until(0.., '.').parse_next(input)?;
        '.'.void().parse_next(input)?;
        let field = take_while(1.., ('a'..='z', 'A'..='Z', '0'..='9', '_')).parse_next(input)?;
        Ok((type_name, field))
    }

    /// The type and field of an `Unknown field "field" on type "Type".` validation error,
    /// leaving the "Did you mean" suggestions async-graphql may append unparsed.
    pub fn unknown_field<'s>(input: &mut &'s str) -> ModalResult<(&'s str, &'s str)> {
        (
            (
                literal("Unknown field \"").void(),
                take_until(0.., '"'),
                '"'.void(),
            ),
            (
                literal(" on type \"").void(),
                take_until(0.., '"'),
                '"'.void(),
            ),
        )
            .map(|((_, field, _), (_, type_name, _))| (type_name, field))
            .parse_next(input)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_ids_round_trip() {
        assert_eq!(database_id(&node_id("User", 42)), Some(42));
        assert_eq!(database_id("MDQ6VXNlcjE="), Some(1));
        assert_eq!(database_id("garbage"), None);
    }

    #[test]
    fn node_ids_round_trip_for_kinds_ending_in_a_digit() {
        assert_eq!(database_id(&node_id("ProjectV2", 4)), Some(4));
        assert_eq!(database_id(&node_id("ProjectV2Item", 17)), Some(17));
    }

    #[test]
    fn github_schema_builds() {
        let started = std::time::Instant::now();
        build_schema().unwrap();
        eprintln!("built GitHub's GraphQL schema in {:?}", started.elapsed());
    }
}
