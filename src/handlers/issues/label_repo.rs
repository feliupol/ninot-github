//! Repository label catalog: create, read, update and delete.
//!
//! A label that exists only as a copy on an issue or pull request is part of the catalog.
//! Updating it stores the label on the repository and rewrites every copy. Deleting it removes
//! the stored label and those copies.

use std::sync::Mutex;

use serde_json::{Map, Value};

use super::super::Handler;
use crate::api::{ApiRequest, ApiResponse, ApiResult};
use crate::handlers::repos::{Found, find_repo};
use crate::operation::ops;
use crate::render;
use crate::state::{State, lock};
use crate::types;

impl Handler for ops::IssuesCreateLabel {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        request.require_scope(&["repo", "public_repo"])?;
        let body = super::object_body(request)?;
        let mut state = lock(state);
        let found = find_repo(&state, request)?;
        super::require_push(&found)?;
        let name = required_name(body.get("name"))?;
        let color = match body.get("color") {
            None => "ededed".to_owned(),
            Some(Value::String(value)) => normalize_color(value)?,
            Some(_) => {
                return Err(ApiResponse::validation_failed("Label", "color", "invalid"));
            }
        };
        let description = optional_description(body.get("description"))?;
        if super::catalog(super::repo(&state, &found)).contains_key(&super::label_key(&name)) {
            return Err(ApiResponse::validation_failed(
                "Label",
                "name",
                "already_exists",
            ));
        }
        let label = new_label(&mut state, request, &found, &name, &color, description);
        super::repo_mut(&mut state, &found)
            .labels
            .insert(super::label_key(&label.name), label.clone());
        Ok(ApiResponse::typed(201, &label).with_header("location", label.url.clone()))
    }
}

impl Handler for ops::IssuesGetLabel {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let found = find_repo(&state, request)?;
        let key = super::label_key(&super::decode_param(request.param("name")));
        let label = super::catalog(super::repo(&state, &found))
            .get(&key)
            .cloned()
            .ok_or_else(ApiResponse::not_found)?;
        Ok(ApiResponse::typed(200, &label))
    }
}

impl Handler for ops::IssuesUpdateLabel {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        request.require_scope(&["repo", "public_repo"])?;
        let body = super::object_body(request)?;
        let mut state = lock(state);
        let found = find_repo(&state, request)?;
        let key = super::label_key(&super::decode_param(request.param("name")));
        if !super::catalog(super::repo(&state, &found)).contains_key(&key) {
            return Err(ApiResponse::not_found());
        }
        super::require_push(&found)?;
        let patch = label_patch(&state, request, &found, &body, &key)?;
        let label = store_patch(&mut state, request, &found, &key, patch)?;
        Ok(ApiResponse::typed(200, &label))
    }
}

impl Handler for ops::IssuesDeleteLabel {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        request.require_scope(&["repo", "public_repo"])?;
        let mut state = lock(state);
        let found = find_repo(&state, request)?;
        let key = super::label_key(&super::decode_param(request.param("name")));
        if !super::catalog(super::repo(&state, &found)).contains_key(&key) {
            return Err(ApiResponse::not_found());
        }
        super::require_push(&found)?;
        let repo = super::repo_mut(&mut state, &found);
        repo.labels.remove(&key);
        let now = super::now();
        for issue in repo.issues.values_mut() {
            let before = issue.data.labels.len();
            issue
                .data
                .labels
                .retain(|label| !super::labels::named(label, &key));
            if issue.data.labels.len() != before {
                issue.data.updated_at = now;
            }
        }
        for pull in repo
            .pull_requests
            .values_mut()
            .filter_map(|pull| pull.data.as_mut())
        {
            let before = pull.labels.len();
            pull.labels
                .retain(|label| super::label_key(&label.name) != key);
            if pull.labels.len() != before {
                pull.updated_at = now;
            }
        }
        Ok(ApiResponse::no_content())
    }
}

struct LabelPatch {
    name: String,
    color: String,
    description: Option<String>,
    description_set: bool,
    archived: Option<bool>,
    archiver: Option<types::SimpleUser>,
}

fn required_name(value: Option<&Value>) -> Result<String, ApiResponse> {
    match value {
        Some(Value::String(name)) => super::labels::trimmed_name(name),
        _ => Err(ApiResponse::validation_failed(
            "Label",
            "name",
            "missing_field",
        )),
    }
}

fn normalize_color(value: &str) -> Result<String, ApiResponse> {
    let color = value.trim();
    let valid = color.len() == 6 && color.chars().all(|character| character.is_ascii_hexdigit());
    if !valid {
        return Err(ApiResponse::validation_failed("Label", "color", "invalid"));
    }
    Ok(color.to_ascii_lowercase())
}

fn optional_description(value: Option<&Value>) -> Result<Option<String>, ApiResponse> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => bounded_description(text).map(Some),
        Some(_) => Err(ApiResponse::validation_failed(
            "Label",
            "description",
            "invalid",
        )),
    }
}

