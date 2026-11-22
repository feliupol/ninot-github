//! Names, organization access and selected repositories shared by Actions variables and
//! organization secrets.

use std::fmt::Display;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use sha2::{Digest, Sha256};

use super::org_support::{Need, find_org, owners_only, scoped};
use crate::api::{ApiRequest, ApiResponse};
use crate::operation::Operation;
use crate::render;
use crate::state::{Account, ActionsVisibility, OrgRole, Permission, State};
use crate::types;

/// The uppercase form GitHub stores.
// Source: <https://docs.github.com/en/actions/learn-github-actions/variables#naming-conventions-for-configuration-variables>
pub(super) fn variable_name(name: &str) -> String {
    name.to_ascii_uppercase()
}

/// What `gh variable set` treats as "create the variable again".
// Source: cli/cli v2.104.0 pkg/cmd/variable/set/set.go `setVariable` retries HTTP 409 with PATCH.
pub(super) fn already_exists() -> ApiResponse {
    crate::errors::GithubError::VariableAlreadyExists.to_response()
}

/// The organization when the caller owns it. Organization Actions configuration is
/// owner-only.
// Source: <https://docs.github.com/en/actions/security-guides/using-secrets-in-github-actions#creating-secrets-for-an-organization>
pub(super) fn owner_org<'a>(
    state: &'a State,
    request: &ApiRequest,
) -> Result<&'a Account, ApiResponse> {
    // Source: OpenAPI actions/list-org-secrets, classic `admin:org` scope.
    let caller = scoped(request, Need::Admin)?;
    let org = find_org(state, request.param("org"))?;
    if org.role_of(&caller.login) != OrgRole::Owner {
        return Err(owners_only());
    }
    Ok(org)
}

/// The organization's Actions secrets key, derived so a client can seal to it.
pub(super) fn org_public_key(org_id: u64) -> types::ActionsPublicKey {
    let digest = Sha256::digest(format!("ninot-github organization {org_id}"));
    types::ActionsPublicKey {
        created_at: None,
        id: None,
        key: STANDARD.encode(digest),
        key_id: org_id.to_string(),
        title: None,
        url: None,
    }
}

/// Repository ids for `visibility`, or a 422 when the ids do not belong with it.
pub(super) fn repository_ids(
    org: &Account,
    visibility: ActionsVisibility,
    ids: Option<&[i64]>,
    resource: &str,
) -> Result<Vec<u64>, ApiResponse> {
    let ids = ids.unwrap_or(&[]);
    if visibility != ActionsVisibility::Selected {
        // Source: OpenAPI actions/create-or-update-org-secret `selected_repository_ids`.
        if !ids.is_empty() {
            return Err(ApiResponse::validation_failed(
                resource,
                "selected_repository_ids",
                "invalid",
            ));
        }
        return Ok(Vec::new());
    }
    let mut parsed = Vec::new();
    for id in ids {
        let Some(id) = u64::try_from(*id).ok() else {
            return Err(unknown_repo(resource));
        };
        if !org.repos().any(|repo| repo.id == id) {
            return Err(unknown_repo(resource));
        }
        if !parsed.contains(&id) {
            parsed.push(id);
        }
    }
    Ok(parsed)
}

fn unknown_repo(resource: &str) -> ApiResponse {
    ApiResponse::validation_failed(resource, "selected_repository_ids", "invalid")
}

/// The selected repositories still present on `org`, in the stored order.
pub(super) fn selected_repositories(
    request: &ApiRequest,
    org: &Account,
    ids: &[u64],
) -> Vec<types::MinimalRepository> {
    ids.iter()
        .filter_map(|id| org.repos().find(|repo| repo.id == *id))
        .map(|repo| render::minimal_repository(request, org, repo, Permission::ADMIN))
        .collect()
}

/// `operation`'s path for this request's organization and `name`.
pub(super) fn selected_url<O: Operation>(request: &ApiRequest, org: &str, name: &str) -> String {
    let name_param = if O::INFO.path.contains("{secret_name}") {
        "secret_name"
    } else {
        "name"
    };
    request.urls().api_for::<O>(&[
        ("org", &org as &dyn Display),
        (name_param, &name as &dyn Display),
    ])
}
