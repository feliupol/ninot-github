//! Repository and environment Actions variables.
//!
//! A variable name is stored uppercase and matched without regard to case.

use std::collections::BTreeMap;
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::Handler;
use super::actions_config::{already_exists, variable_name};
use super::environments::environment;
use super::repo_admin::admin_repo;
use crate::api::{ApiRequest, ApiResponse, ApiResult, paginate_with};
use crate::operation::ops;
use crate::state::{ActionsVariable, State, lock};
use crate::types;

#[derive(Serialize)]
struct VariableBody {
    name: String,
    value: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

fn render(variable: &ActionsVariable) -> VariableBody {
    VariableBody {
        name: variable.name.clone(),
        value: variable.value.clone(),
        created_at: variable.created_at,
        updated_at: variable.updated_at,
    }
}

#[derive(Serialize)]
struct VariablePage {
    total_count: i64,
    variables: Vec<VariableBody>,
}

#[derive(Deserialize)]
struct CreateBody {
    name: String,
    value: String,
}

#[derive(Deserialize)]
struct UpdateBody {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    value: Option<String>,
}

fn page(request: &ApiRequest, variables: &BTreeMap<String, ActionsVariable>) -> ApiResult {
    let variables = variables.values().map(render).collect();
    Ok(paginate_with(
        request,
        &request.path,
        variables,
        |total_count, variables| VariablePage {
            total_count,
            variables,
        },
    ))
}

fn create(
    map: &mut BTreeMap<String, ActionsVariable>,
    body: CreateBody,
) -> Result<(), ApiResponse> {
    let name = variable_name(&body.name);
    if map.contains_key(&name) {
        return Err(already_exists());
    }
    let now = crate::time::now_utc();
    map.insert(
        name.clone(),
        ActionsVariable {
            name,
            value: body.value,
            created_at: now,
            updated_at: now,
        },
    );
    Ok(())
}

fn update(
    map: &mut BTreeMap<String, ActionsVariable>,
    url_name: &str,
    body: UpdateBody,
) -> Result<(), ApiResponse> {
    let key = variable_name(url_name);
    if !map.contains_key(&key) {
        return Err(ApiResponse::not_found());
    }
    let new_key = body
        .name
        .as_deref()
        .map(variable_name)
        .unwrap_or_else(|| key.clone());
    if new_key != key && map.contains_key(&new_key) {
        return Err(already_exists());
    }
    let mut variable = map.remove(&key).expect("variable found above");
    variable.name.clone_from(&new_key);
    if let Some(value) = body.value {
        variable.value = value;
    }
    variable.updated_at = crate::time::now_utc();
    map.insert(new_key, variable);
    Ok(())
}

fn remove(map: &mut BTreeMap<String, ActionsVariable>, name: &str) -> Result<(), ApiResponse> {
    map.remove(&variable_name(name))
        .map(|_| ())
        .ok_or_else(ApiResponse::not_found)
}

fn created() -> ApiResult {
    Ok(ApiResponse::created(&types::EmptyObject {}))
}

impl Handler for ops::ActionsListRepoVariables {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let found = admin_repo(&state, request)?;
        page(request, &found.repo(&state).setup.variables)
    }
}

impl Handler for ops::ActionsCreateRepoVariable {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let body: CreateBody = request.json_as()?;
        let mut state = lock(state);
        let found = admin_repo(&state, request)?;
        create(&mut found.repo_mut(&mut state).setup.variables, body)?;
        created()
    }
}

impl Handler for ops::ActionsGetRepoVariable {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let found = admin_repo(&state, request)?;
        let variable = found
            .repo(&state)
            .setup
            .variables
            .get(&variable_name(request.param("name")))
            .ok_or_else(ApiResponse::not_found)?;
        Ok(ApiResponse::ok_typed(&render(variable)))
    }
}

impl Handler for ops::ActionsUpdateRepoVariable {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let body: UpdateBody = request.json_as()?;
        let name = request.param("name").to_owned();
        let mut state = lock(state);
        let found = admin_repo(&state, request)?;
        update(&mut found.repo_mut(&mut state).setup.variables, &name, body)?;
        Ok(ApiResponse::no_content())
    }
}

impl Handler for ops::ActionsDeleteRepoVariable {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let mut state = lock(state);
        let found = admin_repo(&state, request)?;
        remove(
            &mut found.repo_mut(&mut state).setup.variables,
            request.param("name"),
        )?;
        Ok(ApiResponse::no_content())
    }
}

fn env_map<'a>(
    state: &'a State,
    request: &ApiRequest,
) -> Result<&'a BTreeMap<String, ActionsVariable>, ApiResponse> {
    let (_, env) = environment(state, request)?;
    Ok(&env.variables)
}

impl Handler for ops::ActionsListEnvironmentVariables {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        page(request, env_map(&state, request)?)
    }
}

impl Handler for ops::ActionsCreateEnvironmentVariable {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let body: CreateBody = request.json_as()?;
        let mut state = lock(state);
        let (found, _) = environment(&state, request)?;
        let env_name = request.param("environment_name").to_owned();
        let env = found
            .repo_mut(&mut state)
            .setup
            .environments
            .get_mut(&env_name)
            .expect("environment found above");
        create(&mut env.variables, body)?;
        created()
    }
}

impl Handler for ops::ActionsGetEnvironmentVariable {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let variable = env_map(&state, request)?
            .get(&variable_name(request.param("name")))
            .ok_or_else(ApiResponse::not_found)?;
        Ok(ApiResponse::ok_typed(&render(variable)))
    }
}

impl Handler for ops::ActionsUpdateEnvironmentVariable {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let body: UpdateBody = request.json_as()?;
        let name = request.param("name").to_owned();
        let mut state = lock(state);
        let (found, _) = environment(&state, request)?;
        let env_name = request.param("environment_name").to_owned();
        let env = found
            .repo_mut(&mut state)
            .setup
            .environments
            .get_mut(&env_name)
            .expect("environment found above");
        update(&mut env.variables, &name, body)?;
        Ok(ApiResponse::no_content())
    }
}

impl Handler for ops::ActionsDeleteEnvironmentVariable {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let mut state = lock(state);
        let (found, _) = environment(&state, request)?;
        let env_name = request.param("environment_name").to_owned();
        let env = found
            .repo_mut(&mut state)
            .setup
            .environments
            .get_mut(&env_name)
            .expect("environment found above");
        remove(&mut env.variables, request.param("name"))?;
        Ok(ApiResponse::no_content())
    }
}
