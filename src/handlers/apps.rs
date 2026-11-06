//! GitHub App installation discovery and token exchange.

use std::sync::Mutex;

use super::Handler;
use crate::api::{ApiRequest, ApiResponse, ApiResult, Caller};
use crate::operation::ops;
use crate::state::{State, lock};
use crate::types;

fn app_permissions(secrets: Option<types::AppPermissionsSecrets>) -> types::AppPermissions {
    types::AppPermissions {
        actions: None,
        administration: None,
        artifact_metadata: None,
        attestations: None,
        checks: None,
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
        metadata: None,
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
        secrets,
        security_events: None,
        single_file: None,
        starring: None,
        statuses: None,
        vulnerability_alerts: None,
        workflows: None,
    }
}

impl Handler for ops::AppsGetRepoInstallation {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
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
        let account = state
            .account(&installation.owner)
            .expect("installation owner exists");
        let timestamp = crate::render::timestamp(&crate::state::now());
        Ok(ApiResponse::typed(
            200,
            &types::AppsGetRepoInstallationResponse(types::Installation {
                access_tokens_url: format!(
                    "{}/app/installations/{}/access_tokens",
                    request.api_url, installation.id
                ),
                account: Some(types::InstallationAccount::SimpleUser(
                    crate::render::simple_user(request, account),
                )),
                app_id: 1,
                app_slug: "ninot-test".to_owned(),
                client_id: None,
                contact_email: None,
                created_at: timestamp,
                events: Vec::new(),
                has_multiple_single_files: Some(false),
                html_url: format!(
                    "{}/settings/installations/{}",
                    request.web_url, installation.id
                ),
                id: crate::render::id(installation.id),
                permissions: app_permissions(installation.secrets_permission),
                repositories_url: format!("{}/installation/repositories", request.api_url),
                repository_selection: types::InstallationRepositorySelection::Selected,
                single_file_name: None,
                single_file_paths: Vec::new(),
                suspended_at: None,
                suspended_by: None,
                target_id: crate::render::id(account.id),
                target_type: account.kind.as_str().to_owned(),
                updated_at: timestamp,
            }),
        ))
    }
}

impl Handler for ops::AppsCreateInstallationAccessToken {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
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
            .map_err(|_| ApiResponse::error(500, "Repository id exceeds API limits"))?;
        let repository_is_supported = body
            .repositories
            .iter()
            .all(|name| name.eq_ignore_ascii_case(&installation.repository))
            && body
                .repository_ids
                .iter()
                .all(|requested_id| *requested_id == repository_id);
        if !repository_is_supported {
            return Err(ApiResponse::error(
                422,
                "Requested repositories exceed the installation grant",
            ));
        }
        let requested_secrets = body
            .permissions
            .as_ref()
            .map(|permissions| permissions.secrets);
        if let Some(Some(requested)) = requested_secrets {
            let granted = installation.secrets_permission;
            let within_grant = matches!(
                (granted, requested),
                (
                    Some(types::AppPermissionsSecrets::Read),
                    types::AppPermissionsSecrets::Read
                ) | (Some(types::AppPermissionsSecrets::Write), _)
            );
            if !within_grant {
                return Err(ApiResponse::error(
                    422,
                    "Requested permissions exceed the installation grant",
                ));
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
            "installation:{}/{}",
            installation.owner, installation.repository
        );
        let secrets_scopes = match effective_secrets {
            Some(types::AppPermissionsSecrets::Read) => vec!["actions:secrets:read".to_owned()],
            Some(types::AppPermissionsSecrets::Write) => vec![
                "actions:secrets:read".to_owned(),
                "actions:secrets:write".to_owned(),
            ],
            None => Vec::new(),
        };
        state.tokens.insert(
            token.clone(),
            Caller {
                login: installation.owner.clone(),
                scopes: std::iter::once(installation_scope)
                    .chain(secrets_scopes)
                    .collect(),
            },
        );
        Ok(ApiResponse::typed(
            201,
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
