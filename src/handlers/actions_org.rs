//! Organization Actions secrets and variables.
//!
//! A secret's ciphertext is stored and never returned. A variable name is stored uppercase.

use std::sync::Mutex;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::Handler;
use super::actions_config::{
    already_exists, org_public_key, owner_org, repository_ids, selected_repositories, selected_url,
    variable_name,
};
use crate::api::{ApiRequest, ApiResponse, ApiResult, paginate_with};
use crate::operation::ops;
use crate::state::{Account, ActionsVisibility, OrgActionsSecret, OrgActionsVariable, State, lock};
use crate::types;

#[derive(Serialize)]
struct OrgSecretBody {
    name: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    visibility: ActionsVisibility,
    #[serde(skip_serializing_if = "Option::is_none")]
    selected_repositories_url: Option<String>,
}

fn render_secret(request: &ApiRequest, org: &str, secret: &OrgActionsSecret) -> OrgSecretBody {
    OrgSecretBody {
        name: secret.name.clone(),
        created_at: secret.created_at,
        updated_at: secret.updated_at,
        visibility: secret.visibility,
        selected_repositories_url: (secret.visibility == ActionsVisibility::Selected).then(|| {
            selected_url::<ops::ActionsListSelectedReposForOrgSecret>(request, org, &secret.name)
        }),
    }
}

#[derive(Serialize)]
struct SecretPage {
    total_count: i64,
    secrets: Vec<OrgSecretBody>,
}

#[derive(Deserialize)]
struct PutSecret {
    encrypted_value: String,
    key_id: String,
    visibility: ActionsVisibility,
    #[serde(default)]
    selected_repository_ids: Option<Vec<i64>>,
}

#[derive(Serialize)]
struct OrgVariableBody {
    name: String,
    value: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    visibility: ActionsVisibility,
    #[serde(skip_serializing_if = "Option::is_none")]
    selected_repositories_url: Option<String>,
}

fn render_variable(
    request: &ApiRequest,
    org: &str,
    variable: &OrgActionsVariable,
) -> OrgVariableBody {
    OrgVariableBody {
        name: variable.name.clone(),
        value: variable.value.clone(),
        created_at: variable.created_at,
        updated_at: variable.updated_at,
        visibility: variable.visibility,
        selected_repositories_url: (variable.visibility == ActionsVisibility::Selected).then(
            || {
                selected_url::<ops::ActionsListSelectedReposForOrgVariable>(
                    request,
                    org,
                    &variable.name,
                )
            },
        ),
    }
}

#[derive(Serialize)]
struct VariablePage {
    total_count: i64,
    variables: Vec<OrgVariableBody>,
}

#[derive(Deserialize)]
struct CreateVariable {
    name: String,
    value: String,
    visibility: ActionsVisibility,
    #[serde(default)]
    selected_repository_ids: Option<Vec<i64>>,
}

#[derive(Deserialize)]
struct UpdateVariable {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    visibility: Option<ActionsVisibility>,
    #[serde(default)]
    selected_repository_ids: Option<Vec<i64>>,
}

#[derive(Serialize)]
struct SelectedPage {
    total_count: i64,
    repositories: Vec<types::MinimalRepository>,
}

fn selected_page(request: &ApiRequest, org: &Account, ids: &[u64]) -> ApiResult {
    let repositories = selected_repositories(request, org, ids);
    Ok(paginate_with(
        request,
        &request.path,
        repositories,
        |total_count, repositories| SelectedPage {
            total_count,
            repositories,
        },
    ))
}

fn created() -> ApiResult {
    Ok(ApiResponse::typed(201, &types::EmptyObject {}))
}

