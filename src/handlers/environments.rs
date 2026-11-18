//! Deployment environments: their protection rules, custom deployment branch and tag
//! policies, and encrypted secrets.
//!
//! Required reviewers are users; team reviewers answer 422.

use std::collections::BTreeMap;
use std::sync::Mutex;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use sha2::{Digest, Sha256};

use super::Handler;
use super::repo_admin::{admin_repo, private_plan};
use super::repos::Found;
use crate::api::{ApiRequest, ApiResponse, ApiResult, Body, paginate_with};
use crate::operation::ops;
use crate::render::{node_id, simple_user};
use crate::state::{AccountKind, DeploymentPolicy, Environment, EnvironmentSecret, State, lock};
use crate::types;

/// At most six required reviewers per environment.
// Source: OpenAPI repos/create-or-update-environment, `reviewers` description.
const MAX_REVIEWERS: usize = 6;

/// GitHub's refusal of required reviewers on a private repository whose plan lacks them.
// Source: GitHub's 422 body for a required reviewers rule on a private repository.
const REVIEWERS_PLAN_LIMIT: &str = "Failed to create the environment protection rule. Please ensure the billing plan supports the required reviewers protection rule.";

/// The administered repository of the request and its environment `environment_name`.
fn environment<'a>(
    state: &'a State,
    request: &ApiRequest,
) -> Result<(Found, &'a Environment), ApiResponse> {
    let found = admin_repo(state, request)?;
    let environment = found
        .repo(state)
        .setup
        .environments
        .get(request.param("environment_name"))
        .ok_or_else(ApiResponse::not_found)?;
    Ok((found, environment))
}

fn render(
    request: &ApiRequest,
    state: &State,
    found: &Found,
    env: &Environment,
) -> types::Environment {
    let urls = request.urls().repo(&found.owner, &found.name);
    let mut protection_rules = Vec::new();
    if !env.reviewers.is_empty() {
        let reviewers = env
            .reviewers
            .iter()
            .filter_map(|id| state.accounts().find(|account| account.id == *id))
            .map(
                |account| types::EnvironmentProtectionRulesItemSubtype1ReviewersItem {
                    reviewer: Some(simple_user(request, account).into()),
                    type_: Some(types::DeploymentReviewerType::User),
                },
            )
            .collect();
        protection_rules.push(types::EnvironmentProtectionRulesItem {
            subtype_0: None,
            subtype_1: Some(types::EnvironmentProtectionRulesItemSubtype1 {
                id: env.reviewers_rule_id as i64,
                node_id: node_id("Gate", env.reviewers_rule_id),
                prevent_self_review: Some(env.prevent_self_review),
                reviewers,
                type_: "required_reviewers".to_owned(),
            }),
            subtype_2: None,
        });
    }
    if env.branch_policy.is_some() {
        protection_rules.push(types::EnvironmentProtectionRulesItem {
            subtype_0: None,
            subtype_1: None,
            subtype_2: Some(types::EnvironmentProtectionRulesItemSubtype2 {
                id: env.branch_policy_rule_id as i64,
                node_id: node_id("Gate", env.branch_policy_rule_id),
                type_: "branch_policy".to_owned(),
            }),
        });
    }
    types::Environment {
        created_at: env.created_at,
        deployment_branch_policy: env.branch_policy.clone(),
        // Source: OpenAPI environment example, html_url.
        html_url: format!(
            "{}/deployments/activity_log?environments_filter={}",
            urls.html, env.name
        ),
        id: env.id as i64,
        name: env.name.clone(),
        node_id: node_id("Environment", env.id),
        protection_rules,
        updated_at: env.updated_at,
        url: urls.api(&format!("/environments/{}", env.name)),
    }
}

impl Handler for ops::ReposGetEnvironment {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let (found, env) = environment(&state, request)?;
        Ok(ApiResponse::typed(
            200,
            &types::ReposGetEnvironmentResponse(render(request, &state, &found, env)),
        ))
    }
}

