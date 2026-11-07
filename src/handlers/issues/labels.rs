//! Repository and issue label operations.
//!
//! Names are matched without regard to case. Archived labels stay on issues that already have
//! them and cannot be added to another issue.

use std::sync::Mutex;

use serde_json::{Map, Value};

use super::super::Handler;
use crate::api::{ApiRequest, ApiResponse, ApiResult};
use crate::handlers::repos::{Found, find_repo};
use crate::operation::ops;
use crate::render;
use crate::state::{Repo, State, lock};
use crate::types;

impl Handler for ops::IssuesSetLabels {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        request.require_scope(&["repo", "public_repo"])?;
        let body = super::json_body(request)?;
        let mut state = lock(state);
        let found = find_repo(&state, request)?;
        let number = super::issue_number(request)?;
        let names = requested_names(&body)?;
        let current = current_names(super::repo(&state, &found), number)?;
        super::require_push(&found)?;
        let labels = require_labels(super::repo(&state, &found), &names, &current)?;
        let repo = super::repo_mut(&mut state, &found);
        super::with_subject(
            repo,
            number,
            |issue| {
                issue.labels = labels.iter().map(render::label_on_issue).collect();
                issue.updated_at = super::now();
                Ok(())
            },
            |pull| {
                pull.labels = labels.iter().map(render::label_on_pull).collect();
                pull.updated_at = super::now();
                Ok(())
            },
        )?;
        Ok(ApiResponse::typed(200, &labels))
    }
}

impl Handler for ops::IssuesRemoveAllLabels {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        request.require_scope(&["repo", "public_repo"])?;
        let mut state = lock(state);
        let found = find_repo(&state, request)?;
        let number = super::issue_number(request)?;
        current_names(super::repo(&state, &found), number)?;
        super::require_push(&found)?;
        let repo = super::repo_mut(&mut state, &found);
        super::with_subject(
            repo,
            number,
            |issue| {
                issue.labels.clear();
                issue.updated_at = super::now();
                Ok(())
            },
            |pull| {
                pull.labels.clear();
                pull.updated_at = super::now();
                Ok(())
            },
        )?;
        Ok(ApiResponse::no_content())
    }
}

impl Handler for ops::IssuesRemoveLabel {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        request.require_scope(&["repo", "public_repo"])?;
        let mut state = lock(state);
        let found = find_repo(&state, request)?;
        let number = super::issue_number(request)?;
        let key = super::label_key(&super::decode_param(request.param("name")));
        let current = current_names(super::repo(&state, &found), number)?;
        if !current.iter().any(|name| super::label_key(name) == key) {
            return Err(ApiResponse::not_found());
        }
        super::require_push(&found)?;
        let remaining = remove_named(&mut state, &found, number, &key)?;
        let repo = super::repo(&state, &found);
        Ok(ApiResponse::typed(200, &labels_named(repo, &remaining)))
    }
}

/// Labels named in `names`, rejecting unknown labels and archived labels that are not already present.
pub(super) fn require_labels(
    repo: &Repo,
    names: &[String],
    already: &[String],
) -> Result<Vec<types::Label>, ApiResponse> {
    let catalog = super::catalog(repo);
    let mut chosen = Vec::new();
    for name in names {
        let key = super::label_key(name);
        let Some(label) = catalog.get(&key) else {
            return Err(ApiResponse::validation_failed("Label", "name", "invalid"));
        };
        let present = already
            .iter()
            .any(|current| super::label_key(current) == key);
        if label.archived_at.is_some() && !present {
            return Err(ApiResponse::validation_failed("Label", "name", "invalid"));
        }
        if chosen
            .iter()
            .any(|existing: &types::Label| super::label_key(&existing.name) == key)
        {
            continue;
        }
        chosen.push(label.clone());
    }
    Ok(chosen)
}

fn requested_names(body: &Value) -> Result<Vec<String>, ApiResponse> {
    match body {
        Value::String(name) => Ok(vec![trimmed_name(name)?]),
        Value::Array(items) => items.iter().map(one_name).collect(),
        Value::Object(object) => names_field(object),
        _ => Err(ApiResponse::error(422, "Invalid request body")),
    }
}

fn names_field(object: &Map<String, Value>) -> Result<Vec<String>, ApiResponse> {
    let Some(labels) = object.get("labels") else {
        return Err(ApiResponse::validation_failed(
            "Label",
            "labels",
            "missing_field",
        ));
    };
    match labels {
        Value::Array(items) => items.iter().map(one_name).collect(),
        Value::String(name) => Ok(vec![trimmed_name(name)?]),
        _ => Err(ApiResponse::validation_failed("Label", "labels", "invalid")),
    }
}

fn one_name(value: &Value) -> Result<String, ApiResponse> {
    match value {
        Value::String(name) => trimmed_name(name),
        Value::Object(object) => {
            trimmed_name(object.get("name").and_then(Value::as_str).unwrap_or(""))
        }
        _ => Err(ApiResponse::validation_failed("Label", "name", "invalid")),
    }
}

/// The label name with surrounding whitespace removed.
///
/// An empty name is a missing-field validation error.
pub(super) fn trimmed_name(name: &str) -> Result<String, ApiResponse> {
    let name = name.trim();
    if name.is_empty() {
        return Err(ApiResponse::validation_failed(
            "Label",
            "name",
            "missing_field",
        ));
    }
    Ok(name.to_owned())
}

fn current_names(repo: &Repo, number: u64) -> Result<Vec<String>, ApiResponse> {
    if let Some(pull) = repo.pull_requests.get(&number) {
        let data = pull
            .data
            .as_ref()
            .ok_or_else(|| ApiResponse::error(500, "Pull request REST model is not seeded"))?;
        return Ok(data.labels.iter().map(|label| label.name.clone()).collect());
    }
    repo.issues
        .get(&number)
        .map(|issue| super::label_names(&issue.data))
        .ok_or_else(ApiResponse::not_found)
}

/// Whether `label` is stored under the lowercase key `key`.
pub(super) fn named(label: &types::IssueLabelsItem, key: &str) -> bool {
    super::label_from_issue_item(label).is_some_and(|label| super::label_key(&label.name) == key)
}

fn remove_named(
    state: &mut State,
    found: &Found,
    number: u64,
    key: &str,
) -> Result<Vec<String>, ApiResponse> {
    super::with_subject(
        super::repo_mut(state, found),
        number,
        |issue| {
            issue.labels.retain(|label| !named(label, key));
            issue.updated_at = super::now();
            Ok(super::label_names(issue))
        },
        |pull| {
            pull.labels
                .retain(|label| super::label_key(&label.name) != key);
            pull.updated_at = super::now();
            Ok(pull.labels.iter().map(|label| label.name.clone()).collect())
        },
    )
}

fn labels_named(repo: &Repo, names: &[String]) -> Vec<types::Label> {
    let catalog = super::catalog(repo);
    names
        .iter()
        .filter_map(|name| catalog.get(&super::label_key(name)).cloned())
        .collect()
}
