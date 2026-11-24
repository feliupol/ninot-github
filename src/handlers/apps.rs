//! GitHub App installation discovery and token exchange.

use std::sync::Mutex;

use super::Handler;
use crate::api::{
    ApiRequest, ApiResponse, ApiResult, Caller, INSTALLATION_SCOPE, PERMISSION_SCOPE,
};
use crate::operation::ops;
use crate::permissions::Access;
use crate::state::{ActionsSecretsPermission, Installation, Permission, State, lock};
use crate::types;

fn app_slug(app_id: u64) -> String {
    if app_id == 1 {
        "ninot-test".to_owned()
    } else {
        format!("app-{app_id}")
    }
}

/// Repository permissions of every seeded installation besides Actions secrets, as
/// [`app_permissions`] shows them.
const GRANTED: [(&str, Access); 5] = [
    ("checks", Access::Write),
    ("contents", Access::Write),
    ("issues", Access::Write),
    ("metadata", Access::Read),
    ("pull_requests", Access::Write),
];

fn app_permissions(secrets: Option<ActionsSecretsPermission>) -> types::AppPermissions {
    types::AppPermissions {
        actions: None,
        administration: None,
        artifact_metadata: None,
        attestations: None,
        checks: Some(types::AppPermissionsChecks::Write),
        code_quality: None,
        codespaces: None,
        contents: Some(types::AppPermissionsContents::Write),
        custom_properties_for_organizations: None,
        dependabot_secrets: None,
        deployments: None,
        discussions: None,
        email_addresses: None,
        enterprise_custom_properties_for_organizations: None,
        environments: None,
        followers: None,
        git_ssh_keys: None,
        gpg_keys: None,
        interaction_limits: None,
        issues: Some(types::AppPermissionsIssues::Write),
        members: None,
        merge_queues: None,
        metadata: Some(types::AppPermissionsMetadata::Read),
        organization_administration: None,
        organization_announcement_banners: None,
        organization_copilot_agent_settings: None,
        organization_copilot_seat_management: None,
        organization_custom_org_roles: None,
        organization_custom_properties: None,
        organization_custom_roles: None,
        organization_events: None,
        organization_external_properties_for_repos: None,
        organization_hooks: None,
        organization_packages: None,
        organization_personal_access_token_requests: None,
        organization_personal_access_tokens: None,
        organization_plan: None,
        organization_projects: None,
        organization_secrets: None,
        organization_self_hosted_runners: None,
        organization_user_blocking: None,
        packages: None,
        pages: None,
        profile: None,
        pull_requests: Some(types::AppPermissionsPullRequests::Write),
        repository_custom_properties: None,
        repository_hooks: None,
        repository_projects: None,
        secret_scanning_alerts: None,
        secrets: secrets.map(ActionsSecretsPermission::as_api),
        security_events: None,
        single_file: None,
        starring: None,
        statuses: None,
        vulnerability_alerts: None,
        workflows: None,
    }
}

/// The installation as the API shows it.
pub(super) fn installation_model(
    request: &ApiRequest,
    state: &State,
    installation: &crate::state::Installation,
) -> types::Installation {
    let account = state
        .account(&installation.owner)
        .expect("installation owner exists");
    let timestamp = crate::time::now_utc();
    let urls = request.urls().installation(installation.id);
    types::Installation {
        access_tokens_url: urls.access_tokens,
        account: Some(types::InstallationAccount::SimpleUser(
            crate::render::simple_user(request, account),
        )),
        app_id: crate::render::id(installation.app_id),
        app_slug: app_slug(installation.app_id),
        client_id: None,
        contact_email: None,
        created_at: timestamp,
        events: Vec::new(),
        has_multiple_single_files: Some(false),
        html_url: urls.html,
        id: crate::render::id(installation.id),
        permissions: app_permissions(installation.secrets_permission),
        repositories_url: request.urls().api("/installation/repositories"),
        repository_selection: types::InstallationRepositorySelection::Selected,
        single_file_name: None,
        single_file_paths: Vec::new(),
        suspended_at: None,
        suspended_by: None,
        target_id: crate::render::id(account.id),
        target_type: account.kind.as_str().to_owned(),
        updated_at: timestamp,
    }
}

impl Handler for ops::AppsGetRepoInstallation {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        // Source: <https://docs.github.com/en/rest/apps/apps#get-a-repository-installation-for-the-authenticated-app>
        request.require_scope(&["app"])?;
        let state = lock(state);
        let installation = state
            .installations
            .values()
            .find(|installation| {
                installation
                    .owner
                    .eq_ignore_ascii_case(request.param("owner"))
                    && installation
                        .repository
                        .eq_ignore_ascii_case(request.param("repo"))
            })
            .cloned()
            .ok_or_else(ApiResponse::not_found)?;
        Ok(ApiResponse::ok_typed(
            &types::AppsGetRepoInstallationResponse(installation_model(
                request,
                &state,
                &installation,
            )),
        ))
    }
}

