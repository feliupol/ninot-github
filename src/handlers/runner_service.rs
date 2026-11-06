//! The runner-admin endpoints the official Actions runner's `config.sh` calls.
//!
//! The runner authenticates them with `Authorization: RemoteAuth <token>`, where the token is
//! an organization registration or removal token, and they are outside the REST spec. Three of
//! their paths are also REST operations, so requests are told apart by that scheme.

use std::sync::Mutex;

use http::{HeaderMap, Method};
use serde_json::{Value, json};

use crate::api::ApiResponse;
use crate::state::{AccountKind, RunnerSpec, RunnerTokenKind, State, lock};

/// Answers a request carrying a `RemoteAuth` token, or `None` for any other request.
///
/// `path` has the `/api/v3` prefix removed.
pub(crate) fn serve(
    state: &Mutex<State>,
    method: &Method,
    headers: &HeaderMap,
    path: &str,
    query: Option<&str>,
    body: &[u8],
    web_url: &str,
) -> Option<ApiResponse> {
    let token = headers
        .get(http::header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("RemoteAuth ")?
        .trim();
    let mut state = lock(state);
    let segments: Vec<&str> = path.trim_matches('/').split('/').collect();
    let response = match (method.as_str(), segments.as_slice()) {
        ("POST", ["actions", "runner-registration"]) => {
            tenant_credential(&state, token, body, web_url)
        }
        ("POST", ["actions", "runners", "register"]) => {
            add_runner(&mut state, token, body, web_url)
        }
        ("GET", ["orgs", org, "actions", "runner-groups"]) => runner_groups(&mut state, token, org),
        ("GET", ["orgs", org, "actions", "runners"]) => runners_named(&state, token, org, query),
        ("DELETE", ["orgs", org, "actions", "runners", id]) => {
            delete_runner(&mut state, token, org, id)
        }
        _ => ApiResponse::not_found(),
    };
    Some(response)
}

/// The organization login at the end of a runner's `--url`, such as `https://host/org`.
fn org_of_url(body: &Value) -> Option<String> {
    let url = url::Url::parse(body.get("url")?.as_str()?).ok()?;
    let mut segments = url.path_segments()?.filter(|segment| !segment.is_empty());
    let org = segments.next()?;
    segments.next().is_none().then(|| org.to_lowercase())
}

/// The token's organization when `org` matches it and is an organization.
fn token_org(state: &State, token: &str, kind: RunnerTokenKind, org: &str) -> Option<String> {
    let token_org = state.runner_token_org(token, kind)?;
    let is_org = state
        .account(&token_org)
        .is_some_and(|account| account.kind == AccountKind::Organization);
    (is_org && token_org == org.to_lowercase()).then_some(token_org)
}

fn parse_body(body: &[u8]) -> Value {
    serde_json::from_slice(body).unwrap_or(Value::Null)
}

/// `POST /actions/runner-registration`: where the runner finds the Actions service.
fn tenant_credential(state: &State, token: &str, body: &[u8], web_url: &str) -> ApiResponse {
    let body = parse_body(body);
    let event = body.get("runner_event").and_then(Value::as_str);
    let kind = match event {
        Some("register") => RunnerTokenKind::Registration,
        Some("remove") => RunnerTokenKind::Removal,
        _ => return ApiResponse::error(422, "runner_event must be register or remove"),
    };
    let Some(org) = org_of_url(&body) else {
        return ApiResponse::error(422, "url must name an organization");
    };
    if token_org(state, token, kind, &org).is_none() {
        return ApiResponse::not_found();
    }
    ApiResponse::ok(json!({
        "url": format!("{web_url}/_services/pipelines"),
        "token_schema": "OAuthAccessToken",
        "token": token,
        "use_v2_flow": true,
    }))
}

/// `GET /orgs/{org}/actions/runner-groups`: the groups a new runner can join.
fn runner_groups(state: &mut State, token: &str, org: &str) -> ApiResponse {
    let Some(org) = token_org(state, token, RunnerTokenKind::Registration, org) else {
        return ApiResponse::not_found();
    };
    state.ensure_default_runner_group(&org);
    let groups: Vec<Value> = state
        .account(&org)
        .unwrap()
        .runner_groups
        .values()
        .map(|group| {
            json!({
                "id": group.id,
                "name": group.name,
                "default": group.is_default,
                "is_hosted": false,
            })
        })
        .collect();
    ApiResponse::ok(json!({ "total_count": groups.len(), "runner_groups": groups }))
}

/// `GET /orgs/{org}/actions/runners?name=`: runners with that name, to detect a duplicate.
fn runners_named(state: &State, token: &str, org: &str, query: Option<&str>) -> ApiResponse {
    let Some(org) = token_org(state, token, RunnerTokenKind::Registration, org)
        .or_else(|| token_org(state, token, RunnerTokenKind::Removal, org))
    else {
        return ApiResponse::not_found();
    };
    let name = url::form_urlencoded::parse(query.unwrap_or("").as_bytes())
        .find(|(key, _)| key == "name")
        .map(|(_, value)| value.into_owned());
    let runners: Vec<Value> = state
        .account(&org)
        .unwrap()
        .runners
        .values()
        .filter(|runner| {
            name.as_deref()
                .is_none_or(|name| runner.name.eq_ignore_ascii_case(name))
        })
        .map(|runner| json!({ "id": runner.id, "name": runner.name }))
        .collect();
    ApiResponse::ok(json!({ "total_count": runners.len(), "runners": runners }))
}

/// `POST /actions/runners/register`: adds the runner and says how it authenticates.
fn add_runner(state: &mut State, token: &str, body: &[u8], web_url: &str) -> ApiResponse {
    let body = parse_body(body);
    let Some(org) = org_of_url(&body) else {
        return ApiResponse::error(422, "url must name an organization");
    };
    if token_org(state, token, RunnerTokenKind::Registration, &org).is_none() {
        return ApiResponse::not_found();
    }
    let Some(name) = body.get("name").and_then(Value::as_str) else {
        return ApiResponse::error(422, "name is required");
    };
    let labels: Vec<&str> = body
        .get("labels")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|label| label.get("name").and_then(Value::as_str))
        .collect();
    let os = labels
        .iter()
        .find_map(|label| match label.to_ascii_lowercase().as_str() {
            "linux" => Some("linux"),
            "macos" => Some("macos"),
            "windows" => Some("windows"),
            _ => None,
        })
        .unwrap_or("linux");
    let ephemeral = body.get("ephemeral").and_then(Value::as_bool) == Some(true);
    let group_id = match body.get("group_id").and_then(Value::as_u64) {
        Some(id) => id,
        None => state.ensure_default_runner_group(&org),
    };
    let names: Vec<String> = labels.iter().map(|label| (*label).to_owned()).collect();
    let id = match state.insert_runner(
        &org,
        RunnerSpec {
            name,
            os,
            status: "offline",
            ephemeral,
            group_id,
            labels: &names,
            label_os: Some(os),
        },
    ) {
        Ok(id) => id,
        Err(_) => return ApiResponse::error(422, "could not add the runner"),
    };
    ApiResponse::ok(json!({
        "id": id,
        "name": name,
        "authorization": {
            "authorization_url": format!("{web_url}/_services/token/oauth2/token"),
            "server_url": format!("{web_url}/_services/broker/"),
            "client_id": format!("00000000-0000-4000-8000-{id:012x}"),
        },
    }))
}

/// `DELETE /orgs/{org}/actions/runners/{id}` with a removal token.
fn delete_runner(state: &mut State, token: &str, org: &str, id: &str) -> ApiResponse {
    let Some(org) = token_org(state, token, RunnerTokenKind::Removal, org) else {
        return ApiResponse::not_found();
    };
    let Ok(id) = id.parse::<u64>() else {
        return ApiResponse::not_found();
    };
    match state.account_mut(&org).unwrap().runners.remove(&id) {
        Some(_) => ApiResponse::no_content(),
        None => ApiResponse::not_found(),
    }
}
