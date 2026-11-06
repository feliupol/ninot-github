//! Organization self-hosted Actions runners, their labels, and runner groups.
//!
//! A token needs the `admin:org` scope. Membership of the organization is not required, which is
//! how a registration token is issued for the Actions runner and the `gh` CLI.

use std::sync::Mutex;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};

use super::Handler;
use crate::api::{ApiRequest, ApiResponse, ApiResult, PER_PAGE_DEFAULT, PER_PAGE_MAX};
use crate::operation::ops;
use crate::state::{
    Account, AccountKind, CreateRunnerError, Runner, RunnerGroup, RunnerLabel, RunnerSpec,
    RunnerTokenKind, State, lock, valid_label_name, valid_runner_name,
};

/// The caller's organization, or the error GitHub would return.
///
/// Scope is checked after the organization lookup, so an unknown organization is 404 even when
/// the token lacks `admin:org`. An anonymous caller is 401 either way.
fn authorize(state: &State, request: &ApiRequest) -> Result<String, ApiResponse> {
    let caller = request.require_caller()?;
    let org = state
        .account(request.param("org"))
        .filter(|account| account.kind == AccountKind::Organization)
        .ok_or_else(ApiResponse::not_found)?;
    if !caller.has_scope("admin:org") {
        return Err(ApiResponse::error(
            403,
            "Resource not accessible by personal access token",
        ));
    }
    Ok(org.login.clone())
}

fn path_id(request: &ApiRequest, name: &str) -> Result<u64, ApiResponse> {
    request
        .param(name)
        .parse()
        .map_err(|_| ApiResponse::not_found())
}

fn render_label(label: &RunnerLabel) -> Value {
    json!({
        "id": label.id,
        "name": label.name,
        "type": if label.read_only { "read-only" } else { "custom" },
    })
}

fn render_labels(labels: &[RunnerLabel]) -> Value {
    json!({
        "total_count": labels.len(),
        "labels": labels.iter().map(render_label).collect::<Vec<_>>(),
    })
}

fn render_runner(runner: &Runner) -> Value {
    json!({
        "id": runner.id,
        "runner_group_id": runner.group_id,
        "name": runner.name,
        "os": runner.os,
        "status": runner.status,
        "busy": runner.busy,
        "labels": runner.labels.iter().map(render_label).collect::<Vec<_>>(),
        "ephemeral": runner.ephemeral,
    })
}

fn render_group(request: &ApiRequest, org: &str, group: &RunnerGroup) -> Value {
    let api = &request.api_url;
    let id = group.id;
    let mut value = json!({
        "id": id,
        "name": group.name,
        "visibility": group.visibility,
        "default": group.is_default,
        "runners_url": format!("{api}/orgs/{org}/actions/runner-groups/{id}/runners"),
        "inherited": false,
        "allows_public_repositories": group.allows_public_repositories,
        "workflow_restrictions_read_only": false,
        "restricted_to_workflows": group.restricted_to_workflows,
        "selected_workflows": group.selected_workflows,
    });
    if group.visibility == "selected" {
        value["selected_repositories_url"] =
            format!("{api}/orgs/{org}/actions/runner-groups/{id}/repositories").into();
    }
    if let Some(network) = &group.network_configuration_id {
        value["network_configuration_id"] = network.as_str().into();
    }
    value
}

/// One page of a `{total_count, <key>}` list, with the same `Link` header as [`crate::api::paginate`].
fn paged(request: &ApiRequest, items: Vec<Value>, key: &str) -> ApiResponse {
    let per_page = request
        .query("per_page")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(PER_PAGE_DEFAULT)
        .clamp(1, PER_PAGE_MAX);
    let page = request
        .query("page")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(1)
        .max(1);
    let total = items.len();
    let last = total.div_ceil(per_page).max(1);
    let body: Vec<Value> = items
        .into_iter()
        .skip((page - 1) * per_page)
        .take(per_page)
        .collect();
    let mut value = json!({ "total_count": total });
    value[key] = Value::Array(body);
    let link = |target: usize, rel: &str| {
        format!(
            "<{}{}?per_page={per_page}&page={target}>; rel=\"{rel}\"",
            request.api_url, request.path
        )
    };
    let mut links = Vec::new();
    if page < last {
        links.push(link(page + 1, "next"));
        links.push(link(last, "last"));
    }
    if page > 1 {
        links.push(link(1, "first"));
        links.push(link(page - 1, "prev"));
    }
    let response = ApiResponse::ok(value);
    if links.is_empty() {
        response
    } else {
        response.with_header("link", links.join(", "))
    }
}

