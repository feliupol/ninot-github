//! Authorization and JSON models shared by the organization and team handlers.

#![expect(
    clippy::disallowed_types,
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    reason = "untyped JSON; remove when this module is typed"
)]

use serde_json::{Value, json};

use crate::api::{ApiRequest, ApiResponse, Caller};
use crate::render;
use crate::state::{Account, AccountKind, State, Team};

/// How strong a token scope an operation needs.
#[derive(Clone, Copy)]
pub(super) enum Need {
    /// `read:org`, or a scope that implies it.
    Read,
    /// `write:org`, or a scope that implies it.
    Write,
    /// `admin:org`.
    Admin,
}

/// The authenticated caller; 401 without one and 403 when the token's scopes do not reach `need`.
pub(super) fn scoped(request: &ApiRequest, need: Need) -> Result<&Caller, ApiResponse> {
    let caller = request.require_caller()?;
    let allowed: &[&str] = match need {
        Need::Read => &["read:org", "write:org", "admin:org"],
        Need::Write => &["write:org", "admin:org"],
        Need::Admin => &["admin:org"],
    };
    if allowed.iter().any(|scope| caller.has_scope(scope)) {
        Ok(caller)
    } else {
        Err(crate::errors::GithubError::PatForbidden.to_response())
    }
}

/// The organization `login`, or 404.
pub(super) fn find_org<'s>(state: &'s State, login: &str) -> Result<&'s Account, ApiResponse> {
    state
        .account(login)
        .filter(|account| account.kind == AccountKind::Organization)
        .ok_or_else(ApiResponse::not_found)
}

/// The team `slug` of the organization when `login` may see it, or 404.
pub(super) fn find_team<'s>(
    state: &'s State,
    org: &str,
    slug: &str,
    login: &str,
) -> Result<(&'s Account, &'s Team), ApiResponse> {
    let org = find_org(state, org)?;
    if !org.team_visible_to(slug, Some(login)) {
        return Err(ApiResponse::not_found());
    }
    let team = org.team(slug).ok_or_else(ApiResponse::not_found)?;
    Ok((org, team))
}

/// The 403 for an operation only organization owners may do.
pub(super) fn owners_only() -> ApiResponse {
    crate::errors::GithubError::OrgOwnerRequired.to_response()
}

/// A JSON object body, for operations whose body fields are all optional.
pub(super) fn object_body(
    request: &ApiRequest,
) -> Result<serde_json::Map<String, Value>, ApiResponse> {
    request.json_as()
}

/// The short organization model.
pub(super) fn organization_simple(request: &ApiRequest, org: &Account) -> Value {
    let urls = request.urls().org(&org.login);
    json!({
        "login": org.login,
        "id": render::id(org.id),
        "node_id": render::node_id("Organization", org.id),
        "url": urls.api,
        "repos_url": urls.api("/repos"),
        "events_url": urls.api("/events"),
        "hooks_url": urls.api("/hooks"),
        "issues_url": urls.api("/issues"),
        "members_url": urls.api("/members{/member}"),
        "public_members_url": urls.api("/public_members{/member}"),
        "avatar_url": urls.avatar,
        "description": org.profile.description.clone().unwrap_or_default(),
    })
}

/// A user as members and collaborators lists show them.
pub(super) fn user_json(request: &ApiRequest, state: &State, login: &str) -> Option<Value> {
    state
        .account(login)
        .filter(|account| account.kind == AccountKind::User)
        .map(|account| {
            serde_json::to_value(render::simple_user(request, account))
                .expect("users serialize to JSON")
        })
}

fn team_base(request: &ApiRequest, org: &Account, team: &Team) -> Value {
    let urls = request.urls().team(&org.login, &team.slug);
    json!({
        "id": render::id(team.id),
        "node_id": render::node_id("Team", team.id),
        "name": team.name,
        "slug": team.slug,
        "description": team.description,
        "privacy": team.privacy,
        "notification_setting": team.notification_setting,
        "permission": team.permission,
        "url": urls.api,
        "html_url": urls.html,
        "members_url": urls.members(),
        "repositories_url": urls.repositories(),
        "type": "organization",
        "organization_id": render::id(org.id),
    })
}

/// The `team` model: a team with its parent.
pub(super) fn team_json(request: &ApiRequest, org: &Account, team: &Team) -> Value {
    let mut value = team_base(request, org, team);
    value["parent"] = parent_json(request, org, team);
    value
}

/// The `team-full` model: a team with counts and its organization.
pub(super) fn team_full_json(request: &ApiRequest, org: &Account, team: &Team) -> Value {
    let mut value = team_json(request, org, team);
    value["created_at"] = json!(team.created_at);
    value["updated_at"] = json!(team.updated_at);
    value["members_count"] = json!(team.members.len());
    value["repos_count"] = json!(team.repos.len());
    value["organization"] = serde_json::to_value(render::organization(request, org))
        .expect("organizations serialize to JSON");
    value
}

fn parent_json(request: &ApiRequest, org: &Account, team: &Team) -> Value {
    team.parent_id
        .and_then(|id| org.teams.values().find(|candidate| candidate.id == id))
        .map_or(Value::Null, |parent| team_base(request, org, parent))
}

/// A team slug as GitHub derives it from a name.
pub(super) fn slugify(name: &str) -> String {
    let mut slug = String::new();
    for ch in name.trim().chars() {
        if ch.is_alphanumeric() {
            slug.extend(ch.to_lowercase());
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    slug.trim_matches('-').to_owned()
}