/// Ids to store for a partial update. Omitted ids keep the current selection.
fn chosen_ids(
    org: &Account,
    current_visibility: ActionsVisibility,
    current_ids: &[u64],
    visibility: ActionsVisibility,
    ids: Option<&[i64]>,
    resource: &str,
) -> Result<Vec<u64>, ApiResponse> {
    if ids.is_none() && visibility == ActionsVisibility::Selected {
        return Ok(if current_visibility == ActionsVisibility::Selected {
            current_ids.to_vec()
        } else {
            Vec::new()
        });
    }
    repository_ids(org, visibility, ids, resource)
}

// Source: OpenAPI actions/list-selected-repos-for-org-variable, 409 when visibility is not selected.
fn not_selected() -> ApiResponse {
    ApiResponse::error(409, "Visibility must be selected")
}

impl Handler for ops::ActionsGetOrgPublicKey {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let org = owner_org(&state, request)?;
        Ok(ApiResponse::typed(200, &org_public_key(org.id)))
    }
}

impl Handler for ops::ActionsListOrgSecrets {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let org = owner_org(&state, request)?;
        let login = org.login.clone();
        let secrets = org
            .profile
            .actions_secrets
            .values()
            .map(|secret| render_secret(request, &login, secret))
            .collect();
        Ok(paginate_with(
            request,
            &request.path,
            secrets,
            |total_count, secrets| SecretPage {
                total_count,
                secrets,
            },
        ))
    }
}

impl Handler for ops::ActionsGetOrgSecret {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let org = owner_org(&state, request)?;
        let secret = org
            .profile
            .actions_secrets
            .get(request.param("secret_name"))
            .ok_or_else(ApiResponse::not_found)?;
        Ok(ApiResponse::typed(
            200,
            &render_secret(request, &org.login, secret),
        ))
    }
}

impl Handler for ops::ActionsCreateOrUpdateOrgSecret {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let body: PutSecret = request.json_as()?;
        let mut state = lock(state);
        let (login, ids) = {
            let org = owner_org(&state, request)?;
            if body.key_id != org_public_key(org.id).key_id {
                return Err(ApiResponse::validation_failed("Secret", "key_id", "custom"));
            }
            let ids = repository_ids(
                org,
                body.visibility,
                body.selected_repository_ids.as_deref(),
                "Secret",
            )?;
            (org.login.clone(), ids)
        };
        let name = request.param("secret_name").to_owned();
        let org = state.account_mut(&login).expect("organization found above");
        let now = crate::time::now_utc();
        let created_at = org
            .profile
            .actions_secrets
            .get(&name)
            .map(|secret| secret.created_at);
        org.profile.actions_secrets.insert(
            name.clone(),
            OrgActionsSecret {
                name,
                encrypted_value: body.encrypted_value,
                key_id: body.key_id,
                visibility: body.visibility,
                selected_repository_ids: ids,
                created_at: created_at.unwrap_or(now),
                updated_at: now,
            },
        );
        // Source: OpenAPI actions/create-or-update-org-secret, 201 when creating and 204 when updating.
        if created_at.is_some() {
            Ok(ApiResponse::no_content())
        } else {
            created()
        }
    }
}

impl Handler for ops::ActionsDeleteOrgSecret {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let mut state = lock(state);
        let login = owner_org(&state, request)?.login.clone();
        let org = state.account_mut(&login).expect("organization found above");
        org.profile
            .actions_secrets
            .remove(request.param("secret_name"))
            .ok_or_else(ApiResponse::not_found)?;
        Ok(ApiResponse::no_content())
    }
}

impl Handler for ops::ActionsListSelectedReposForOrgSecret {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let org = owner_org(&state, request)?;
        let ids = org
            .profile
            .actions_secrets
            .get(request.param("secret_name"))
            .ok_or_else(ApiResponse::not_found)?
            .selected_repository_ids
            .clone();
        selected_page(request, org, &ids)
    }
}

impl Handler for ops::ActionsListOrgVariables {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let org = owner_org(&state, request)?;
        let login = org.login.clone();
        let variables = org
            .profile
            .actions_variables
            .values()
            .map(|variable| render_variable(request, &login, variable))
            .collect();
        Ok(paginate_with(
            request,
            &request.path,
            variables,
            |total_count, variables| VariablePage {
                total_count,
                variables,
            },
        ))
    }
}