fn issue_token(state: &Mutex<State>, request: &ApiRequest, kind: RunnerTokenKind) -> ApiResult {
    let mut state = lock(state);
    let org = authorize(&state, request)?;
    let (token, expires_at) = state.issue_runner_token(&org, kind);
    Ok(ApiResponse::json(
        201,
        json!({ "token": token, "expires_at": expires_at }),
    ))
}

fn label_names(body: &Value, min: usize, max: usize) -> Result<Vec<String>, ApiResponse> {
    let Some(labels) = body.get("labels") else {
        return Err(ApiResponse::validation_failed(
            "Runner",
            "labels",
            "missing_field",
        ));
    };
    let Some(labels) = labels.as_array() else {
        return Err(ApiResponse::validation_failed(
            "Runner", "labels", "invalid",
        ));
    };
    if !(min..=max).contains(&labels.len()) {
        return Err(ApiResponse::validation_failed(
            "Runner", "labels", "invalid",
        ));
    }
    let mut names = Vec::with_capacity(labels.len());
    for label in labels {
        let Some(name) = label.as_str() else {
            return Err(ApiResponse::validation_failed(
                "Runner", "labels", "invalid",
            ));
        };
        if !valid_label_name(name) {
            return Err(ApiResponse::validation_failed(
                "Runner", "labels", "invalid",
            ));
        }
        names.push(name.to_owned());
    }
    Ok(names)
}

fn required_runner_name(body: &Value) -> Result<String, ApiResponse> {
    match body.get("name") {
        Some(Value::String(name)) if valid_runner_name(name) => Ok(name.clone()),
        Some(_) => Err(ApiResponse::validation_failed("Runner", "name", "invalid")),
        None => Err(ApiResponse::validation_failed(
            "Runner",
            "name",
            "missing_field",
        )),
    }
}

fn required_u64(body: &Value, field: &str) -> Result<u64, ApiResponse> {
    match body.get(field) {
        Some(value) => value
            .as_u64()
            .ok_or_else(|| ApiResponse::validation_failed("Runner", field, "invalid")),
        None => Err(ApiResponse::validation_failed(
            "Runner",
            field,
            "missing_field",
        )),
    }
}

fn work_folder(body: &Value) -> Result<String, ApiResponse> {
    match body.get("work_folder") {
        None | Some(Value::Null) => Ok("_work".to_owned()),
        Some(Value::String(folder)) if valid_work_folder(folder) => Ok(folder.clone()),
        Some(_) => Err(ApiResponse::validation_failed(
            "Runner",
            "work_folder",
            "invalid",
        )),
    }
}

fn valid_work_folder(folder: &str) -> bool {
    !folder.is_empty() && !folder.chars().any(char::is_control)
}

fn runner_http_error(error: CreateRunnerError) -> ApiResponse {
    match error {
        CreateRunnerError::InvalidName => {
            ApiResponse::validation_failed("Runner", "name", "invalid")
        }
        CreateRunnerError::DuplicateName => {
            ApiResponse::error(409, "A runner with this name already exists.")
        }
        CreateRunnerError::UnknownGroup => ApiResponse::not_found(),
    }
}

fn encode_jit(
    request: &ApiRequest,
    org: &str,
    runner: &Runner,
    group_name: &str,
    work_folder: &str,
) -> String {
    let labels: Vec<&str> = runner
        .labels
        .iter()
        .map(|label| label.name.as_str())
        .collect();
    let payload = json!({
        "gitHubUrl": format!("{}/{org}", request.web_url),
        "runnerName": runner.name,
        "runnerGroup": group_name,
        "runnerLabels": &labels,
        "labels": &labels,
        "runnerId": runner.id,
        "runnerGroupId": runner.group_id,
        "workFolder": work_folder,
        "useV2Flow": true,
    });
    STANDARD.encode(serde_json::to_vec(&payload).unwrap())
}

fn valid_group_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 255 && !name.chars().any(char::is_control)
}

