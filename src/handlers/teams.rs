//! Teams of an organization: listing, creation, updates, deletion and nesting.
//!
//! Owners see every team; members see closed teams and the secret teams they belong to; an
//! outsider sees none. Owners and team maintainers manage a team. Child teams must be closed.

#![expect(
    clippy::disallowed_types,
    clippy::disallowed_methods,
    reason = "untyped JSON; remove when this module is typed"
)]

use std::sync::Mutex;

use serde_json::{Map, Value};

use super::Handler;
use super::org_support::{
    Need, find_org, find_team, object_body, scoped, slugify, team_full_json, team_json,
};
use crate::api::{ApiRequest, ApiResponse, ApiResult, paginate};
use crate::operation::ops;
use crate::state::{Account, OrgRole, State, Team, lock, now};

fn invalid(field: &str) -> ApiResponse {
    ApiResponse::validation_failed("Team", field, "invalid")
}

/// The teams of `org` that `login` can see, by slug.
// Source: <https://docs.github.com/en/rest/teams/teams#list-teams>
fn visible_teams<'a>(org: &'a Account, login: &str) -> Vec<&'a Team> {
    org.teams
        .values()
        .filter(|team| org.team_visible_to(&team.slug, Some(login)))
        .collect()
}

fn text(body: &Map<String, Value>, key: &str) -> Option<String> {
    body.get(key).and_then(Value::as_str).map(str::to_owned)
}

// Source: <https://docs.github.com/en/rest/teams/teams#update-a-team>
fn forbidden() -> ApiResponse {
    ApiResponse::error(
        403,
        "You must be an organization owner or team maintainer to do this.",
    )
}

impl Handler for ops::TeamsList {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = scoped(request, Need::Read)?;
        let state = lock(state);
        let org = find_org(&state, request.param("org"))?;
        let teams = if request.query("team_type") == Some("enterprise") {
            Vec::new()
        } else {
            visible_teams(org, &caller.login)
        };
        let body: Vec<Value> = teams
            .into_iter()
            .map(|team| team_json(request, org, team))
            .collect();
        Ok(paginate(request, &request.path, body))
    }
}

impl Handler for ops::TeamsListForAuthenticatedUser {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = scoped(request, Need::Read)?;
        let state = lock(state);
        let login = caller.login.to_lowercase();
        let mut body = Vec::new();
        for org in state
            .accounts()
            .filter(|account| account.kind == crate::state::AccountKind::Organization)
        {
            for team in org.teams.values() {
                if team.members.contains(&login) {
                    body.push(team_full_json(request, org, team));
                }
            }
        }
        Ok(paginate(request, &request.path, body))
    }
}

impl Handler for ops::TeamsCreate {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = scoped(request, Need::Write)?;
        let body = object_body(request)?;
        let mut state = lock(state);
        let org = find_org(&state, request.param("org"))?;
        // Source: <https://docs.github.com/en/rest/teams/teams#create-a-team>
        if org.role_of(&caller.login) == OrgRole::Outsider {
            return Err(ApiResponse::error(
                403,
                "You must be an organization member to create a team.",
            ));
        }
        let name = text(&body, "name").unwrap_or_default().trim().to_owned();
        let slug = slugify(&name);
        if slug.is_empty() {
            return Err(ApiResponse::validation_failed(
                "Team",
                "name",
                "missing_field",
            ));
        }
        if org.team(&slug).is_some() {
            return Err(ApiResponse::validation_failed(
                "Team",
                "name",
                "already_exists",
            ));
        }
        let parent = match body.get("parent_team_id").and_then(Value::as_u64) {
            Some(id) => Some(
                org.teams
                    .values()
                    .find(|team| crate::render::id(team.id) == id as i64)
                    .filter(|team| org.team_visible_to(&team.slug, Some(&caller.login)))
                    .ok_or_else(|| invalid("parent_team_id"))?,
            ),
            None => None,
        };
        let privacy = text(&body, "privacy");
        // Source: <https://docs.github.com/en/rest/teams/teams#update-a-team>
        if parent.is_some_and(|team| team.privacy == "secret")
            || (parent.is_some() && privacy.as_deref() == Some("secret"))
        {
            return Err(invalid("privacy"));
        }
        let mut maintainers = Vec::new();
        for login in body
            .get("maintainers")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            if !org.members.contains(&login.to_lowercase()) {
                return Err(invalid("maintainers"));
            }
            maintainers.push(login.to_lowercase());
        }
        let mut repos = Vec::new();
        for full in body
            .get("repo_names")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            let repo = full
                .split_once('/')
                .filter(|(owner, _)| owner.eq_ignore_ascii_case(&org.login))
                .and_then(|(_, name)| org.repo(name))
                .ok_or_else(|| invalid("repo_names"))?;
            repos.push(repo.name.to_lowercase());
        }
        let parent_id = parent.map(|team| team.id);
        let permission = text(&body, "permission").unwrap_or_else(|| "pull".to_owned());
        let id = state.next_id();
        let mut team = Team::new(
            id,
            slug.clone(),
            // Source: <https://docs.github.com/en/rest/teams/teams#create-a-team>
            if parent_id.is_some() {
                "closed"
            } else {
                "secret"
            },
        );
        team.name = name;
        team.description = text(&body, "description");
        if let Some(privacy) = privacy {
            team.privacy = privacy;
        }
        if let Some(setting) = text(&body, "notification_setting") {
            team.notification_setting = setting;
        }
        team.parent_id = parent_id;
        // Source: <https://docs.github.com/en/rest/teams/teams#create-a-team>
        for login in maintainers.into_iter().chain([caller.login.to_lowercase()]) {
            team.members.insert(login.clone());
            team.maintainers.insert(login);
        }
        for repo in repos {
            team.repos.insert(repo, permission.clone());
        }
        team.permission = permission;
        let org = state
            .account_mut(request.param("org"))
            .expect("organization was found");
        org.teams.insert(slug.clone(), team);
        let org = state.account(request.param("org")).expect("organization");
        Ok(ApiResponse::json(
            201,
            team_full_json(
                request,
                org,
                org.team(&slug).expect("team was just created"),
            ),
        ))
    }
}

