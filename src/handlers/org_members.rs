//! Organization members and memberships.
//!
//! A token holding `read:org` that belongs to a member sees concealed members too; everyone
//! else, anonymous callers included, sees only the members who publicized their membership.
//! Setting a membership for someone who is not a member invites them: the membership is
//! `pending` until the invitation is accepted.

use std::sync::Mutex;

use serde_json::{Value, json};

use super::Handler;
use super::org_support::{
    Need, find_org, object_body, organization_simple, owners_only, scoped, user_json,
};
use crate::api::{ApiRequest, ApiResponse, ApiResult, Body, paginate};
use crate::operation::ops;
use crate::state::{Account, AccountKind, OrgRole, State, lock};

/// Whether the caller may see concealed members of `org`.
// Source: <https://docs.github.com/en/rest/orgs/members#list-organization-members>
fn sees_all(request: &ApiRequest, org: &Account) -> bool {
    request.caller.as_ref().is_some_and(|caller| {
        ["read:org", "write:org", "admin:org"]
            .iter()
            .any(|scope| caller.has_scope(scope))
            && org.role_of(&caller.login) != OrgRole::Outsider
    })
}

/// Whether a two-factor `filter` query selects members, as opposed to `all`.
///
/// Only owners may use one; an unknown value is a 422 for everyone.
pub(super) fn two_factor_filter(request: &ApiRequest, org: &Account) -> Result<bool, ApiResponse> {
    let invalid = || ApiResponse::validation_failed("Organization", "filter", "invalid");
    match request.query("filter").unwrap_or("all") {
        "all" => Ok(false),
        // Source: <https://docs.github.com/en/rest/orgs/members#list-organization-members>
        "2fa_disabled" | "2fa_insecure" => {
            let owner = request
                .caller
                .as_ref()
                .is_some_and(|caller| org.role_of(&caller.login) == OrgRole::Owner);
            if owner { Ok(true) } else { Err(invalid()) }
        }
        _ => Err(invalid()),
    }
}

impl Handler for ops::OrgsListMembers {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let org = find_org(&state, request.param("org"))?;
        let two_factor = two_factor_filter(request, org)?;
        let role = request.query("role").unwrap_or("all");
        if !["all", "admin", "member"].contains(&role) {
            return Err(ApiResponse::validation_failed(
                "Organization",
                "role",
                "invalid",
            ));
        }
        let all = sees_all(request, org);
        let selected = org.members.iter().filter(|login| match role {
            "admin" => org.owners.contains(*login),
            "member" => !org.owners.contains(*login),
            _ => true,
        });
        // No account has two-factor authentication, so every member has it disabled and none
        // has it insecurely configured.
        let members: Vec<Value> = selected
            .filter(|login| all || org.public_members.contains(*login))
            .filter(|_| !two_factor || request.query("filter") == Some("2fa_disabled"))
            .filter_map(|login| user_json(request, &state, login))
            .collect();
        Ok(paginate(request, &request.path, members))
    }
}

impl Handler for ops::OrgsCheckMembershipForUser {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let org = find_org(&state, request.param("org"))?;
        let username = request.param("username").to_lowercase();
        // Source: OpenAPI orgs/check-membership-for-user 302
        if !sees_all(request, org) {
            let location = request
                .urls()
                .org(&org.login)
                .public_member(request.param("username"));
            return Ok(ApiResponse {
                status: 302,
                headers: vec![("location".to_owned(), location)],
                body: Body::Empty,
            });
        }
        if org.members.contains(&username) {
            Ok(ApiResponse::no_content())
        } else {
            Err(ApiResponse::not_found())
        }
    }
}

impl Handler for ops::OrgsRemoveMember {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = scoped(request, Need::Admin)?;
        let mut state = lock(state);
        if find_org(&state, request.param("org"))?.role_of(&caller.login) != OrgRole::Owner {
            return Err(owners_only());
        }
        let org = request.param("org").to_owned();
        // Source: <https://docs.github.com/en/rest/orgs/members#remove-an-organization-member>
        state.remove_org_member(&org, request.param("username"));
        Ok(ApiResponse::no_content())
    }
}

/// The `org-membership` model of `login` in `org`, or `None` when they have none.
fn membership_json(
    request: &ApiRequest,
    state: &State,
    org: &Account,
    login: &str,
) -> Option<Value> {
    let key = login.to_lowercase();
    let (membership, role) = if org.members.contains(&key) {
        let owner = org.owners.contains(&key);
        ("active", if owner { "admin" } else { "member" })
    } else {
        ("pending", org.pending.get(&key)?.as_str())
    };
    let user = user_json(request, state, login)?;
    Some(json!({
        "url": request.urls().org(&org.login).membership(login),
        "state": membership,
        "role": role,
        "direct_membership": membership == "active",
        "organization_url": request.urls().org(&org.login).api,
        "organization": organization_simple(request, org),
        "user": user,
        "permissions": {"can_create_repository": role == "admin"},
    }))
}

impl Handler for ops::OrgsGetMembershipForUser {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = scoped(request, Need::Read)?;
        let state = lock(state);
        let org = find_org(&state, request.param("org"))?;
        let own = caller.login.eq_ignore_ascii_case(request.param("username"));
        // Source: <https://docs.github.com/en/rest/orgs/members#get-organization-membership-for-a-user>
        if org.role_of(&caller.login) == OrgRole::Outsider && !own {
            return Err(ApiResponse::error(
                403,
                "You must be an organization member to see its memberships.",
            ));
        }
        membership_json(request, &state, org, request.param("username"))
            .map(|body| ApiResponse::json(200, body))
            .ok_or_else(ApiResponse::not_found)
    }
}

impl Handler for ops::OrgsSetMembershipForUser {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = scoped(request, Need::Admin)?;
        let body = object_body(request)?;
        let mut state = lock(state);
        // Source: <https://docs.github.com/en/rest/orgs/members#set-organization-membership-for-a-user>
        if find_org(&state, request.param("org"))?.role_of(&caller.login) != OrgRole::Owner {
            return Err(owners_only());
        }
        let user = state
            .account(request.param("username"))
            .filter(|account| account.kind == AccountKind::User)
            .map(|account| account.login.clone())
            .ok_or_else(ApiResponse::not_found)?;
        let key = user.to_lowercase();
        let admin = body.get("role").and_then(Value::as_str) == Some("admin");
        let org = state
            .account_mut(request.param("org"))
            .expect("organization was found");
        if org.members.contains(&key) {
            if admin {
                org.owners.insert(key);
            } else {
                org.owners.remove(&key);
            }
        } else {
            org.pending
                .insert(key, if admin { "admin" } else { "member" }.to_owned());
        }
        let org = state.account(request.param("org")).expect("organization");
        let body = membership_json(request, &state, org, &user).expect("membership was just set");
        Ok(ApiResponse::json(200, body))
    }
}

impl Handler for ops::OrgsRemoveMembershipForUser {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = scoped(request, Need::Admin)?;
        let mut state = lock(state);
        let org = find_org(&state, request.param("org"))?;
        // Source: <https://docs.github.com/en/rest/orgs/members#remove-organization-membership-for-a-user>
        if org.role_of(&caller.login) != OrgRole::Owner {
            return Err(owners_only());
        }
        let key = request.param("username").to_lowercase();
        if !org.members.contains(&key) && !org.pending.contains_key(&key) {
            return Err(ApiResponse::not_found());
        }
        let org = org.login.clone();
        state.remove_org_member(&org, &key);
        Ok(ApiResponse::no_content())
    }
}