fn required_group_name(body: &Value) -> Result<String, ApiResponse> {
    match body.get("name") {
        Some(Value::String(name)) if valid_group_name(name) => Ok(name.clone()),
        Some(_) => Err(ApiResponse::validation_failed(
            "RunnerGroup",
            "name",
            "invalid",
        )),
        None => Err(ApiResponse::validation_failed(
            "RunnerGroup",
            "name",
            "missing_field",
        )),
    }
}

fn parse_visibility(value: &Value) -> Result<String, ApiResponse> {
    let Some(value) = value.as_str() else {
        return Err(ApiResponse::validation_failed(
            "RunnerGroup",
            "visibility",
            "invalid",
        ));
    };
    if matches!(value, "all" | "selected" | "private") {
        Ok(value.to_owned())
    } else {
        Err(ApiResponse::validation_failed(
            "RunnerGroup",
            "visibility",
            "invalid",
        ))
    }
}

fn optional_bool(body: &Value, field: &str) -> Result<Option<bool>, ApiResponse> {
    match body.get(field) {
        None => Ok(None),
        Some(Value::Bool(value)) => Ok(Some(*value)),
        Some(_) => Err(ApiResponse::validation_failed(
            "RunnerGroup",
            field,
            "invalid",
        )),
    }
}

fn optional_strings(body: &Value, field: &str) -> Result<Option<Vec<String>>, ApiResponse> {
    match body.get(field) {
        None => Ok(None),
        Some(Value::Array(items)) => {
            let mut values = Vec::with_capacity(items.len());
            for item in items {
                let Some(value) = item.as_str() else {
                    return Err(ApiResponse::validation_failed(
                        "RunnerGroup",
                        field,
                        "invalid",
                    ));
                };
                values.push(value.to_owned());
            }
            Ok(Some(values))
        }
        Some(_) => Err(ApiResponse::validation_failed(
            "RunnerGroup",
            field,
            "invalid",
        )),
    }
}

fn parse_ids(items: &[Value], field: &str) -> Result<Vec<u64>, ApiResponse> {
    let mut ids = Vec::with_capacity(items.len());
    for item in items {
        let Some(id) = item.as_u64() else {
            return Err(ApiResponse::validation_failed(
                "RunnerGroup",
                field,
                "invalid",
            ));
        };
        ids.push(id);
    }
    Ok(ids)
}

fn optional_ids(body: &Value, field: &str) -> Result<Option<Vec<u64>>, ApiResponse> {
    match body.get(field) {
        None => Ok(None),
        Some(Value::Array(items)) => Ok(Some(parse_ids(items, field)?)),
        Some(_) => Err(ApiResponse::validation_failed(
            "RunnerGroup",
            field,
            "invalid",
        )),
    }
}

fn required_ids(body: &Value, field: &str) -> Result<Vec<u64>, ApiResponse> {
    match body.get(field) {
        None => Err(ApiResponse::validation_failed(
            "RunnerGroup",
            field,
            "missing_field",
        )),
        Some(Value::Array(items)) => parse_ids(items, field),
        Some(_) => Err(ApiResponse::validation_failed(
            "RunnerGroup",
            field,
            "invalid",
        )),
    }
}

/// Absent leaves the stored value, JSON `null` clears it, and a string sets it.
fn network_on_update(body: &Value) -> Result<Option<Option<String>>, ApiResponse> {
    match body.get("network_configuration_id") {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(Value::String(value)) => Ok(Some(Some(value.clone()))),
        Some(_) => Err(ApiResponse::validation_failed(
            "RunnerGroup",
            "network_configuration_id",
            "invalid",
        )),
    }
}

fn network_on_create(body: &Value) -> Result<Option<String>, ApiResponse> {
    match body.get("network_configuration_id") {
        None => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(ApiResponse::validation_failed(
            "RunnerGroup",
            "network_configuration_id",
            "invalid",
        )),
    }
}

fn group_name_taken(account: &Account, name: &str, except: Option<u64>) -> bool {
    account
        .runner_groups
        .values()
        .any(|group| except != Some(group.id) && group.name.eq_ignore_ascii_case(name))
}

fn group_visible(group: &RunnerGroup, repo_id: u64, private: bool) -> bool {
    if !private && !group.allows_public_repositories {
        return false;
    }
    match group.visibility.as_str() {
        "all" => true,
        "private" => private,
        "selected" => group.repository_ids.contains(&repo_id),
        _ => false,
    }
}

