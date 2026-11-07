//! Check suite create, read, list, rerequest, and repository preferences.

use std::sync::Mutex;

use super::Handler;
use super::checks_model::{
    checks_forbidden, commit_key, mark_rerequested, open_suite, render_suite, require_checks_app,
    select_runs, suite_runs,
};
use super::repos::find_repo;
use crate::api::{ApiRequest, ApiResponse, ApiResult, paginate_with};
use crate::operation::ops;
use crate::render;
use crate::state::{State, lock};
use crate::types;

impl Handler for ops::ChecksCreateSuite {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let body: types::ChecksCreateSuiteRequest = request.json_as()?;
        if body.head_sha.is_empty() {
            return Err(ApiResponse::validation_failed(
                "CheckSuite",
                "head_sha",
                "missing_field",
            ));
        }
        let mut state = lock(state);
        let found = find_repo(&state, request)?;
        let app_id = require_checks_app(request)?;
        let (suite_id, created) = open_suite(&mut state, &found, app_id, &body.head_sha);
        let suite = render_suite(&state, request, &found, suite_id)?;
        if created {
            let mut ctx =
                crate::webhooks::events::Ctx::new(&mut state, request, &found.owner, &found.name);
            crate::webhooks::events::check_suite::requested_suite(&mut ctx, &suite);
        }
        let status = if created { 201 } else { 200 };
        Ok(ApiResponse::typed(
            status,
            &types::ChecksCreateSuiteResponse(suite),
        ))
    }
}

impl Handler for ops::ChecksSetSuitesPreferences {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        request.require_caller()?;
        let body: types::ChecksSetSuitesPreferencesRequest = request.json_as()?;
        let mut state = lock(state);
        let found = find_repo(&state, request)?;
        if !found.permission.admin {
            return Err(ApiResponse::error(
                403,
                "Must have admin rights to Repository.",
            ));
        }
        if let Some(items) = body.auto_trigger_checks {
            let repo = state
                .account_mut(&found.owner)
                .and_then(|account| account.repo_mut(&found.name))
                .expect("repository exists");
            for item in items {
                repo.check_suite_preferences
                    .insert(item.app_id, item.setting);
            }
        }
        let preference = preference_response(&state, request, &found);
        Ok(ApiResponse::typed(
            200,
            &types::ChecksSetSuitesPreferencesResponse(preference),
        ))
    }
}

impl Handler for ops::ChecksGetSuite {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let found = find_repo(&state, request)?;
        let id = parse_id(request.param("check_suite_id"))?;
        let suite = render_suite(&state, request, &found, id)?;
        Ok(ApiResponse::typed(
            200,
            &types::ChecksGetSuiteResponse(suite),
        ))
    }
}

impl Handler for ops::ChecksListForSuite {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let found = find_repo(&state, request)?;
        let id = parse_id(request.param("check_suite_id"))?;
        let repo = state
            .account(&found.owner)
            .and_then(|account| account.repo(&found.name))
            .expect("repository exists");
        if !repo.check_suites.contains_key(&id) {
            return Err(ApiResponse::not_found());
        }
        let runs = select_runs(suite_runs(repo, id), request);
        Ok(paginate_with(
            request,
            &request.path,
            runs,
            |total_count, check_runs| types::ChecksListForSuiteResponse {
                total_count,
                check_runs,
            },
        ))
    }
}

impl Handler for ops::ChecksRerequestSuite {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let id = parse_id(request.param("check_suite_id"))?;
        let mut state = lock(state);
        let found = find_repo(&state, request)?;
        let app_id = require_checks_app(request)?;
        let owner_app = state
            .account(&found.owner)
            .and_then(|account| account.repo(&found.name))
            .and_then(|repo| repo.check_suites.get(&id))
            .map(|suite| suite.app_id);
        let Some(owner_app) = owner_app else {
            return Err(ApiResponse::not_found());
        };
        if owner_app != app_id {
            return Err(checks_forbidden(request));
        }
        {
            let repo = state
                .account_mut(&found.owner)
                .and_then(|account| account.repo_mut(&found.name))
                .expect("repository exists");
            mark_rerequested(repo, id);
        }
        let suite = render_suite(&state, request, &found, id)?;
        let mut ctx =
            crate::webhooks::events::Ctx::new(&mut state, request, &found.owner, &found.name);
        crate::webhooks::events::check_suite::rerequested(&mut ctx, &suite);
        Ok(ApiResponse::typed(
            201,
            &types::ChecksRerequestSuiteResponse(types::EmptyObject {}),
        ))
    }
}

impl Handler for ops::ChecksListSuitesForRef {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let found = find_repo(&state, request)?;
        let ids = {
            let repo = state
                .account(&found.owner)
                .and_then(|account| account.repo(&found.name))
                .expect("repository exists");
            let sha = commit_key(repo, request.param("ref"));
            let mut ids: Vec<i64> = repo
                .check_suites
                .values()
                .filter(|suite| suite.head_sha == sha)
                .map(|suite| suite.id)
                .collect();
            if let Some(app) = request.query("app_id") {
                let Ok(app_id) = app.parse::<i64>() else {
                    return Ok(page_suites(request, Vec::new()));
                };
                ids.retain(|id| {
                    repo.check_suites
                        .get(id)
                        .is_some_and(|suite| suite.app_id == app_id)
                });
            }
            if let Some(name) = request.query("check_name") {
                ids.retain(|id| suite_runs(repo, *id).iter().any(|run| run.name == name));
            }
            ids.sort_unstable();
            ids
        };
        let suites = ids
            .into_iter()
            .map(|id| render_suite(&state, request, &found, id))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(page_suites(request, suites))
    }
}

fn page_suites(request: &ApiRequest, suites: Vec<types::CheckSuite>) -> ApiResponse {
    paginate_with(
        request,
        &request.path,
        suites,
        |total_count, check_suites| types::ChecksListSuitesForRefResponse {
            total_count,
            check_suites,
        },
    )
}

fn preference_response(
    state: &State,
    request: &ApiRequest,
    found: &super::repos::Found,
) -> types::CheckSuitePreference {
    let account = state
        .account(&found.owner)
        .expect("repository owner exists");
    let repo = account.repo(&found.name).expect("repository exists");
    let auto_trigger_checks = repo
        .check_suite_preferences
        .iter()
        .map(
            |(app_id, setting)| types::CheckSuitePreferencePreferencesAutoTriggerChecksItem {
                app_id: *app_id,
                setting: *setting,
            },
        )
        .collect();
    types::CheckSuitePreference {
        preferences: types::CheckSuitePreferencePreferences {
            auto_trigger_checks,
        },
        repository: render::minimal_repository(request, account, repo, found.permission),
    }
}

fn parse_id(value: &str) -> Result<i64, ApiResponse> {
    value.parse().map_err(|_| ApiResponse::not_found())
}