impl Handler for ops::ActionsCreateOrgVariable {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let body: CreateVariable = request.json_as()?;
        let mut state = lock(state);
        let (login, name, ids) = {
            let org = owner_org(&state, request)?;
            let name = variable_name(&body.name);
            let ids = repository_ids(
                org,
                body.visibility,
                body.selected_repository_ids.as_deref(),
                "Variable",
            )?;
            (org.login.clone(), name, ids)
        };
        let org = state.account_mut(&login).expect("organization found above");
        if org.profile.actions_variables.contains_key(&name) {
            return Err(already_exists());
        }
        let now = crate::time::now_utc();
        org.profile.actions_variables.insert(
            name.clone(),
            OrgActionsVariable {
                name,
                value: body.value,
                visibility: body.visibility,
                selected_repository_ids: ids,
                created_at: now,
                updated_at: now,
            },
        );
        created()
    }
}

impl Handler for ops::ActionsGetOrgVariable {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let org = owner_org(&state, request)?;
        let variable = org
            .profile
            .actions_variables
            .get(&variable_name(request.param("name")))
            .ok_or_else(ApiResponse::not_found)?;
        Ok(ApiResponse::typed(
            200,
            &render_variable(request, &org.login, variable),
        ))
    }
}

impl Handler for ops::ActionsUpdateOrgVariable {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let body: UpdateVariable = request.json_as()?;
        let url_name = variable_name(request.param("name"));
        let mut state = lock(state);
        let (login, new_name, value, visibility, ids, created_at) = {
            let org = owner_org(&state, request)?;
            let current = org
                .profile
                .actions_variables
                .get(&url_name)
                .ok_or_else(ApiResponse::not_found)?;
            let new_name = body
                .name
                .as_deref()
                .map(variable_name)
                .unwrap_or_else(|| url_name.clone());
            if new_name != url_name && org.profile.actions_variables.contains_key(&new_name) {
                return Err(already_exists());
            }
            let visibility = body.visibility.unwrap_or(current.visibility);
            let ids = chosen_ids(
                org,
                current.visibility,
                &current.selected_repository_ids,
                visibility,
                body.selected_repository_ids.as_deref(),
                "Variable",
            )?;
            let value = body.value.unwrap_or_else(|| current.value.clone());
            (
                org.login.clone(),
                new_name,
                value,
                visibility,
                ids,
                current.created_at,
            )
        };
        let org = state.account_mut(&login).expect("organization found above");
        org.profile.actions_variables.remove(&url_name);
        org.profile.actions_variables.insert(
            new_name.clone(),
            OrgActionsVariable {
                name: new_name,
                value,
                visibility,
                selected_repository_ids: ids,
                created_at,
                updated_at: crate::time::now_utc(),
            },
        );
        Ok(ApiResponse::no_content())
    }
}

impl Handler for ops::ActionsDeleteOrgVariable {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let mut state = lock(state);
        let login = owner_org(&state, request)?.login.clone();
        let name = variable_name(request.param("name"));
        let org = state.account_mut(&login).expect("organization found above");
        org.profile
            .actions_variables
            .remove(&name)
            .ok_or_else(ApiResponse::not_found)?;
        Ok(ApiResponse::no_content())
    }
}

impl Handler for ops::ActionsListSelectedReposForOrgVariable {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let org = owner_org(&state, request)?;
        let variable = org
            .profile
            .actions_variables
            .get(&variable_name(request.param("name")))
            .ok_or_else(ApiResponse::not_found)?;
        if variable.visibility != ActionsVisibility::Selected {
            return Err(not_selected());
        }
        let ids = variable.selected_repository_ids.clone();
        selected_page(request, org, &ids)
    }
}