/// `query` is a repository name in the organization, or `owner/name`. An unknown repository
/// yields no groups rather than 404.
fn groups_for<'a>(account: &'a Account, query: Option<&str>) -> Vec<&'a RunnerGroup> {
    let repo = query.map(|visible| {
        let name = visible.rsplit_once('/').map_or(visible, |(_, name)| name);
        account.repo(name).map(|repo| (repo.id, repo.private))
    });
    account
        .runner_groups
        .values()
        .filter(|group| match &repo {
            None => true,
            Some(None) => false,
            Some(Some((id, private))) => group_visible(group, *id, *private),
        })
        .collect()
}

/// Moves `runner_ids` into `group_id`. Runners that this drops from a non-default group return
/// to the default group. Every runner stays in exactly one group, so the default group refuses
/// a list that omits one of its runners.
fn assign_runners(
    account: &mut Account,
    group_id: u64,
    runner_ids: &[u64],
) -> Result<(), ApiResponse> {
    let Some(is_default) = account
        .runner_groups
        .get(&group_id)
        .map(|group| group.is_default)
    else {
        return Err(ApiResponse::not_found());
    };
    if runner_ids
        .iter()
        .any(|id| !account.runners.contains_key(id))
    {
        return Err(ApiResponse::not_found());
    }
    if is_default
        && account
            .runners
            .values()
            .any(|runner| runner.group_id == group_id && !runner_ids.contains(&runner.id))
    {
        return Err(ApiResponse::error(
            422,
            "Runners cannot be removed from the default runner group.",
        ));
    }
    let default_id = account.default_runner_group_id().unwrap();
    for runner in account.runners.values_mut() {
        if runner_ids.contains(&runner.id) {
            runner.group_id = group_id;
        } else if runner.group_id == group_id {
            runner.group_id = default_id;
        }
    }
    Ok(())
}

fn runner_labels(state: &State, org: &str, id: u64) -> Result<Vec<RunnerLabel>, ApiResponse> {
    state
        .account(org)
        .and_then(|account| account.runners.get(&id))
        .map(|runner| runner.labels.clone())
        .ok_or_else(ApiResponse::not_found)
}

impl Handler for ops::ActionsCreateRegistrationTokenForOrg {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        issue_token(state, request, RunnerTokenKind::Registration)
    }
}

impl Handler for ops::ActionsCreateRemoveTokenForOrg {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        issue_token(state, request, RunnerTokenKind::Removal)
    }
}

impl Handler for ops::ActionsListSelfHostedRunnersForOrg {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let org = authorize(&state, request)?;
        let name = request.query("name");
        let runners: Vec<Value> = state
            .account(&org)
            .unwrap()
            .runners
            .values()
            .filter(|runner| name.is_none_or(|wanted| runner.name.eq_ignore_ascii_case(wanted)))
            .map(render_runner)
            .collect();
        Ok(paged(request, runners, "runners"))
    }
}

impl Handler for ops::ActionsGetSelfHostedRunnerForOrg {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let org = authorize(&state, request)?;
        let id = path_id(request, "runner_id")?;
        let runner = state
            .account(&org)
            .unwrap()
            .runners
            .get(&id)
            .ok_or_else(ApiResponse::not_found)?;
        Ok(ApiResponse::ok(render_runner(runner)))
    }
}

impl Handler for ops::ActionsDeleteSelfHostedRunnerFromOrg {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let mut state = lock(state);
        let org = authorize(&state, request)?;
        let id = path_id(request, "runner_id")?;
        if state
            .account_mut(&org)
            .unwrap()
            .runners
            .remove(&id)
            .is_none()
        {
            return Err(ApiResponse::not_found());
        }
        Ok(ApiResponse::no_content())
    }
}

impl Handler for ops::ActionsListLabelsForSelfHostedRunnerForOrg {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let org = authorize(&state, request)?;
        let id = path_id(request, "runner_id")?;
        Ok(ApiResponse::ok(render_labels(&runner_labels(
            &state, &org, id,
        )?)))
    }
}

fn change_labels(
    state: &Mutex<State>,
    request: &ApiRequest,
    replace: bool,
    min: usize,
) -> ApiResult {
    let mut state = lock(state);
    let org = authorize(&state, request)?;
    let id = path_id(request, "runner_id")?;
    let body = request.json_as::<Value>()?;
    let names = label_names(&body, min, 100)?;
    let labels = state.assign_runner_labels(&org, id, &names, replace)?;
    Ok(ApiResponse::ok(render_labels(&labels)))
}

