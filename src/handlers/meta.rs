//! Server metadata operations.

use std::sync::Mutex;

use super::Handler;
use crate::api::{ApiRequest, ApiResponse, ApiResult};
use crate::operation::ops;
use crate::rate_limit::Resource;
use crate::state::{State, lock};
use crate::types;

/// The subset of `GET /meta` that `gh` reads.
#[derive(serde::Serialize)]
struct Meta {
    verifiable_password_authentication: bool,
    installed_version: &'static str,
}

/// API metadata. `installed_version` makes `gh` treat this host as GHES 3.21, which
/// opts `gh workflow run` into `return_run_details`.
impl Handler for ops::MetaGet {
    fn handle(_: &Mutex<State>, _: &ApiRequest) -> ApiResult {
        Ok(ApiResponse::ok_typed(&Meta {
            verifiable_password_authentication: true,
            installed_version: "3.21.0",
        }))
    }
}

impl Handler for ops::MetaGetZen {
    fn handle(_: &Mutex<State>, _: &ApiRequest) -> ApiResult {
        Ok(ApiResponse::bytes_ok(
            "text/plain;charset=utf-8",
            b"Keep it logically awesome.".to_vec(),
        ))
    }
}

/// The caller's windows, without counting against them.
// Source: <https://docs.github.com/en/rest/rate-limit/rate-limit#get-rate-limit-status-for-the-authenticated-user>
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
        Ok(ApiResponse::ok_typed(&types::RateLimitOverview {
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
        }))
    }
}

/// The API's index of URL templates, which clients such as `gh auth status` probe.
impl Handler for ops::MetaRoot {
    fn handle(_: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let urls = request.urls();
        let search = |kind: &str| {
            urls.api(&format!(
                "/search/{kind}?q={{query}}{{&page,per_page,sort,order}}"
            ))
        };
        Ok(ApiResponse::ok_typed(&types::Root {
            authorizations_url: urls.api("/authorizations"),
            code_search_url: search("code"),
            commit_search_url: search("commits"),
            current_user_authorizations_html_url: urls
                .web("/settings/connections/applications{/client_id}"),
            current_user_repositories_url: urls.api("/user/repos{?type,page,per_page,sort}"),
            current_user_url: urls.api("/user"),
            emails_url: urls.api("/user/emails"),
            emojis_url: urls.api("/emojis"),
            events_url: urls.api("/events"),
            feeds_url: urls.api("/feeds"),
            followers_url: urls.api("/user/followers"),
            following_url: urls.api("/user/following{/target}"),
            gists_url: urls.api("/gists{/gist_id}"),
            hub_url: None,
            issue_search_url: search("issues"),
            issues_url: urls.api("/issues"),
            keys_url: urls.api("/user/keys"),
            label_search_url: urls
                .api("/search/labels?q={query}&repository_id={repository_id}{&page,per_page}"),
            notifications_url: urls.api("/notifications"),
            organization_repositories_url: urls.api("/orgs/{org}/repos{?type,page,per_page,sort}"),
            organization_teams_url: urls.api("/orgs/{org}/teams"),
            organization_url: urls.api("/orgs/{org}"),
            public_gists_url: urls.api("/gists/public"),
            rate_limit_url: urls.api("/rate_limit"),
            repository_search_url: search("repositories"),
            repository_url: urls.api("/repos/{owner}/{repo}"),
            starred_gists_url: urls.api("/gists/starred"),
            starred_url: urls.api("/user/starred{/owner}{/repo}"),
            topic_search_url: None,
            user_organizations_url: urls.api("/user/orgs"),
            user_repositories_url: urls.api("/users/{user}/repos{?type,page,per_page,sort}"),
            user_search_url: urls.api("/search/users?q={query}{&page,per_page}"),
            user_url: urls.api("/users/{user}"),
        }))
    }
}
