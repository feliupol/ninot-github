//! The `check_run` event.

use serde_json::{Value, json};

use super::{Ctx, json_of, stub_app};
use crate::types;

/// Queues `check_run` `created`.
pub(crate) fn created(ctx: &mut Ctx, check: &types::CheckRun) {
    if !ctx.wants("check_run") {
        return;
    }
    let Some(repository) = ctx.repository_value() else {
        return;
    };
    let Some(sender) = ctx.sender_value(None) else {
        return;
    };
    let now = crate::time::now();
    ctx.enqueue(
        "check_run",
        "check_run-created",
        json!({
            "action": "created",
            "check_run": prepare(check, &repository, &sender, &now),
            "repository": repository,
            "sender": sender,
        }),
    );
}

/// The check suite object nested in a check run, and the base of a `check_suite` event.
pub(crate) fn suite_payload(
    check: &types::CheckRun,
    repository: &Value,
    sender: &Value,
    now: &str,
) -> Value {
    let value = json_of(check);
    let id = value
        .get("check_suite")
        .and_then(|suite| suite.get("id"))
        .and_then(Value::as_i64)
        .or_else(|| value.get("id").and_then(Value::as_i64))
        .unwrap_or(1);
    let status = suite_status(
        value
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("queued"),
    );
    let conclusion = suite_conclusion(value.get("conclusion"));
    let head_sha = value
        .get("head_sha")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let repo_url = repository
        .get("url")
        .and_then(Value::as_str)
        .unwrap_or("https://api.github.com/repos/unknown/unknown");
    let app = app_value(&value, sender, now);
    json!({
        "id": id,
        "head_branch": Value::Null,
        "head_sha": head_sha,
        "status": status,
        "conclusion": conclusion,
        "url": format!("{repo_url}/check-suites/{id}"),
        "before": Value::Null,
        "after": Value::Null,
        "pull_requests": value.get("pull_requests").cloned().filter(Value::is_array).unwrap_or_else(|| Value::Array(Vec::new())),
        "app": app,
        "created_at": now,
        "updated_at": now,
    })
}

fn prepare(check: &types::CheckRun, repository: &Value, sender: &Value, now: &str) -> Value {
    let mut value = json_of(check);
    let suite = suite_payload(check, repository, sender, now);
    let app = app_value(&value, sender, now);
    let Some(object) = value.as_object_mut() else {
        return value;
    };
    super::fill_null(object, "external_id", Value::from(""));
    if object.get("html_url").is_none_or(Value::is_null) {
        let url = object
            .get("url")
            .cloned()
            .unwrap_or_else(|| Value::from("https://github.com/checks"));
        object.insert("html_url".to_owned(), url);
    }
    super::fill_null(object, "started_at", Value::from(now));
    object.insert("app".to_owned(), app);
    object.insert("check_suite".to_owned(), suite);
    if object
        .get("pull_requests")
        .is_none_or(|pulls| !pulls.is_array())
    {
        object.insert("pull_requests".to_owned(), Value::Array(Vec::new()));
    }
    value
}

fn app_value(check: &Value, sender: &Value, now: &str) -> Value {
    check
        .get("app")
        .cloned()
        .filter(|app| {
            app.get("html_url")
                .and_then(Value::as_str)
                .is_some_and(|url| !url.is_empty())
        })
        .unwrap_or_else(|| stub_app(sender, now))
}

fn suite_status(status: &str) -> &'static str {
    match status.to_ascii_lowercase().as_str() {
        "in_progress" => "in_progress",
        "completed" => "completed",
        _ => "queued",
    }
}

fn suite_conclusion(value: Option<&Value>) -> Value {
    let Some(text) = value.and_then(Value::as_str) else {
        return Value::Null;
    };
    match text.to_ascii_lowercase().as_str() {
        "success" | "failure" | "neutral" | "cancelled" | "timed_out" | "action_required"
        | "stale" => Value::from(text.to_ascii_lowercase()),
        _ => Value::Null,
    }
}
