//! Team members and memberships.
//!
//! Adding someone who is not yet an organization member invites them to the organization and
//! leaves their team membership `pending`.

#![expect(
    clippy::disallowed_types,
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    reason = "untyped JSON; remove when this module is typed"
)]

use std::sync::Mutex;

use serde_json::{Value, json};

use super::Handler;
use super::org_support::{Need, find_team, object_body, scoped, user_json};
use crate::api::{ApiRequest, ApiResponse, ApiResult, paginate};
use crate::operation::ops;
use crate::state::{AccountKind, State, Team, lock};

fn forbidden() -> ApiResponse {
    ApiResponse::error(
        403,
        "You must be an organization owner or team maintainer to do this.",
    )
}

fn membership(
    request: &ApiRequest,
    org: &str,
    team: &Team,
    login: &str,
    state: &str,
    role: &str,
) -> Value {
    json!({
        "url": request.urls().team(org, &team.slug).membership(login),
        "role": role,
        "state": state,
    })
}

impl Handler for ops::TeamsListMembersInOrg {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = scoped(request, Need::Read)?;
        let state = lock(state);
        let (org, team) = find_team(
            &state,
            request.param("org"),
            request.param("team_slug"),
            &caller.login,
        )?;
        let role = request.query("role").unwrap_or("all");
        let mut logins: Vec<&String> = match role {
            "maintainer" => team.maintainers.iter().collect(),
            _ => team
                .members
                .iter()
                .filter(|login| role == "all" || !team.maintainers.contains(*login))
                .collect(),
        };
        // Source: <https://docs.github.com/en/rest/teams/members#list-team-members>
        if role != "maintainer" {
            let inherited = org
                .descendant_slugs(&team.slug)
                .into_iter()
                .filter_map(|slug| org.team(&slug))
                .flat_map(|child| child.members.iter());
            for login in inherited {
                if !logins.contains(&login) {
                    logins.push(login);
                }
            }
        }
        logins.sort();
        let body: Vec<Value> = logins
            .into_iter()
            .filter_map(|login| user_json(request, &state, login))
            .collect();
        Ok(paginate(request, &request.path, body))
    }
}

impl Handler for ops::TeamsGetMembershipForUserInOrg {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = scoped(request, Need::Read)?;
        let state = lock(state);
        let (org, team) = find_team(
            &state,
            request.param("org"),
            request.param("team_slug"),
            &caller.login,
        )?;
        let username = request.param("username");
        let key = username.to_lowercase();
        // Source: OpenAPI teams/get-membership-for-user-in-org 404
        let (membership_state, role) = if team.members.contains(&key) {
            let maintainer = team.maintainers.contains(&key);
            ("active", if maintainer { "maintainer" } else { "member" })
        } else if team.pending.contains(&key) {
            ("pending", "member")
        } else {
            return Err(ApiResponse::not_found());
        };
        Ok(ApiResponse::json(
            200,
            membership(request, &org.login, team, username, membership_state, role),
        ))
    }
}

impl Handler for ops::TeamsAddOrUpdateMembershipForUserInOrg {
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
        // Source: <https://docs.github.com/en/rest/teams/members#add-or-update-team-membership-for-a-user>
        if !org.manages_team(&team.slug, &caller.login) {
            return Err(forbidden());
        }
        let user = state
            .account(request.param("username"))
            .filter(|account| account.kind == AccountKind::User)
            .map(|account| account.login.clone())
            .ok_or_else(ApiResponse::not_found)?;
        let key = user.to_lowercase();
        let maintainer = body.get("role").and_then(Value::as_str) == Some("maintainer");
        let active = org.members.contains(&key);
        let (org_login, slug) = (org.login.clone(), team.slug.clone());
        let org = state.account_mut(&org_login).expect("organization");
        // Source: <https://docs.github.com/en/rest/teams/members#add-or-update-team-membership-for-a-user>
        if !active {
            org.pending
                .entry(key.clone())
                .or_insert_with(|| "member".to_owned());
        }
        let team = org.teams.get_mut(&slug).expect("team");
        if active {
            team.pending.remove(&key);
            team.members.insert(key.clone());
            if maintainer {
                team.maintainers.insert(key);
            } else {
                team.maintainers.remove(&key);
            }
        } else {
            team.pending.insert(key);
        }
        team.updated_at = crate::state::now();
        let role = if maintainer { "maintainer" } else { "member" };
        let org = state.account(&org_login).expect("organization");
        Ok(ApiResponse::json(
            200,
            membership(
                request,
                &org_login,
                org.team(&slug).expect("team"),
                &user,
                if active { "active" } else { "pending" },
                role,
            ),
        ))
    }
}

impl Handler for ops::TeamsRemoveMembershipForUserInOrg {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = scoped(request, Need::Write)?;
        let mut state = lock(state);
        let (org, team) = find_team(
            &state,
            request.param("org"),
            request.param("team_slug"),
            &caller.login,
        )?;
        // Source: <https://docs.github.com/en/rest/teams/members#remove-team-membership-for-a-user>
        if !org.manages_team(&team.slug, &caller.login) {
            return Err(forbidden());
        }
        let key = request.param("username").to_lowercase();
        let (org_login, slug) = (org.login.clone(), team.slug.clone());
        let team = state
            .account_mut(&org_login)
            .and_then(|org| org.teams.get_mut(&slug))
            .expect("team was found");
        team.members.remove(&key);
        team.maintainers.remove(&key);
        team.pending.remove(&key);
        team.updated_at = crate::state::now();
        Ok(ApiResponse::no_content())
    }
}