impl Handler for ops::AppsCreateInstallationAccessToken {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        // Source: <https://docs.github.com/en/rest/apps/apps#create-an-installation-access-token-for-an-app>
        request.require_scope(&["app"])?;
        let body: types::AppsCreateInstallationAccessTokenRequest = request.json_as()?;
        let id = request
            .param("installation_id")
            .parse::<u64>()
            .map_err(|_| ApiResponse::not_found())?;
        let mut state = lock(state);
        let installation = state
            .installations
            .get(&id)
            .cloned()
            .ok_or_else(ApiResponse::not_found)?;
        let account = state
            .account(&installation.owner)
            .ok_or_else(ApiResponse::not_found)?;
        let repository = account
            .repo(&installation.repository)
            .ok_or_else(ApiResponse::not_found)?;
        let repository_id = i64::try_from(repository.id)
            .map_err(|_| ApiResponse::internal("Repository id exceeds API limits"))?;
        let repository_is_supported = body
            .repositories
            .iter()
            .all(|name| name.eq_ignore_ascii_case(&installation.repository))
            && body
                .repository_ids
                .iter()
                .all(|requested_id| *requested_id == repository_id);
        // Source: <https://docs.github.com/en/rest/apps/apps#create-an-installation-access-token-for-an-app>
        if !repository_is_supported {
            return Err(crate::errors::GithubError::RepositoriesExceedGrant.to_response());
        }
        let requested_secrets = body
            .permissions
            .as_ref()
            .map(|permissions| permissions.secrets.map(ActionsSecretsPermission::from_api));
        if let Some(Some(requested)) = requested_secrets {
            let granted = installation.secrets_permission;
            let within_grant = matches!(
                (granted, requested),
                (
                    Some(ActionsSecretsPermission::Read),
                    ActionsSecretsPermission::Read
                ) | (Some(ActionsSecretsPermission::Write), _)
            );
            // Source: <https://docs.github.com/en/rest/apps/apps#create-an-installation-access-token-for-an-app>
            if !within_grant {
                return Err(crate::errors::GithubError::PermissionsExceedGrant.to_response());
            }
        }
        let effective_secrets = requested_secrets.unwrap_or(installation.secrets_permission);
        let repository_response = crate::render::repository_for_pull(
            request,
            account,
            repository,
            crate::state::Permission::ADMIN,
        );
        let token_id = state.next_id();
        let token = format!("ghs_ninot_installation_{id}_{token_id}");
        let installation_scope = format!(
            "{INSTALLATION_SCOPE}{}/{}",
            installation.owner, installation.repository
        );
        let app_scope = format!("github-app:{}", installation.app_id);
        let secrets = effective_secrets.map(|secrets| match secrets {
            ActionsSecretsPermission::Read => ("secrets", Access::Read),
            ActionsSecretsPermission::Write => ("secrets", Access::Write),
        });
        let permission_scopes = GRANTED
            .into_iter()
            .chain(secrets)
            .map(|(name, access)| format!("{PERMISSION_SCOPE}{name}:{}", access.as_str()));
        state.tokens.insert(
            token.clone(),
            Caller {
                login: installation.owner.clone(),
                scopes: std::iter::once(installation_scope)
                    .chain(std::iter::once(app_scope))
                    .chain(permission_scopes)
                    .collect(),
            },
        );
        Ok(ApiResponse::created(
            &types::AppsCreateInstallationAccessTokenResponse(types::InstallationToken {
                expires_at: "2099-01-01T00:00:00Z".to_owned(),
                has_multiple_single_files: None,
                permissions: Some(app_permissions(effective_secrets)),
                repositories: vec![repository_response],
                repository_selection: Some(types::InstallationTokenRepositorySelection::Selected),
                single_file: None,
                single_file_paths: Vec::new(),
                token,
            }),
        ))
    }
}

/// The envelope of a list of repositories an installation can access.
#[derive(serde::Serialize)]
struct RepositoryList {
    total_count: i64,
    repository_selection: &'static str,
    repositories: Vec<types::Repository>,
}

/// The claims of an App JWT that name the app.
#[derive(serde::Deserialize)]
struct AppClaims {
    iss: Issuer,
}

/// `iss` is the app's id or client id; only an id selects an app.
#[derive(serde::Deserialize)]
#[serde(untagged)]
enum Issuer {
    Id(u64),
    Text(String),
}

/// The id of the app an App JWT was issued for, when its `iss` claim names one.
fn jwt_app_id(request: &ApiRequest) -> Option<u64> {
    use base64::Engine as _;
    let token = crate::server::token_from_authorization(&request.headers)?;
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .ok()?;
    match serde_json::from_slice::<AppClaims>(&bytes).ok()?.iss {
        Issuer::Id(id) => Some(id),
        Issuer::Text(text) => text.parse().ok(),
    }
}

