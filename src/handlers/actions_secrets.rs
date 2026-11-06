//! Repository Actions secrets and their public encryption key.

use std::sync::Mutex;

use super::Handler;
use super::repos::find_repo;
use crate::api::{ApiRequest, ApiResponse, ApiResult, paginate_with};
use crate::operation::ops;
use crate::state::{State, lock};
use crate::types;

const READ: &str = "actions:secrets:read";
const WRITE: &str = "actions:secrets:write";

fn secret_repository<'a>(
    state: &'a State,
    request: &ApiRequest,
) -> Result<&'a crate::state::Repo, ApiResponse> {
    let found = find_repo(state, request)?;
    if !found.permission.admin {
        return Err(ApiResponse::error(
            403,
            "Must have admin rights to Repository.",
        ));
    }
    let repo = state
        .account(&found.owner)
        .and_then(|account| account.repo(&found.name))
        .ok_or_else(ApiResponse::not_found)?;
    Ok(repo)
}

impl Handler for ops::ActionsGetRepoPublicKey {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        request.require_actions_secrets_permission(READ)?;
        let state = lock(state);
        let repo = secret_repository(&state, request)?;
        let key = repo
            .actions_public_key
            .clone()
            .ok_or_else(ApiResponse::not_found)?;
        Ok(ApiResponse::typed(
            200,
            &types::ActionsGetRepoPublicKeyResponse(key),
        ))
    }
}

impl Handler for ops::ActionsListRepoSecrets {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        request.require_actions_secrets_permission(READ)?;
        let state = lock(state);
        let repo = secret_repository(&state, request)?;
        let secrets = repo
            .actions_secrets
            .values()
            .map(|secret| types::ActionsSecret {
                name: secret.name.clone(),
                created_at: secret.created_at,
                updated_at: secret.updated_at,
            })
            .collect::<Vec<_>>();
        Ok(paginate_with(
            request,
            &request.path,
            secrets,
            |total_count, secrets| types::ActionsListRepoSecretsResponse {
                total_count,
                secrets,
            },
        ))
    }
}

impl Handler for ops::ActionsGetRepoSecret {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        request.require_actions_secrets_permission(READ)?;
        let state = lock(state);
        let repo = secret_repository(&state, request)?;
        let secret = repo
            .actions_secrets
            .get(request.param("secret_name"))
            .ok_or_else(ApiResponse::not_found)?;
        let output = types::ActionsSecret {
            name: secret.name.clone(),
            created_at: secret.created_at,
            updated_at: secret.updated_at,
        };
        Ok(ApiResponse::typed(
            200,
            &types::ActionsGetRepoSecretResponse(output),
        ))
    }
}

impl Handler for ops::ActionsCreateOrUpdateRepoSecret {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        request.require_actions_secrets_permission(WRITE)?;
        let body: types::ActionsCreateOrUpdateRepoSecretRequest = request.json_as()?;
        let mut state = lock(state);
        let found = find_repo(&state, request)?;
        if !found.permission.admin {
            return Err(ApiResponse::error(
                403,
                "Must have admin rights to Repository.",
            ));
        }
        let repo = state
            .account_mut(&found.owner)
            .and_then(|account| account.repo_mut(&found.name))
            .ok_or_else(ApiResponse::not_found)?;
        let public_key = repo
            .actions_public_key
            .as_ref()
            .ok_or_else(ApiResponse::not_found)?;
        if body.key_id != public_key.key_id {
            return Err(ApiResponse::validation_failed("Secret", "key_id", "custom"));
        }
        let name = request.param("secret_name").to_owned();
        let timestamp = crate::render::timestamp(&crate::state::now());
        let existed = repo.actions_secrets.contains_key(&name);
        let created_at = repo
            .actions_secrets
            .get(&name)
            .map(|secret| secret.created_at)
            .unwrap_or(timestamp);
        repo.actions_secrets.insert(
            name.clone(),
            crate::state::StoredActionsSecret {
                name,
                encrypted_value: body.encrypted_value.into(),
                key_id: body.key_id,
                created_at,
                updated_at: timestamp,
            },
        );
        if existed {
            Ok(ApiResponse::no_content())
        } else {
            Ok(ApiResponse::typed(
                201,
                &types::ActionsCreateOrUpdateRepoSecretResponse(types::EmptyObject {}),
            ))
        }
    }
}

impl Handler for ops::ActionsDeleteRepoSecret {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        request.require_actions_secrets_permission(WRITE)?;
        let mut state = lock(state);
        let found = find_repo(&state, request)?;
        if !found.permission.admin {
            return Err(ApiResponse::error(
                403,
                "Must have admin rights to Repository.",
            ));
        }
        let repo = state
            .account_mut(&found.owner)
            .and_then(|account| account.repo_mut(&found.name))
            .ok_or_else(ApiResponse::not_found)?;
        repo.actions_secrets
            .remove(request.param("secret_name"))
            .ok_or_else(ApiResponse::not_found)?;
        Ok(ApiResponse::no_content())
    }
}
