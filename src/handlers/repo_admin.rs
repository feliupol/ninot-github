//! Repository administration: topics and the Actions permissions, plus what the environment
//! and ruleset handlers share: admin access and the plan limits of private repositories.

use std::sync::Mutex;

use super::Handler;
use super::repos::{Found, find_repo};
use crate::api::{ApiRequest, ApiResponse, ApiResult};
use crate::operation::ops;
use crate::state::{Plan, State, lock};
use crate::types;

/// Classic token scope of the administration operations.
// Source: <https://docs.github.com/en/rest/repos/repos#replace-all-repository-topics>
pub(super) const SCOPES: [&str; 1] = ["repo"];

/// The repository of the request, when the caller administers it.
pub(super) fn admin_repo(state: &State, request: &ApiRequest) -> Result<Found, ApiResponse> {
    request.require_scope(&SCOPES)?;
    let found = find_repo(state, request)?;
    if !found.permission.admin {
        return Err(crate::errors::GithubError::AdminRequired.to_response());
    }
    Ok(found)
}

/// The plan of the repository's owner, when the repository is private and the owner has one.
pub(super) fn private_plan(state: &State, found: &Found) -> Option<Plan> {
    if !found.repo(state).private {
        return None;
    }
    state.account(&found.owner).and_then(|owner| owner.plan)
}

/// 403 when the owner's plan withholds rulesets from this repository.
///
/// GitHub's refusal of a feature its plan withholds from private repositories.
// Source: GitHub's 403 body for rulesets of a private repository on Free.
pub(super) fn require_rulesets(state: &State, found: &Found) -> Result<(), ApiResponse> {
    match private_plan(state, found) {
        Some(plan) if !plan.private_rulesets() => {
            Err(crate::errors::GithubError::PlanLimit.to_response())
        }
        _ => Ok(()),
    }
}

/// Whether `topic` is a valid topic: a lowercase letter or digit, then up to 49 lowercase
/// letters, digits and hyphens.
// Source: <https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/customizing-your-repository/classifying-your-repository-with-topics>
fn valid_topic(topic: &str) -> bool {
    let mut chars = topic.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_lowercase() || first.is_ascii_digit())
        && topic.len() <= 50
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// At most 20 topics per repository.
// Source: <https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/customizing-your-repository/classifying-your-repository-with-topics>
const MAX_TOPICS: usize = 20;

impl Handler for ops::ReposReplaceAllTopics {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let body: types::ReposReplaceAllTopicsRequest = request.json_as()?;
        let mut state = lock(state);
        let found = admin_repo(&state, request)?;
        // GitHub lowercases topics and drops repeats.
        let mut names: Vec<String> = Vec::new();
        for name in body.names {
            let name = name.trim().to_lowercase();
            if !valid_topic(&name) {
                return Err(ApiResponse::validation_failed(
                    "Repository",
                    "names",
                    "invalid",
                ));
            }
            if !names.contains(&name) {
                names.push(name);
            }
        }
        if names.len() > MAX_TOPICS {
            return Err(ApiResponse::validation_failed(
                "Repository",
                "names",
                "invalid",
            ));
        }
        found.repo_mut(&mut state).setup.topics = names.clone();
        Ok(ApiResponse::ok_typed(
            &types::ReposReplaceAllTopicsResponse(types::Topic { names }),
        ))
    }
}

impl Handler for ops::ReposGetAllTopics {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let found = find_repo(&state, request)?;
        let names = found.repo(&state).setup.topics.clone();
        Ok(ApiResponse::ok_typed(&types::ReposGetAllTopicsResponse(
            types::Topic { names },
        )))
    }
}

impl Handler for ops::ActionsGetGithubActionsPermissionsRepository {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let found = admin_repo(&state, request)?;
        let actions = &found.repo(&state).setup.actions;
        // A disabled repository has no allowed actions.
        // Source: <https://docs.github.com/en/rest/actions/permissions#get-github-actions-permissions-for-a-repository>
        let allowed_actions = actions.enabled.then_some(actions.allowed_actions);
        Ok(ApiResponse::ok_typed(
            &types::ActionsGetGithubActionsPermissionsRepositoryResponse(
                types::ActionsRepositoryPermissions {
                    allowed_actions,
                    enabled: types::ActionsEnabled(actions.enabled),
                    selected_actions_url: None,
                    sha_pinning_required: None,
                },
            ),
        ))
    }
}

impl Handler for ops::ActionsSetGithubActionsPermissionsRepository {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let body: types::ActionsSetGithubActionsPermissionsRepositoryRequest = request.json_as()?;
        let mut state = lock(state);
        let found = admin_repo(&state, request)?;
        let actions = &mut found.repo_mut(&mut state).setup.actions;
        actions.enabled = *body.enabled;
        if let Some(allowed) = body.allowed_actions {
            actions.allowed_actions = allowed;
        }
        Ok(ApiResponse::no_content())
    }
}

impl Handler for ops::ActionsGetGithubActionsDefaultWorkflowPermissionsRepository {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let found = admin_repo(&state, request)?;
        let actions = &found.repo(&state).setup.actions;
        Ok(ApiResponse::ok_typed(
            &types::ActionsGetGithubActionsDefaultWorkflowPermissionsRepositoryResponse(
                types::ActionsGetDefaultWorkflowPermissions {
                    can_approve_pull_request_reviews: types::ActionsCanApprovePullRequestReviews(
                        actions.can_approve_pull_request_reviews,
                    ),
                    default_workflow_permissions: actions.default_workflow_permissions,
                },
            ),
        ))
    }
}

impl Handler for ops::ActionsSetGithubActionsDefaultWorkflowPermissionsRepository {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let body: types::ActionsSetDefaultWorkflowPermissions = request.json_as()?;
        let mut state = lock(state);
        let found = admin_repo(&state, request)?;
        let actions = &mut found.repo_mut(&mut state).setup.actions;
        if let Some(permissions) = body.default_workflow_permissions {
            actions.default_workflow_permissions = permissions;
        }
        if let Some(can_approve) = body.can_approve_pull_request_reviews {
            actions.can_approve_pull_request_reviews = *can_approve;
        }
        Ok(ApiResponse::no_content())
    }
}
