//! The `workflow_run` event.

use serde_json::{Value, json};

use super::{Ctx, commit_simple, fill_null, json_of};
use crate::types;

/// Queues `workflow_run` for `run`.
///
/// The action follows the run's status: `completed` and `in_progress` keep that name, and every
/// other status, including `queued`, is delivered as `requested`.
pub(crate) fn deliver(ctx: &mut Ctx, run: &types::WorkflowRun) {
    if !ctx.wants("workflow_run") {
        return;
    }
    let Some(repository) = ctx.repository_value() else {
        return;
    };
    let Some(lite) = ctx.repository_lite() else {
        return;
    };
    let Some(sender) = ctx.sender_value(None) else {
        return;
    };
    let prepared = prepare(ctx, run, &lite, &sender);
    let action = prepared
        .get("status")
        .and_then(Value::as_str)
        .map(run_action)
        .unwrap_or("requested");
    let workflow = workflow(&prepared, &repository);
    ctx.enqueue(
        "workflow_run",
        &format!("workflow_run-{action}"),
        json!({
            "action": action,
            "workflow": workflow,
            "workflow_run": prepared,
            "repository": repository,
            "sender": sender,
        }),
    );
}

fn run_action(status: &str) -> &'static str {
    match status {
        "completed" => "completed",
        "in_progress" => "in_progress",
        _ => "requested",
    }
}

fn prepare(ctx: &Ctx, run: &types::WorkflowRun, lite: &Value, sender: &Value) -> Value {
    let mut value = json_of(run);
    let Some(object) = value.as_object_mut() else {
        return value;
    };
    let status = object
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("queued")
        .to_ascii_lowercase();
    let status = if matches!(
        status.as_str(),
        "requested" | "in_progress" | "completed" | "queued" | "waiting"
    ) {
        status
    } else {
        "queued".to_owned()
    };
    let action = run_action(&status);
    object.insert("status".to_owned(), Value::from(status.clone()));
    let conclusion = object
        .get("conclusion")
        .and_then(Value::as_str)
        .map(|text| text.to_ascii_lowercase());
    let allowed = [
        "success",
        "failure",
        "neutral",
        "cancelled",
        "timed_out",
        "action_required",
        "stale",
        "skipped",
    ];
    let conclusion = match conclusion {
        Some(text) if allowed.contains(&text.as_str()) => Value::from(text),
        _ if action == "completed" => Value::from("neutral"),
        _ => Value::Null,
    };
    object.insert("conclusion".to_owned(), conclusion);
    fill_null(object, "name", Value::from("CI"));
    fill_null(object, "head_branch", Value::from("main"));
    fill_null(object, "event", Value::from("push"));
    fill_null(object, "path", Value::from(".github/workflows/ci.yml"));
    let name = object.get("name").cloned().unwrap_or(Value::from("CI"));
    fill_null(object, "display_title", name);
    fill_null(object, "run_attempt", Value::from(1));
    let created = object
        .get("created_at")
        .cloned()
        .unwrap_or_else(|| Value::from(crate::time::now()));
    fill_null(object, "run_started_at", created);
    if object.get("check_suite_id").is_none_or(Value::is_null) {
        let id = object.get("id").cloned().unwrap_or(Value::from(1));
        object.insert("check_suite_id".to_owned(), id);
    }
    if object.get("check_suite_node_id").is_none_or(Value::is_null) {
        let id = object.get("id").and_then(Value::as_i64).unwrap_or(1);
        object.insert(
            "check_suite_node_id".to_owned(),
            Value::from(format!("CS_{id}")),
        );
    }
    let sha = object
        .get("head_sha")
        .and_then(Value::as_str)
        .unwrap_or("0000000000000000000000000000000000000000")
        .to_owned();
    object.insert("head_commit".to_owned(), commit_simple(ctx, &sha));
    object.insert("actor".to_owned(), sender.clone());
    object.insert("triggering_actor".to_owned(), sender.clone());
    object.insert("repository".to_owned(), lite.clone());
    object.insert("head_repository".to_owned(), lite.clone());
    if object
        .get("pull_requests")
        .is_none_or(|pulls| !pulls.is_array())
    {
        object.insert("pull_requests".to_owned(), Value::Array(Vec::new()));
    }
    value
}

fn workflow(run: &Value, repository: &Value) -> Value {
    let id = run
        .get("workflow_id")
        .cloned()
        .filter(|id| !id.is_null())
        .or_else(|| run.get("id").cloned())
        .unwrap_or(Value::from(1));
    let name = run
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("CI")
        .to_owned();
    let path = run
        .get("path")
        .and_then(Value::as_str)
        .unwrap_or(".github/workflows/ci.yml");
    let html = repository
        .get("html_url")
        .and_then(Value::as_str)
        .unwrap_or("https://github.com/unknown/unknown");
    let api = repository
        .get("url")
        .and_then(Value::as_str)
        .unwrap_or("https://api.github.com/repos/unknown/unknown");
    let created = run
        .get("created_at")
        .and_then(Value::as_str)
        .unwrap_or("1970-01-01T00:00:00Z");
    let numeric = id.as_i64().unwrap_or(1);
    json!({
        "badge_url": format!("{html}/actions/workflows/{numeric}/badge.svg"),
        "created_at": created,
        "html_url": format!("{html}/blob/HEAD/{path}"),
        "id": id,
        "name": name,
        "node_id": format!("W_{numeric}"),
        "path": path,
        "state": "active",
        "updated_at": run.get("updated_at").and_then(Value::as_str).unwrap_or(created),
        "url": format!("{api}/actions/workflows/{numeric}"),
    })
}
