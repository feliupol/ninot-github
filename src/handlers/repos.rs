//! Repository lifecycle: create, read, list, update, transfer and delete.

use std::path::Path;
use std::sync::Mutex;

use winnow::Parser as _;
use winnow::error::ContextError;
use winnow::token::take_while;

use super::Handler;
use crate::api::{ApiRequest, ApiResponse, ApiResult, DOCS_URL, paginate};
use crate::operation::ops;
use crate::render;
use crate::state::{AccountKind, Permission, Settings, State, lock};
use crate::types;
use ninot_common::git;

/// A repository the caller can see, under its current owner and name.
pub struct Found {
    pub owner: String,
    pub name: String,
    pub permission: Permission,
}

/// Resolves `{owner}/{repo}` for the caller: invisible repositories are 404, and renamed or
/// transferred ones answer 301 with their new location, as GitHub does.
pub fn find_repo(state: &State, request: &ApiRequest) -> Result<Found, ApiResponse> {
    let (owner, name) = (request.param("owner"), request.param("repo"));
    let caller = request.caller.as_ref();
    if let Some(permission) = state.permission(caller, owner, name) {
        let account = state.account(owner).unwrap();
        return Ok(Found {
            owner: account.login.clone(),
            name: account.repo(name).unwrap().name.clone(),
            permission,
        });
    }
    if let Some((to_owner, to_name)) = state.redirect(owner, name)
        && state.permission(caller, to_owner, to_name).is_some()
    {
        let rest: String = request
            .path
            .split('/')
            .skip(4)
            .map(|segment| format!("/{segment}"))
            .collect();
        let location = format!("{}/repos/{to_owner}/{to_name}{rest}", request.api_url);
        let body = types::BasicError {
            documentation_url: Some(DOCS_URL.to_owned()),
            message: Some("Moved Permanently".to_owned()),
            status: None,
            url: Some(location.clone()),
        };
        return Err(ApiResponse::typed(301, &body).with_header("location", location));
    }
    Err(ApiResponse::not_found())
}

/// Like [`find_repo`], but the caller must administer the repository.
fn find_admin_repo(state: &State, request: &ApiRequest) -> Result<Found, ApiResponse> {
    let found = find_repo(state, request)?;
    if found.permission.admin {
        Ok(found)
    } else {
        Err(ApiResponse::error(
            403,
            "Must have admin rights to Repository.",
        ))
    }
}

fn render_found(state: &State, request: &ApiRequest, found: &Found) -> types::FullRepository {
    let account = state.account(&found.owner).unwrap();
    render::full_repository(
        request,
        account,
        account.repo(&found.name).unwrap(),
        found.permission,
    )
}

/// Applies only the settings explicitly supplied by a typed repository update.
fn updated_settings(current: &Settings, body: &types::ReposUpdateRequest) -> Settings {
    let mut settings = current.clone();
    if let Some(homepage) = &body.homepage {
        settings.homepage = homepage.clone();
    }
    if let Some(value) = body.has_issues {
        settings.has_issues = value;
    }
    if let Some(value) = body.has_projects {
        settings.has_projects = value;
    }
    if let Some(value) = body.has_wiki {
        settings.has_wiki = value;
    }
    if let Some(value) = body.has_pull_requests {
        settings.has_pull_requests = value;
    }
    if let Some(value) = body.is_template {
        settings.is_template = value;
    }
    if let Some(value) = body.allow_squash_merge {
        settings.allow_squash_merge = value;
    }
    if let Some(value) = body.allow_merge_commit {
        settings.allow_merge_commit = value;
    }
    if let Some(value) = body.allow_rebase_merge {
        settings.allow_rebase_merge = value;
    }
    if let Some(value) = body.allow_auto_merge {
        settings.allow_auto_merge = value;
    }
    if let Some(value) = body.delete_branch_on_merge {
        settings.delete_branch_on_merge = value;
    }
    if let Some(value) = body.allow_update_branch {
        settings.allow_update_branch = value;
    }
    if let Some(value) = body.use_squash_pr_title_as_default {
        settings.use_squash_pr_title_as_default = value;
    }
    if let Some(value) = body.archived {
        settings.archived = value;
    }
    if let Some(value) = body.allow_forking {
        settings.allow_forking = value;
    }
    if let Some(value) = body.web_commit_signoff_required {
        settings.web_commit_signoff_required = value;
    }
    if let Some(value) = body.pull_request_creation_policy {
        settings.pull_request_creation_policy = value.into();
    }
    if let Some(value) = body.squash_merge_commit_title {
        settings.squash_merge_commit_title = value.into();
    }
    if let Some(value) = body.squash_merge_commit_message {
        settings.squash_merge_commit_message = value.into();
    }
    if let Some(value) = body.merge_commit_title {
        settings.merge_commit_title = value.into();
    }
    if let Some(value) = body.merge_commit_message {
        settings.merge_commit_message = value.into();
    }
    settings
}

/// The repository name, which may hold only the characters GitHub allows.
fn checked_name(name: &str) -> Result<String, ApiResponse> {
    if valid_name(name) {
        Ok(name.to_owned())
    } else {
        Err(ApiResponse::validation_failed(
            "Repository",
            "name",
            "invalid",
        ))
    }
}