fn bounded_description(text: &str) -> Result<String, ApiResponse> {
    if text.chars().count() > 100 {
        return Err(ApiResponse::validation_failed(
            "Label",
            "description",
            "invalid",
        ));
    }
    Ok(text.to_owned())
}

fn new_label(
    state: &mut State,
    request: &ApiRequest,
    found: &Found,
    name: &str,
    color: &str,
    description: Option<String>,
) -> types::Label {
    let id = state.next_id();
    let api = format!("{}/repos/{}/{}", request.api_url, found.owner, found.name);
    types::Label {
        archived_at: None,
        archived_by: None,
        color: color.to_owned(),
        default: false,
        description,
        id: render::id(id),
        name: name.to_owned(),
        node_id: render::node_id("Label", id),
        url: super::label_api_url(&api, name),
    }
}

fn label_patch(
    state: &State,
    request: &ApiRequest,
    found: &Found,
    body: &Map<String, Value>,
    key: &str,
) -> Result<LabelPatch, ApiResponse> {
    let current = super::catalog(super::repo(state, found))
        .get(key)
        .cloned()
        .ok_or_else(ApiResponse::not_found)?;
    let name = match body.get("new_name") {
        None => current.name.clone(),
        Some(Value::String(name)) => super::labels::trimmed_name(name)?,
        Some(_) => {
            return Err(ApiResponse::validation_failed(
                "Label",
                "name",
                "missing_field",
            ));
        }
    };
    let new_key = super::label_key(&name);
    if new_key != key && super::catalog(super::repo(state, found)).contains_key(&new_key) {
        return Err(ApiResponse::validation_failed(
            "Label",
            "name",
            "already_exists",
        ));
    }
    let color = match body.get("color") {
        None => current.color.clone(),
        Some(Value::String(value)) => normalize_color(value)?,
        Some(_) => return Err(ApiResponse::validation_failed("Label", "color", "invalid")),
    };
    let (description, description_set) = match body.get("description") {
        None => (current.description.clone(), false),
        Some(value) => (optional_description(Some(value))?, true),
    };
    let archived = match body.get("archived") {
        None => None,
        Some(Value::Bool(value)) => Some(*value),
        Some(_) => {
            return Err(ApiResponse::validation_failed(
                "Label", "archived", "invalid",
            ));
        }
    };
    let archiver = if archived == Some(true) {
        Some(super::caller_user(
            state,
            request,
            &request.caller.as_ref().expect("scoped").login,
        )?)
    } else {
        None
    };
    Ok(LabelPatch {
        name,
        color,
        description,
        description_set,
        archived,
        archiver,
    })
}

fn store_patch(
    state: &mut State,
    request: &ApiRequest,
    found: &Found,
    key: &str,
    patch: LabelPatch,
) -> Result<types::Label, ApiResponse> {
    let stored = super::repo_mut(state, found).labels.remove(key);
    let mut label = match stored {
        Some(label) => label,
        None => super::catalog(super::repo(state, found))
            .remove(key)
            .ok_or_else(ApiResponse::not_found)?,
    };
    label.name.clone_from(&patch.name);
    label.color = patch.color;
    if patch.description_set {
        label.description = patch.description;
    }
    apply_archive(&mut label, patch.archived, patch.archiver);
    let api = format!("{}/repos/{}/{}", request.api_url, found.owner, found.name);
    label.url = super::label_api_url(&api, &label.name);
    let repo = super::repo_mut(state, found);
    rewrite_copies(repo, key, &label);
    repo.labels
        .insert(super::label_key(&label.name), label.clone());
    Ok(label)
}

fn apply_archive(
    label: &mut types::Label,
    archived: Option<bool>,
    archiver: Option<types::SimpleUser>,
) {
    match archived {
        Some(true) if label.archived_at.is_none() => {
            label.archived_at = Some(super::now());
            label.archived_by = archiver;
        }
        Some(false) => {
            label.archived_at = None;
            label.archived_by = None;
        }
        _ => {}
    }
}

fn rewrite_copies(repo: &mut crate::state::Repo, previous: &str, label: &types::Label) {
    let on_issue = render::label_on_issue(label);
    let on_pull = render::label_on_pull(label);
    let now = super::now();
    for issue in repo.issues.values_mut() {
        let mut changed = false;
        for slot in &mut issue.data.labels {
            if super::labels::named(slot, previous) {
                *slot = on_issue.clone();
                changed = true;
            }
        }
        if changed {
            issue.data.updated_at = now;
        }
    }
    for pull in repo
        .pull_requests
        .values_mut()
        .filter_map(|pull| pull.data.as_mut())
    {
        let mut changed = false;
        for slot in &mut pull.labels {
            if super::label_key(&slot.name) == previous {
                *slot = on_pull.clone();
                changed = true;
            }
        }
        if changed {
            pull.updated_at = now;
        }
    }
}