impl Handler for ops::ReposCreateOrUpdateEnvironment {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let body: types::ReposCreateOrUpdateEnvironmentRequest = request.json_as()?;
        let body = body.unwrap_or_default();
        let mut state = lock(state);
        let found = admin_repo(&state, request)?;
        let reviewers = body.reviewers.flatten().unwrap_or_default();
        if reviewers.len() > MAX_REVIEWERS {
            return Err(ApiResponse::validation_failed(
                "Environment",
                "reviewers",
                "invalid",
            ));
        }
        let mut reviewer_ids = Vec::new();
        for reviewer in &reviewers {
            let user = reviewer
                .id
                .filter(|_| reviewer.type_ == Some(types::DeploymentReviewerType::User))
                .and_then(|id| {
                    state.accounts().find(|account| {
                        account.id as i64 == id && account.kind == AccountKind::User
                    })
                })
                .ok_or_else(|| {
                    ApiResponse::validation_failed("Environment", "reviewers", "invalid")
                })?;
            if !reviewer_ids.contains(&user.id) {
                reviewer_ids.push(user.id);
            }
        }
        if !reviewer_ids.is_empty()
            && private_plan(&state, &found).is_some_and(|plan| !plan.private_required_reviewers())
        {
            return Err(ApiResponse::unprocessable(REVIEWERS_PLAN_LIMIT));
        }
        let name = request.param("environment_name").to_owned();
        let exists = found.repo(&state).setup.environments.contains_key(&name);
        let ids = if exists {
            None
        } else {
            Some((state.next_id(), state.next_id(), state.next_id()))
        };
        let now = crate::time::now_utc();
        let environments = &mut found.repo_mut(&mut state).setup.environments;
        let env = environments.entry(name.clone()).or_insert_with(|| {
            let (id, reviewers_rule_id, branch_policy_rule_id) =
                ids.expect("ids are allocated for a new environment");
            Environment {
                id,
                name,
                created_at: now,
                updated_at: now,
                reviewers_rule_id,
                reviewers: Vec::new(),
                prevent_self_review: false,
                branch_policy_rule_id,
                branch_policy: None,
                policies: BTreeMap::new(),
                secrets: BTreeMap::new(),
            }
        });
        env.reviewers = reviewer_ids;
        env.prevent_self_review = body.prevent_self_review.is_some_and(|value| *value);
        env.branch_policy = body.deployment_branch_policy.flatten();
        if !env
            .branch_policy
            .as_ref()
            .is_some_and(|policy| policy.custom_branch_policies)
        {
            env.policies.clear();
        }
        env.updated_at = now;
        let env = env.clone();
        Ok(ApiResponse::typed(
            200,
            &types::ReposCreateOrUpdateEnvironmentResponse(render(request, &state, &found, &env)),
        ))
    }
}

/// The custom policies of an environment, or 404 while it has none.
// Source: OpenAPI repos/create-deployment-branch-policy 404, "Not Found or
// `deployment_branch_policy.custom_branch_policies` property for the environment is set to false".
fn custom_policies(env: &Environment) -> Result<&BTreeMap<u64, DeploymentPolicy>, ApiResponse> {
    if env
        .branch_policy
        .as_ref()
        .is_some_and(|policy| policy.custom_branch_policies)
    {
        Ok(&env.policies)
    } else {
        Err(ApiResponse::not_found())
    }
}

fn render_policy(policy: &DeploymentPolicy) -> types::DeploymentBranchPolicy {
    types::DeploymentBranchPolicy {
        id: Some(policy.id as i64),
        name: Some(policy.name.clone()),
        node_id: Some(node_id("GateBranchPolicy", policy.id)),
        type_: Some(policy.type_),
    }
}

impl Handler for ops::ReposListDeploymentBranchPolicies {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let (_, env) = environment(&state, request)?;
        let policies = custom_policies(env)?
            .values()
            .map(render_policy)
            .collect::<Vec<_>>();
        Ok(paginate_with(
            request,
            &request.path,
            policies,
            |total_count, branch_policies| types::ReposListDeploymentBranchPoliciesResponse {
                total_count,
                branch_policies,
            },
        ))
    }
}