impl Handler for ops::ActionsAddCustomLabelsToSelfHostedRunnerForOrg {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        change_labels(state, request, false, 1)
    }
}

impl Handler for ops::ActionsSetCustomLabelsForSelfHostedRunnerForOrg {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        change_labels(state, request, true, 0)
    }
}

impl Handler for ops::ActionsRemoveAllCustomLabelsFromSelfHostedRunnerForOrg {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let mut state = lock(state);
        let org = authorize(&state, request)?;
        let id = path_id(request, "runner_id")?;
        let labels = state.assign_runner_labels(&org, id, &[], true)?;
        Ok(ApiResponse::ok(render_labels(&labels)))
    }
}

impl Handler for ops::ActionsRemoveCustomLabelFromSelfHostedRunnerForOrg {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let mut state = lock(state);
        let org = authorize(&state, request)?;
        let id = path_id(request, "runner_id")?;
        let labels = state.remove_runner_label(&org, id, request.param("name"))?;
        Ok(ApiResponse::ok(render_labels(&labels)))
    }
}

impl Handler for ops::ActionsGenerateRunnerJitconfigForOrg {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let mut state = lock(state);
        let org = authorize(&state, request)?;
        let body = request.json_as::<Value>()?;
        let name = required_runner_name(&body)?;
        let group_id = required_u64(&body, "runner_group_id")?;
        let labels = label_names(&body, 1, 100)?;
        let folder = work_folder(&body)?;
        let id = state
            .insert_runner(
                &org,
                RunnerSpec {
                    name: &name,
                    os: "unknown",
                    status: "offline",
                    ephemeral: true,
                    group_id,
                    labels: &labels,
                    label_os: None,
                },
            )
            .map_err(runner_http_error)?;
        let account = state.account(&org).unwrap();
        let runner = &account.runners[&id];
        let group_name = account.runner_groups[&group_id].name.clone();
        let encoded = encode_jit(request, &org, runner, &group_name, &folder);
        let body = json!({
            "runner": render_runner(runner),
            "encoded_jit_config": encoded,
        });
        Ok(ApiResponse::json(201, body))
    }
}

impl Handler for ops::ActionsListSelfHostedRunnerGroupsForOrg {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let mut state = lock(state);
        let org = authorize(&state, request)?;
        state.ensure_default_runner_group(&org);
        let account = state.account(&org).unwrap();
        let groups = groups_for(account, request.query("visible_to_repository"))
            .into_iter()
            .map(|group| render_group(request, &org, group))
            .collect();
        Ok(paged(request, groups, "runner_groups"))
    }
}

impl Handler for ops::ActionsCreateSelfHostedRunnerGroupForOrg {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let mut state = lock(state);
        let org = authorize(&state, request)?;
        let body = request.json_as::<Value>()?;
        let name = required_group_name(&body)?;
        let visibility = match body.get("visibility") {
            None => "all".to_owned(),
            Some(value) => parse_visibility(value)?,
        };
        let allows_public = optional_bool(&body, "allows_public_repositories")?.unwrap_or(false);
        let restricted = optional_bool(&body, "restricted_to_workflows")?.unwrap_or(false);
        let workflows = optional_strings(&body, "selected_workflows")?.unwrap_or_default();
        let repository_ids = optional_ids(&body, "selected_repository_ids")?.unwrap_or_default();
        let runners = optional_ids(&body, "runners")?.unwrap_or_default();
        let network = network_on_create(&body)?;
        state.ensure_default_runner_group(&org);
        {
            let account = state.account(&org).unwrap();
            if group_name_taken(account, &name, None) {
                return Err(ApiResponse::validation_failed(
                    "RunnerGroup",
                    "name",
                    "custom",
                ));
            }
            if repository_ids
                .iter()
                .any(|id| account.repos().all(|repo| repo.id != *id))
            {
                return Err(ApiResponse::validation_failed(
                    "RunnerGroup",
                    "selected_repository_ids",
                    "invalid",
                ));
            }
            if runners.iter().any(|id| !account.runners.contains_key(id)) {
                return Err(ApiResponse::not_found());
            }
        }
        let id = state.next_id();
        {
            let account = state.account_mut(&org).unwrap();
            account.runner_groups.insert(
                id,
                RunnerGroup {
                    id,
                    name,
                    visibility,
                    is_default: false,
                    allows_public_repositories: allows_public,
                    restricted_to_workflows: restricted,
                    selected_workflows: workflows,
                    network_configuration_id: network,
                    repository_ids,
                },
            );
            if let Err(error) = assign_runners(account, id, &runners) {
                account.runner_groups.remove(&id);
                return Err(error);
            }
        }
        let account = state.account(&org).unwrap();
        Ok(ApiResponse::json(
            201,
            render_group(request, &org, &account.runner_groups[&id]),
        ))
    }
}

