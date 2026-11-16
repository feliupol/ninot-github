//! Organizations: listing, settings, app installations and outside collaborators.

#![expect(
    clippy::disallowed_types,
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    reason = "untyped JSON; remove when this module is typed"
)]

use std::sync::Mutex;

use serde_json::{Value, json};

use super::Handler;
use super::org_support::{Need, find_org, object_body, organization_simple, owners_only, scoped};
use crate::api::{ApiRequest, ApiResponse, ApiResult, PER_PAGE_DEFAULT, PER_PAGE_MAX, paginate};
use crate::operation::ops;
use crate::render;
use crate::state::{AccountKind, OrgRole, State, lock};

/// The longest organization description GitHub accepts.
// Source: <https://docs.github.com/en/rest/orgs/orgs#update-an-organization>
const DESCRIPTION_LIMIT: usize = 160;

impl Handler for ops::OrgsList {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let since = request
            .query("since")
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0);
        let per_page = request
            .query("per_page")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(PER_PAGE_DEFAULT)
            .clamp(1, PER_PAGE_MAX);
        let state = lock(state);
        let mut orgs: Vec<_> = state
            .accounts()
            .filter(|account| account.kind == AccountKind::Organization && account.id > since)
            .collect();
        // Source: <https://docs.github.com/en/rest/orgs/orgs#list-organizations>
        orgs.sort_by_key(|org| org.id);
        let more = orgs.len() > per_page;
        orgs.truncate(per_page);
        let next = orgs.last().map(|org| org.id);
        let body: Vec<Value> = orgs
            .iter()
            .map(|org| organization_simple(request, org))
            .collect();
        let response = ApiResponse::json(200, Value::Array(body));
        Ok(match next {
            // Source: <https://docs.github.com/en/rest/orgs/orgs#list-organizations>
            Some(last) if more => response.with_header(
                "link",
                format!(
                    "<{}>; rel=\"next\"",
                    request
                        .urls()
                        .api(&format!("/organizations?per_page={per_page}&since={last}"))
                ),
            ),
            _ => response,
        })
    }
}

impl Handler for ops::OrgsUpdate {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = scoped(request, Need::Admin)?;
        let body = object_body(request)?;
        let mut state = lock(state);
        // Source: <https://docs.github.com/en/rest/orgs/orgs#update-an-organization>
        if find_org(&state, request.param("org"))?.role_of(&caller.login) != OrgRole::Owner {
            return Err(owners_only());
        }
        if body
            .get("description")
            .and_then(Value::as_str)
            .is_some_and(|text| text.chars().count() > DESCRIPTION_LIMIT)
        {
            return Err(ApiResponse::validation_failed(
                "Organization",
                "description",
                "custom",
            ));
        }
        let text = |key: &str| body.get(key).and_then(Value::as_str).map(str::to_owned);
        let flag = |key: &str| body.get(key).and_then(Value::as_bool);
        let org = state
            .account_mut(request.param("org"))
            .expect("organization was found");
        if let Some(name) = text("name") {
            org.name = Some(name);
        }
        let profile = &mut org.profile;
        for (key, slot) in [
            ("description", &mut profile.description),
            ("company", &mut profile.company),
            ("location", &mut profile.location),
            ("email", &mut profile.email),
            ("blog", &mut profile.blog),
            ("twitter_username", &mut profile.twitter_username),
            ("billing_email", &mut profile.billing_email),
            (
                "default_repository_permission",
                &mut profile.default_repository_permission,
            ),
        ] {
            if let Some(value) = text(key) {
                *slot = Some(value);
            }
        }
        if let Some(value) = flag("has_organization_projects") {
            profile.has_organization_projects = Some(value);
        }
        if let Some(value) = flag("has_repository_projects") {
            profile.has_repository_projects = Some(value);
        }
        let org = state.account(request.param("org")).expect("organization");
        Ok(ApiResponse::typed(200, &render::organization(request, org)))
    }
}

impl Handler for ops::OrgsListAppInstallations {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = scoped(request, Need::Read)?;
        let state = lock(state);
        let org = find_org(&state, request.param("org"))?;
        // Source: <https://docs.github.com/en/rest/orgs/orgs#list-app-installations-for-an-organization>
        if org.role_of(&caller.login) != OrgRole::Owner {
            return Err(owners_only());
        }
        let mut found: Vec<_> = state
            .installations
            .values()
            .filter(|installation| installation.owner.eq_ignore_ascii_case(&org.login))
            .collect();
        found.sort_by_key(|installation| installation.id);
        let total = found.len();
        let (page, link) = crate::api::page(
            request,
            &request.path,
            found
                .into_iter()
                .map(|installation| super::apps::installation_model(request, &state, installation))
                .collect(),
        );
        let response = ApiResponse::json(
            200,
            json!({
                "total_count": total,
                "installations": serde_json::to_value(page).expect("installations serialize"),
            }),
        );
        Ok(match link {
            Some(link) => response.with_header("link", link),
            None => response,
        })
    }
}

impl Handler for ops::OrgsListOutsideCollaborators {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = scoped(request, Need::Read)?;
        let state = lock(state);
        let org = find_org(&state, request.param("org"))?;
        if org.role_of(&caller.login) != OrgRole::Owner {
            return Err(owners_only());
        }
        super::org_members::two_factor_filter(request, org)?;
        // The fake has no per-repository collaborators, so nobody outside the organization
        // holds access to its repositories.
        let outside: Vec<Value> = Vec::new();
        Ok(paginate(request, &request.path, outside))
    }
}
