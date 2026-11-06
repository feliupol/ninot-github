//! Server metadata operations.

use std::sync::Mutex;

use super::Handler;
use crate::api::{ApiRequest, ApiResponse, ApiResult};
use crate::operation::ops;
use crate::rate_limit::Resource;
use crate::state::{State, lock};
use crate::types;

impl Handler for ops::MetaGetZen {
    fn handle(_: &Mutex<State>, _: &ApiRequest) -> ApiResult {
        Ok(ApiResponse::bytes(
            200,
            "text/plain;charset=utf-8",
            b"Keep it logically awesome.".to_vec(),
        ))
    }
}

/// The caller's windows, without counting against them.
impl Handler for ops::RateLimitGet {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let client = request
            .client
            .as_ref()
            .expect("API requests carry their client");
        let mut state = lock(state);
        let mut window = |resource| {
            let window = state.rate_limits.window(client, resource);
            types::RateLimit {
                limit: window.limit.into(),
                remaining: window.remaining().into(),
                reset: window.reset as i64,
                used: window.used.into(),
            }
        };
        let core = window(Resource::Core);
        let search = window(Resource::Search);
        let graphql = window(Resource::Graphql);
        Ok(ApiResponse::typed(
            200,
            &types::RateLimitOverview {
                rate: core.clone(),
                resources: types::RateLimitOverviewResources {
                    actions_runner_registration: None,
                    code_scanning_autofix: None,
                    code_search: None,
                    copilot_usage_records: None,
                    core,
                    dependency_sbom: None,
                    dependency_snapshots: None,
                    graphql: Some(graphql),
                    integration_manifest: None,
                    scim: None,
                    search,
                    source_import: None,
                },
            },
        ))
    }
}

/// The API's index of URL templates, which clients such as `gh auth status` probe.
impl Handler for ops::MetaRoot {
    fn handle(_: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let api = &request.api_url;
        let web = &request.web_url;
        let search =
            |kind: &str| format!("{api}/search/{kind}?q={{query}}{{&page,per_page,sort,order}}");
        Ok(ApiResponse::typed(
            200,
            &types::Root {
                authorizations_url: format!("{api}/authorizations"),
                code_search_url: search("code"),
                commit_search_url: search("commits"),
                current_user_authorizations_html_url: format!(
                    "{web}/settings/connections/applications{{/client_id}}"
                ),
                current_user_repositories_url: format!(
                    "{api}/user/repos{{?type,page,per_page,sort}}"
                ),
                current_user_url: format!("{api}/user"),
                emails_url: format!("{api}/user/emails"),
                emojis_url: format!("{api}/emojis"),
                events_url: format!("{api}/events"),
                feeds_url: format!("{api}/feeds"),
                followers_url: format!("{api}/user/followers"),
                following_url: format!("{api}/user/following{{/target}}"),
                gists_url: format!("{api}/gists{{/gist_id}}"),
                hub_url: None,
                issue_search_url: search("issues"),
                issues_url: format!("{api}/issues"),
                keys_url: format!("{api}/user/keys"),
                label_search_url: format!(
                    "{api}/search/labels?q={{query}}&repository_id={{repository_id}}{{&page,per_page}}"
                ),
                notifications_url: format!("{api}/notifications"),
                organization_repositories_url: format!(
                    "{api}/orgs/{{org}}/repos{{?type,page,per_page,sort}}"
                ),
                organization_teams_url: format!("{api}/orgs/{{org}}/teams"),
                organization_url: format!("{api}/orgs/{{org}}"),
                public_gists_url: format!("{api}/gists/public"),
                rate_limit_url: format!("{api}/rate_limit"),
                repository_search_url: search("repositories"),
                repository_url: format!("{api}/repos/{{owner}}/{{repo}}"),
                starred_gists_url: format!("{api}/gists/starred"),
                starred_url: format!("{api}/user/starred{{/owner}}{{/repo}}"),
                topic_search_url: None,
                user_organizations_url: format!("{api}/user/orgs"),
                user_repositories_url: format!(
                    "{api}/users/{{user}}/repos{{?type,page,per_page,sort}}"
                ),
                user_search_url: format!("{api}/search/users?q={{query}}{{&page,per_page}}"),
                user_url: format!("{api}/users/{{user}}"),
            },
        ))
    }
}