impl Handler for ops::ActionsGetSelfHostedRunnerGroupForOrg {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let org = authorize(&state, request)?;
        let id = path_id(request, "runner_group_id")?;
        let group = state
            .account(&org)
            .unwrap()
            .runner_groups
            .get(&id)
            .ok_or_else(ApiResponse::not_found)?;
        Ok(ApiResponse::ok(render_group(request, &org, group)))
    }
}

impl Handler for ops::ActionsUpdateSelfHostedRunnerGroupForOrg {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let mut state = lock(state);
        let org = authorize(&state, request)?;
        let id = path_id(request, "runner_group_id")?;
        if !state.account(&org).unwrap().runner_groups.contains_key(&id) {
            return Err(ApiResponse::not_found());
        }
        let body = request.json_as::<Value>()?;
        let name = required_group_name(&body)?;
        let visibility = match body.get("visibility") {
            None => None,
            Some(value) => Some(parse_visibility(value)?),
        };
        let allows_public = optional_bool(&body, "allows_public_repositories")?;
        let restricted = optional_bool(&body, "restricted_to_workflows")?;
        let workflows = optional_strings(&body, "selected_workflows")?;
        let network = network_on_update(&body)?;
        if group_name_taken(state.account(&org).unwrap(), &name, Some(id)) {
            return Err(ApiResponse::validation_failed(
                "RunnerGroup",
                "name",
                "custom",
            ));
        }
        {
            let group = state
                .account_mut(&org)
                .unwrap()
                .runner_groups
                .get_mut(&id)
                .unwrap();
            group.name = name;
            if let Some(visibility) = visibility {
                group.visibility = visibility;
            }
            if let Some(allows_public) = allows_public {
                group.allows_public_repositories = allows_public;
            }
            if let Some(restricted) = restricted {
                group.restricted_to_workflows = restricted;
            }
            if let Some(workflows) = workflows {
                group.selected_workflows = workflows;
            }
            if let Some(network) = network {
                group.network_configuration_id = network;
            }
        }
        let account = state.account(&org).unwrap();
        Ok(ApiResponse::ok(render_group(
            request,
            &org,
            &account.runner_groups[&id],
        )))
    }
}

impl Handler for ops::ActionsDeleteSelfHostedRunnerGroupFromOrg {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let mut state = lock(state);
        let org = authorize(&state, request)?;
        let id = path_id(request, "runner_group_id")?;
        let default_id = state.ensure_default_runner_group(&org);
        let account = state.account_mut(&org).unwrap();
        let Some(is_default) = account.runner_groups.get(&id).map(|group| group.is_default) else {
            return Err(ApiResponse::not_found());
        };
        if is_default {
            return Err(ApiResponse::error(
                403,
                "Cannot delete the default runner group.",
            ));
        }
        for runner in account.runners.values_mut() {
            if runner.group_id == id {
                runner.group_id = default_id;
            }
        }
        account.runner_groups.remove(&id);
        Ok(ApiResponse::no_content())
    }
}

impl Handler for ops::ActionsSetSelfHostedRunnersInGroupForOrg {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let mut state = lock(state);
        let org = authorize(&state, request)?;
        let id = path_id(request, "runner_group_id")?;
        if !state.account(&org).unwrap().runner_groups.contains_key(&id) {
            return Err(ApiResponse::not_found());
        }
        let body = request.json_as::<Value>()?;
        let runners = required_ids(&body, "runners")?;
        state.ensure_default_runner_group(&org);
        assign_runners(state.account_mut(&org).unwrap(), id, &runners)?;
        Ok(ApiResponse::no_content())
    }
}