impl Handler for ops::TeamsGetByName {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = scoped(request, Need::Read)?;
        let state = lock(state);
        let (org, team) = find_team(
            &state,
            request.param("org"),
            request.param("team_slug"),
            &caller.login,
        )?;
        Ok(ApiResponse::json(200, team_full_json(request, org, team)))
    }
}

impl Handler for ops::TeamsUpdateInOrg {
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
        if !org.manages_team(&team.slug, &caller.login) {
            return Err(forbidden());
        }
        let old_slug = team.slug.clone();
        let has_children = !org.descendant_slugs(&old_slug).is_empty();
        let mut new_slug = old_slug.clone();
        if let Some(name) = text(&body, "name") {
            new_slug = slugify(&name);
            if new_slug.is_empty() {
                return Err(ApiResponse::validation_failed(
                    "Team",
                    "name",
                    "missing_field",
                ));
            }
            if new_slug != old_slug && org.team(&new_slug).is_some() {
                return Err(ApiResponse::validation_failed(
                    "Team",
                    "name",
                    "already_exists",
                ));
            }
        }
        let parent = match body.get("parent_team_id") {
            None => team.parent_id,
            Some(Value::Null) => None,
            Some(value) => {
                let wanted = value.as_u64().ok_or_else(|| invalid("parent_team_id"))?;
                let parent = org
                    .teams
                    .values()
                    .find(|candidate| crate::render::id(candidate.id) == wanted as i64)
                    .filter(|candidate| {
                        candidate.slug != old_slug
                            && !org.descendant_slugs(&old_slug).contains(&candidate.slug)
                            && candidate.privacy != "secret"
                    })
                    .ok_or_else(|| invalid("parent_team_id"))?;
                Some(parent.id)
            }
        };
        let privacy = text(&body, "privacy").unwrap_or_else(|| team.privacy.clone());
        // Source: <https://docs.github.com/en/rest/teams/teams#update-a-team>
        if privacy == "secret" && (parent.is_some() || has_children) {
            return Err(invalid("privacy"));
        }
        let org_login = org.login.clone();
        let org = state.account_mut(&org_login).expect("organization");
        let mut team = org.teams.remove(&old_slug).expect("team was found");
        if let Some(name) = text(&body, "name") {
            team.name = name;
        }
        if let Some(description) = text(&body, "description") {
            team.description = Some(description);
        }
        if let Some(setting) = text(&body, "notification_setting") {
            team.notification_setting = setting;
        }
        if let Some(permission) = text(&body, "permission") {
            team.permission = permission;
        }
        team.privacy = privacy;
        team.parent_id = parent;
        team.slug.clone_from(&new_slug);
        team.updated_at = now();
        org.teams.insert(new_slug.clone(), team);
        let org = state.account(&org_login).expect("organization");
        Ok(ApiResponse::json(
            200,
            team_full_json(request, org, org.team(&new_slug).expect("team")),
        ))
    }
}

impl Handler for ops::TeamsDeleteInOrg {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = scoped(request, Need::Write)?;
        let mut state = lock(state);
        let (org, team) = find_team(
            &state,
            request.param("org"),
            request.param("team_slug"),
            &caller.login,
        )?;
        if !org.manages_team(&team.slug, &caller.login) {
            return Err(forbidden());
        }
        // Source: <https://docs.github.com/en/rest/teams/teams#delete-a-team>
        let mut doomed = org.descendant_slugs(&team.slug);
        doomed.push(team.slug.clone());
        let org_login = org.login.clone();
        let org = state.account_mut(&org_login).expect("organization");
        for slug in doomed {
            org.teams.remove(&slug);
        }
        Ok(ApiResponse::no_content())
    }
}

impl Handler for ops::TeamsListChildInOrg {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = scoped(request, Need::Read)?;
        let state = lock(state);
        let (org, team) = find_team(
            &state,
            request.param("org"),
            request.param("team_slug"),
            &caller.login,
        )?;
        let body: Vec<Value> = visible_teams(org, &caller.login)
            .into_iter()
            .filter(|child| child.parent_id == Some(team.id))
            .map(|child| team_json(request, org, child))
            .collect();
        Ok(paginate(request, &request.path, body))
    }
}