/// What `login` may do in the installation's repository, by an explicit grant: ownership,
/// organization administration or a collaboration, but not the read access every public
/// repository gives.
fn explicit_permission(
    state: &State,
    login: &str,
    installation: &Installation,
) -> Option<Permission> {
    let account = state.account(&installation.owner)?;
    account.repo(&installation.repository)?;
    if account.is_administered_by(login) {
        return Some(Permission::ADMIN);
    }
    account
        .member_permission(login, &installation.repository)
        .filter(|permission| permission.pull || permission.push || permission.admin)
}

/// The installations of the app behind a user access token that its user can access.
fn user_installations<'a>(
    state: &'a State,
    login: &str,
    app_id: u64,
) -> Vec<(&'a Installation, Permission)> {
    state
        .installations
        .values()
        .filter(|installation| installation.app_id == app_id)
        .filter_map(|installation| {
            explicit_permission(state, login, installation)
                .map(|permission| (installation, permission))
        })
        .collect()
}

/// The id of the app behind a user access token, or the error of any other caller.
// Source: <https://docs.github.com/en/rest/apps/installations#list-app-installations-accessible-to-the-user-access-token>
fn user_token_app(request: &ApiRequest) -> Result<(&crate::api::Caller, u64), ApiResponse> {
    let caller = request.require_caller()?;
    let app_id = caller
        .user_to_server_app()
        .ok_or_else(|| crate::errors::GithubError::PatForbidden.to_response())?;
    Ok((caller, app_id))
}

fn installation_repository(
    request: &ApiRequest,
    state: &State,
    installation: &Installation,
    permission: Permission,
) -> Result<types::Repository, ApiResponse> {
    let account = state
        .account(&installation.owner)
        .ok_or_else(ApiResponse::not_found)?;
    let repo = account
        .repo(&installation.repository)
        .ok_or_else(ApiResponse::not_found)?;
    Ok(crate::render::repository_for_pull(
        request, account, repo, permission,
    ))
}

impl Handler for ops::AppsListInstallations {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        // Source: <https://docs.github.com/en/rest/apps/apps#list-installations-for-the-authenticated-app>
        request.require_scope(&["app"])?;
        let since = request
            .query("since")
            .and_then(|since| since.parse::<u64>().ok())
            .unwrap_or(0);
        let app_id = jwt_app_id(request);
        let state = lock(state);
        let installations = state
            .installations
            .values()
            .filter(|installation| {
                installation.id > since && app_id.is_none_or(|app| installation.app_id == app)
            })
            .map(|installation| installation_model(request, &state, installation))
            .collect();
        Ok(crate::api::paginate(request, &request.path, installations))
    }
}

impl Handler for ops::AppsListReposAccessibleToInstallation {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        // Source: <https://docs.github.com/en/rest/apps/installations#list-repositories-accessible-to-the-app-installation>
        let caller = request.require_caller()?;
        let (owner, repository) = caller
            .installation()
            .and_then(|installation| installation.split_once('/'))
            .ok_or_else(|| crate::errors::GithubError::PatForbidden.to_response())?;
        let state = lock(state);
        let account = state.account(owner).ok_or_else(ApiResponse::not_found)?;
        let repo = account
            .repo(repository)
            .ok_or_else(ApiResponse::not_found)?;
        let repositories = vec![crate::render::repository_for_pull(
            request,
            account,
            repo,
            Permission::ADMIN,
        )];
        Ok(crate::api::paginate_with(
            request,
            &request.path,
            repositories,
            |total_count, repositories| RepositoryList {
                total_count,
                repository_selection: "selected",
                repositories,
            },
        ))
    }
}

impl Handler for ops::AppsListInstallationsForAuthenticatedUser {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        // Source: <https://docs.github.com/en/rest/apps/installations#list-app-installations-accessible-to-the-user-access-token>
        let (caller, app_id) = user_token_app(request)?;
        let state = lock(state);
        let installations = user_installations(&state, &caller.login, app_id)
            .into_iter()
            .map(|(installation, _)| installation_model(request, &state, installation))
            .collect();
        Ok(crate::api::paginate_with(
            request,
            &request.path,
            installations,
            |total_count, installations| types::AppsListInstallationsForAuthenticatedUserResponse {
                installations,
                total_count,
            },
        ))
    }
}

impl Handler for ops::AppsListInstallationReposForAuthenticatedUser {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        // Source: <https://docs.github.com/en/rest/apps/installations#list-repositories-accessible-to-the-user-access-token>
        let (caller, app_id) = user_token_app(request)?;
        let id = request
            .param("installation_id")
            .parse::<u64>()
            .map_err(|_| ApiResponse::not_found())?;
        let state = lock(state);
        let (installation, permission) = user_installations(&state, &caller.login, app_id)
            .into_iter()
            .find(|(installation, _)| installation.id == id)
            .ok_or_else(ApiResponse::not_found)?;
        let repositories = vec![installation_repository(
            request,
            &state,
            installation,
            permission,
        )?];
        Ok(crate::api::paginate_with(
            request,
            &request.path,
            repositories,
            |total_count, repositories| RepositoryList {
                total_count,
                repository_selection: "selected",
                repositories,
            },
        ))
    }
}