impl Handler for ops::ReposCreateDeploymentBranchPolicy {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let body: types::ReposCreateDeploymentBranchPolicyRequest = request.json_as()?;
        let body = body.0;
        let type_ = match body.type_ {
            Some(types::DeploymentBranchPolicyNamePatternWithTypeType::Tag) => {
                types::DeploymentBranchPolicyType::Tag
            }
            _ => types::DeploymentBranchPolicyType::Branch,
        };
        let mut state = lock(state);
        let (found, env) = environment(&state, request)?;
        let policies = custom_policies(env)?;
        if policies
            .values()
            .any(|policy| policy.name == body.name && policy.type_ == type_)
        {
            // Source: OpenAPI repos/create-deployment-branch-policy 303, "Response if the same
            // branch name pattern already exists".
            return Ok(ApiResponse {
                status: 303,
                headers: Vec::new(),
                body: Body::Empty,
            });
        }
        let name = request.param("environment_name").to_owned();
        let id = state.next_id();
        let policy = DeploymentPolicy {
            id,
            name: body.name,
            type_,
        };
        let rendered = render_policy(&policy);
        let env = found
            .repo_mut(&mut state)
            .setup
            .environments
            .get_mut(&name)
            .expect("environment found above");
        env.policies.insert(id, policy);
        Ok(ApiResponse::typed(
            200,
            &types::ReposCreateDeploymentBranchPolicyResponse(rendered),
        ))
    }
}

/// The environment's secrets key: 32 bytes derived from the repository and environment ids, so
/// a client can seal secrets to it. Nothing holds the private key.
fn public_key(found_repo_id: u64, env: &Environment) -> types::ActionsPublicKey {
    let digest = Sha256::digest(format!(
        "ninot-github environment {found_repo_id}/{}",
        env.id
    ));
    types::ActionsPublicKey {
        created_at: None,
        id: None,
        key: STANDARD.encode(digest),
        key_id: env.id.to_string(),
        title: None,
        url: None,
    }
}

impl Handler for ops::ActionsGetEnvironmentPublicKey {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let (found, env) = environment(&state, request)?;
        Ok(ApiResponse::typed(
            200,
            &types::ActionsGetEnvironmentPublicKeyResponse(public_key(found.repo(&state).id, env)),
        ))
    }
}

impl Handler for ops::ActionsListEnvironmentSecrets {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let (_, env) = environment(&state, request)?;
        let secrets = env
            .secrets
            .values()
            .map(|secret| types::ActionsSecret {
                created_at: secret.created_at,
                name: secret.name.clone(),
                updated_at: secret.updated_at,
            })
            .collect::<Vec<_>>();
        Ok(paginate_with(
            request,
            &request.path,
            secrets,
            |total_count, secrets| types::ActionsListEnvironmentSecretsResponse {
                total_count,
                secrets,
            },
        ))
    }
}

impl Handler for ops::ActionsCreateOrUpdateEnvironmentSecret {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let body: types::ActionsCreateOrUpdateEnvironmentSecretRequest = request.json_as()?;
        let mut state = lock(state);
        let (found, env) = environment(&state, request)?;
        if body.key_id != public_key(found.repo(&state).id, env).key_id {
            return Err(ApiResponse::validation_failed("Secret", "key_id", "custom"));
        }
        let env_name = request.param("environment_name").to_owned();
        let name = request.param("secret_name").to_owned();
        let now = crate::time::now_utc();
        let env = found
            .repo_mut(&mut state)
            .setup
            .environments
            .get_mut(&env_name)
            .expect("environment found above");
        let created_at = env.secrets.get(&name).map(|secret| secret.created_at);
        env.secrets.insert(
            name.clone(),
            EnvironmentSecret {
                name,
                encrypted_value: body.encrypted_value.into(),
                key_id: body.key_id,
                created_at: created_at.unwrap_or(now),
                updated_at: now,
            },
        );
        // Source: OpenAPI actions/create-or-update-environment-secret 201 and 204
        if created_at.is_some() {
            Ok(ApiResponse::no_content())
        } else {
            Ok(ApiResponse::typed(201, &types::EmptyObject {}))
        }
    }
}
