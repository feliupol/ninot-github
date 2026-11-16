//! Repositories a team has access to, and the permission it holds on each.
//!
//! Only repositories the organization owns can be added to its teams.

#![expect(
    clippy::disallowed_types,
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    reason = "untyped JSON; remove when this module is typed"
)]

use std::sync::Mutex;

use serde_json::{Value, json};

use super::Handler;
use super::org_support::{Need, find_team, object_body, scoped};
use crate::api::{ApiRequest, ApiResponse, ApiResult, paginate};
use crate::operation::ops;
use crate::render;
use crate::state::TEAM_PERMISSIONS;
use crate::state::{Account, Permission, State, lock};

/// The media type that asks the check operation for the repository instead of a bare 204.
const REPOSITORY_MEDIA_TYPE: &str = "application/vnd.github.v3.repository+json";

fn permission_of(role: &str) -> Permission {
    Permission::from_team_permission(role).unwrap_or_default()
}

/// The permission flags of a team role, as team repositories list them.
fn role_flags(role: &str) -> Value {
    let rank = TEAM_PERMISSIONS
        .iter()
        .position(|known| *known == role)
        .unwrap_or(0);
    json!({
        "pull": true,
        "triage": rank >= 1,
        "push": rank >= 2,
        "maintain": rank >= 3,
        "admin": rank >= 4,
    })
}

/// The repository `owner/name` of the organization, or 404 when the caller cannot see it.
fn org_repo<'s>(
    state: &'s State,
    request: &ApiRequest,
    org: &'s Account,
) -> Result<(&'s crate::state::Repo, Permission), ApiResponse> {
    let owner = request.param("owner");
    let name = request.param("repo");
    let permission = state
        .permission(request.caller.as_ref(), owner, name)
        .ok_or_else(ApiResponse::not_found)?;
    let account = state.account(owner).ok_or_else(ApiResponse::not_found)?;
    let repo = account.repo(name).ok_or_else(ApiResponse::not_found)?;
    // Source: <https://docs.github.com/en/rest/teams/teams#add-or-update-team-repository-permissions>
    if !account.login.eq_ignore_ascii_case(&org.login) {
        return Err(ApiResponse::validation_failed(
            "Team",
            "repository",
            "invalid",
        ));
    }
    Ok((repo, permission))
}

impl Handler for ops::TeamsListReposInOrg {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = scoped(request, Need::Read)?;
        let state = lock(state);
        let (org, team) = find_team(
            &state,
            request.param("org"),
            request.param("team_slug"),
            &caller.login,
        )?;
        let body: Vec<Value> = team
            .repos
            .iter()
            .filter_map(|(name, role)| Some((org.repo(name)?, role)))
            .map(|(repo, role)| {
                serde_json::to_value(render::minimal_repository(
                    request,
                    org,
                    repo,
                    permission_of(role),
                ))
                .expect("repositories serialize to JSON")
            })
            .collect();
        Ok(paginate(request, &request.path, body))
    }
}

impl Handler for ops::TeamsCheckPermissionsForRepoInOrg {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = scoped(request, Need::Read)?;
        let state = lock(state);
        let (org, team) = find_team(
            &state,
            request.param("org"),
            request.param("team_slug"),
            &caller.login,
        )?;
        let on_team = org
            .login
            .eq_ignore_ascii_case(request.param("owner"))
            .then(|| team.repos.get(&request.param("repo").to_lowercase()))
            .flatten()
            .and_then(|role| Some((org.repo(request.param("repo"))?, role)));
        // Source: OpenAPI teams/check-permissions-for-repo-in-org 404
        let Some((repo, role)) = on_team else {
            return Err(ApiResponse::not_found());
        };
        // Source: OpenAPI teams/check-permissions-for-repo-in-org 204
        if !request.accept().contains(REPOSITORY_MEDIA_TYPE) {
            return Ok(ApiResponse::no_content());
        }
        let mut body = serde_json::to_value(render::full_repository(
            request,
            org,
            repo,
            permission_of(role),
        ))
        .expect("repositories serialize to JSON");
        body["permissions"] = role_flags(role);
        body["role_name"] = json!(role);
        for (key, from) in [
            ("forks", "forks_count"),
            ("open_issues", "open_issues_count"),
            ("watchers", "watchers_count"),
        ] {
            if body.get(key).is_none() {
                body[key] = body.get(from).cloned().unwrap_or(json!(0));
            }
        }
        Ok(ApiResponse::json(200, body))
    }
}

impl Handler for ops::TeamsAddOrUpdateRepoPermissionsInOrg {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = scoped(request, Need::Write)?;
        let body = object_body(request)?;
        let mut state = lock(state);
        let (org, team) = find_team(
            &state,
            request.param("org"),
            request.param("team_slug"),
            &caller.login,
        )?;
        let (repo, permission) = org_repo(&state, request, org)?;
        // Source: <https://docs.github.com/en/rest/teams/teams#add-or-update-team-repository-permissions>
        if !permission.admin {
            return Err(ApiResponse::error(
                403,
                "You must have admin access to the repository.",
            ));
        }
        let role = body
            .get("permission")
            .and_then(Value::as_str)
            .unwrap_or(&team.permission)
            .to_owned();
        if !TEAM_PERMISSIONS.contains(&role.as_str()) {
            return Err(ApiResponse::validation_failed(
                "Team",
                "permission",
                "invalid",
            ));
        }
        let (org_login, slug, name) = (
            org.login.clone(),
            team.slug.clone(),
            repo.name.to_lowercase(),
        );
        let team = state
            .account_mut(&org_login)
            .and_then(|org| org.teams.get_mut(&slug))
            .expect("team was found");
        team.repos.insert(name, role);
        team.updated_at = crate::state::now();
        Ok(ApiResponse::no_content())
    }
}

impl Handler for ops::TeamsRemoveRepoInOrg {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = scoped(request, Need::Write)?;
        let mut state = lock(state);
        let (org, team) = find_team(
            &state,
            request.param("org"),
            request.param("team_slug"),
            &caller.login,
        )?;
        let (repo, permission) = org_repo(&state, request, org)?;
        // Source: <https://docs.github.com/en/rest/teams/teams#remove-a-repository-from-a-team>
        if !permission.admin && !org.manages_team(&team.slug, &caller.login) {
            return Err(ApiResponse::error(
                403,
                "You must have admin access to the repository or maintain the team.",
            ));
        }
        let (org_login, slug, name) = (
            org.login.clone(),
            team.slug.clone(),
            repo.name.to_lowercase(),
        );
        let team = state
            .account_mut(&org_login)
            .and_then(|org| org.teams.get_mut(&slug))
            .expect("team was found");
        team.repos.remove(&name);
        team.updated_at = crate::state::now();
        Ok(ApiResponse::no_content())
    }
}