/// Letters, digits, `-`, `_` and `.`, but not `.` or `..`.
fn valid_name(name: &str) -> bool {
    name.len() <= 100
        && !matches!(name, "." | "..")
        && take_while::<_, _, ContextError>(1.., ('a'..='z', 'A'..='Z', '0'..='9', '-', '_', '.'))
            .parse(name)
            .is_ok()
}

/// Creates the bare git repository of the newly recorded `owner/name` at `git_dir`, with
/// `HEAD` on `branch` and, when `readme` is set, a README committed there. Runs with the lock
/// released so other requests proceed; on failure the repository is deleted again, freeing
/// its name.
///
/// # Errors
///
/// The git error that made creating the repository fail.
pub fn init_storage(
    state: &Mutex<State>,
    owner: &str,
    name: &str,
    git_dir: &Path,
    branch: &str,
    readme: bool,
) -> std::io::Result<()> {
    let created = git::init_bare(git_dir, branch).and_then(|()| {
        if readme {
            git::commit_readme(git_dir, branch, name, &super::BOT)
        } else {
            Ok(())
        }
    });
    if created.is_err() {
        let mut state = lock(state);
        let still_ours = state
            .account(owner)
            .and_then(|account| account.repo(name))
            .is_some_and(|repo| repo.git_dir.as_deref() == Some(git_dir));
        if still_ours {
            state.delete_repo(owner, name);
        }
        drop(state);
        let _ = std::fs::remove_dir_all(git_dir);
    }
    created
}

fn create(
    state: &Mutex<State>,
    request: &ApiRequest,
    owner: &str,
    name: String,
    private: bool,
    description: Option<String>,
    auto_init: bool,
) -> ApiResult {
    let name = checked_name(&name)?;
    let scopes: &[&str] = if private {
        &["repo"]
    } else {
        &["repo", "public_repo"]
    };
    request.require_scope(scopes)?;
    let storage = {
        let mut state = lock(state);
        let repo = state.create_repo(owner, &name, private, description)?;
        repo.git_dir
            .clone()
            .map(|git_dir| (git_dir, repo.default_branch.clone()))
    };
    if let Some((git_dir, branch)) = &storage {
        init_storage(state, owner, &name, git_dir, branch, auto_init)
            .map_err(|error| ApiResponse::error(500, &error.to_string()))?;
    }
    let mut state = lock(state);
    let repo = state
        .account_mut(owner)
        .and_then(|account| account.repo_mut(&name))
        .ok_or_else(ApiResponse::not_found)?;
    if auto_init && storage.is_some() {
        repo.pushed_at = Some(crate::state::now());
    }
    let account = state.account(owner).unwrap();
    let value = render::full_repository(
        request,
        account,
        account.repo(&name).unwrap(),
        Permission {
            admin: true,
            push: true,
            pull: true,
        },
    );
    Ok(ApiResponse::typed(201, &value))
}

impl Handler for ops::ReposCreateForAuthenticatedUser {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let login = request.require_caller()?.login.clone();
        let body: types::ReposCreateForAuthenticatedUserRequest = request.json_as()?;
        create(
            state,
            request,
            &login,
            body.name,
            body.private,
            body.description,
            body.auto_init,
        )
    }
}

impl Handler for ops::ReposCreateInOrg {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = request.require_caller()?;
        let body: types::ReposCreateInOrgRequest = request.json_as()?;
        let org = request.param("org");
        {
            let state = lock(state);
            let account = state
                .account(org)
                .filter(|account| account.kind == AccountKind::Organization)
                .ok_or_else(ApiResponse::not_found)?;
            if !account.is_administered_by(&caller.login) {
                return Err(ApiResponse::error(
                    403,
                    "You need admin access to the organization before adding a repository to it.",
                ));
            }
        }
        let private = body.visibility.map_or(body.private, |visibility| {
            visibility == types::ReposCreateInOrgRequestVisibility::Private
        });
        create(
            state,
            request,
            org,
            body.name,
            private,
            body.description,
            body.auto_init,
        )
    }
}

impl Handler for ops::ReposGet {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let found = find_repo(&state, request)?;
        Ok(ApiResponse::typed(
            200,
            &render_found(&state, request, &found),
        ))
    }
}

/// Renders every repository of `owners` that `keep` accepts as `T`, one of the spec's
/// repository shapes, in one paginated list.
fn list<'a, T: serde::Serialize + From<types::FullRepository>>(
    state: &State,
    request: &ApiRequest,
    owners: impl Iterator<Item = &'a str>,
    keep: impl Fn(bool) -> bool,
) -> ApiResponse {
    let caller = request.caller.as_ref();
    let mut items = Vec::new();
    for owner in owners {
        let account = state.account(owner).unwrap();
        for repo in account.repos() {
            if let Some(permission) = state.permission(caller, owner, &repo.name)
                && keep(repo.private)
            {
                let full = render::full_repository(request, account, repo, permission);
                items.push(T::from(full));
            }
        }
    }
    paginate(request, &request.path, items)
}

