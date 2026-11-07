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
use crate::webhooks::events;

impl Handler for ops::IssuesSetLabels {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = request.require_scope(&["repo", "public_repo"])?;
        let body = super::json_body(request)?;
        let mut state = lock(state);
        let found = find_repo(&state, request)?;
        let number = super::issue_number(request)?;
        let names = requested_names(&body)?;
        let before = super::record::snapshot(super::repo(&state, &found), number)?;
        let current = before
            .labels
            .iter()
            .map(|label| label.name.clone())
            .collect::<Vec<_>>();
        super::require_push(&found)?;
        let labels = require_labels(super::repo(&state, &found), &names, &current)?;
        let catalog = super::catalog(super::repo(&state, &found));
        let added: Vec<types::Label> = labels
            .iter()
            .filter(|label| {
                !current
                    .iter()
                    .any(|name| super::label_key(name) == super::label_key(&label.name))
            })
            .cloned()
            .collect();
        let removed: Vec<types::Label> = current
            .iter()
            .filter(|name| {
                !labels
                    .iter()
                    .any(|label| super::label_key(&label.name) == super::label_key(name))
            })
            .filter_map(|name| catalog.get(&super::label_key(name)).cloned())
            .collect();
        let actor = super::caller_user(&state, request, &caller.login)?;
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
        fire_label_diff(&mut state, request, &found, number, &added, &removed);
        let after = super::record::snapshot(super::repo(&state, &found), number)?;
        super::record::record_changes(&mut state, &found, number, &actor, &before, &after);
        Ok(ApiResponse::typed(200, &labels))
    }
}

impl Handler for ops::IssuesRemoveAllLabels {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = request.require_scope(&["repo", "public_repo"])?;
        let mut state = lock(state);
        let found = find_repo(&state, request)?;
        let number = super::issue_number(request)?;
        let before = super::record::snapshot(super::repo(&state, &found), number)?;
        let current = before
            .labels
            .iter()
            .map(|label| label.name.clone())
            .collect::<Vec<_>>();
        super::require_push(&found)?;
        let actor = super::caller_user(&state, request, &caller.login)?;
        let catalog = super::catalog(super::repo(&state, &found));
        let removed: Vec<types::Label> = current
            .iter()
            .filter_map(|name| catalog.get(&super::label_key(name)).cloned())
            .collect();
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
        fire_label_diff(&mut state, request, &found, number, &[], &removed);
        let after = super::record::snapshot(super::repo(&state, &found), number)?;
        super::record::record_changes(&mut state, &found, number, &actor, &before, &after);
        Ok(ApiResponse::no_content())
    }
}

impl Handler for ops::IssuesRemoveLabel {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = request.require_scope(&["repo", "public_repo"])?;
        let mut state = lock(state);
        let found = find_repo(&state, request)?;
        let number = super::issue_number(request)?;
        let key = super::label_key(&super::decode_param(request.param("name")));
        let before = super::record::snapshot(super::repo(&state, &found), number)?;
        if !before
            .labels
            .iter()
            .any(|label| super::label_key(&label.name) == key)
        {
            return Err(ApiResponse::not_found());
        }
        super::require_push(&found)?;
        let removed = super::catalog(super::repo(&state, &found))
            .get(&key)
            .cloned()
            .into_iter()
            .collect::<Vec<_>>();
        let actor = super::caller_user(&state, request, &caller.login)?;
        let remaining = remove_named(&mut state, &found, number, &key)?;
        fire_label_diff(&mut state, request, &found, number, &[], &removed);
        let after = super::record::snapshot(super::repo(&state, &found), number)?;
        super::record::record_changes(&mut state, &found, number, &actor, &before, &after);
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

fn fire_label_diff(
    state: &mut State,
    request: &ApiRequest,
    found: &Found,
    number: u64,
    added: &[types::Label],
    removed: &[types::Label],
) {
    let issue = super::stored_issue(super::repo(state, found), number);
    let pull = super::stored_pull(super::repo(state, found), number);
    let mut ctx = super::webhook_ctx(state, request, found);
    if let Some(issue) = &issue {
        for label in added {
            events::issues::labeled(&mut ctx, issue, label);
        }
        for label in removed {
            events::issues::unlabeled(&mut ctx, issue, label);
        }
    }
    if let Some(pull) = &pull {
        for label in added {
            events::pull_request::labeled(&mut ctx, pull, label);
        }
    }
}

fn labels_named(repo: &Repo, names: &[String]) -> Vec<types::Label> {
    let catalog = super::catalog(repo);
    names
        .iter()
        .filter_map(|name| catalog.get(&super::label_key(name)).cloned())
        .collect()
}