/// Applies a `visibility` or `type` filter of `all`, `public` or `private`.
fn visibility_filter(request: &ApiRequest, name: &str) -> impl Fn(bool) -> bool {
    let wanted = request.query(name).unwrap_or("all").to_owned();
    move |private| match wanted.as_str() {
        "public" => !private,
        "private" => private,
        _ => true,
    }
}

impl Handler for ops::ReposListForAuthenticatedUser {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = request.require_caller()?;
        let state = lock(state);
        let owners: Vec<String> = state
            .accounts()
            .filter(|account| account.is_administered_by(&caller.login))
            .map(|account| account.login.clone())
            .collect();
        Ok(list::<types::Repository>(
            &state,
            request,
            owners.iter().map(String::as_str),
            visibility_filter(request, "visibility"),
        ))
    }
}

impl Handler for ops::ReposListForOrg {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let org = state
            .account(request.param("org"))
            .filter(|account| account.kind == AccountKind::Organization)
            .ok_or_else(ApiResponse::not_found)?
            .login
            .clone();
        Ok(list::<types::MinimalRepository>(
            &state,
            request,
            std::iter::once(org.as_str()),
            visibility_filter(request, "type"),
        ))
    }
}

/// Lists only public repositories, as GitHub does for this operation.
impl Handler for ops::ReposListForUser {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let user = state
            .account(request.param("username"))
            .ok_or_else(ApiResponse::not_found)?
            .login
            .clone();
        Ok(list::<types::MinimalRepository>(
            &state,
            request,
            std::iter::once(user.as_str()),
            |private| !private,
        ))
    }
}

impl Handler for ops::ReposUpdate {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        request.require_scope(&["repo"])?;
        let body: types::ReposUpdateRequest = request.json_as()?;
        let new_name = body.name.as_deref().map(checked_name).transpose()?;
        let mut state = lock(state);
        let mut found = find_admin_repo(&state, request)?;
        let current = state
            .account(&found.owner)
            .unwrap()
            .repo(&found.name)
            .unwrap();
        let settings = updated_settings(&current.settings, &body);
        let branch = body.default_branch.as_deref();
        if let (Some(branch), Some(git_dir)) = (branch, &current.git_dir) {
            let exists = git::resolve_commit(git_dir, &format!("refs/heads/{branch}"))
                .map_err(|error| ApiResponse::error(500, &error.to_string()))?
                .is_some();
            if !exists {
                return Err(ApiResponse::validation_failed(
                    "Repository",
                    "default_branch",
                    "invalid",
                ));
            }
        }
        if let Some(new_name) = new_name {
            state.move_repo(&found.owner, &found.name, &found.owner, &new_name)?;
            found.name = new_name;
        }
        let repo = state
            .account_mut(&found.owner)
            .unwrap()
            .repo_mut(&found.name)
            .unwrap();
        if let Some(private) = body
            .visibility
            .map(|visibility| visibility == types::ReposUpdateRequestVisibility::Private)
            .or(body.private)
        {
            repo.private = private;
        }
        repo.settings = settings;
        if let Some(description) = body.description {
            repo.description = description;
        }
        if let Some(branch) = branch {
            repo.set_default_branch(branch)
                .map_err(|error| ApiResponse::error(500, &error.to_string()))?;
        }
        repo.updated_at = crate::state::now();
        Ok(ApiResponse::typed(
            200,
            &render_found(&state, request, &found),
        ))
    }
}

impl Handler for ops::ReposDelete {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let mut state = lock(state);
        let found = find_admin_repo(&state, request)?;
        if !request.require_caller()?.has_scope("delete_repo") {
            return Err(ApiResponse::error(
                403,
                "Must have admin rights to Repository.",
            ));
        }
        let deleted = state.delete_repo(&found.owner, &found.name);
        drop(state);
        if let Some(git_dir) = deleted.and_then(|repo| repo.git_dir) {
            let _ = std::fs::remove_dir_all(git_dir);
        }
        Ok(ApiResponse::no_content())
    }
}

impl Handler for ops::ReposTransfer {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = request.require_scope(&["repo"])?.clone();
        let body: types::ReposTransferRequest = request.json_as()?;
        let new_owner = body.new_owner.as_str();
        let mut state = lock(state);
        let found = find_admin_repo(&state, request)?;
        let target = state
            .account(new_owner)
            .filter(|account| account.is_administered_by(&caller.login))
            .ok_or_else(|| ApiResponse::validation_failed("Repository", "new_owner", "invalid"))?;
        let new_owner = target.login.clone();
        let new_name = match body.new_name.as_deref() {
            Some(name) if valid_name(name) => name.to_owned(),
            Some(_) => {
                return Err(ApiResponse::validation_failed(
                    "Repository",
                    "new_name",
                    "invalid",
                ));
            }
            None => found.name.clone(),
        };
        state.move_repo(&found.owner, &found.name, &new_owner, &new_name)?;
        let moved = Found {
            owner: new_owner,
            name: new_name,
            permission: found.permission,
        };
        Ok(ApiResponse::typed(
            202,
            &types::MinimalRepository::from(render_found(&state, request, &moved)),
        ))
    }
}
