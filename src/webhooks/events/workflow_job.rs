//! The `workflow_job` event.

use serde_json::{Value, json};

use super::{Ctx, fill_null, json_of};
use crate::types;

/// Queues `workflow_job` for `job`.
///
/// The action is the job status: `completed`, `in_progress`, `waiting`, or `queued`.
pub(crate) fn deliver(ctx: &mut Ctx, run: &types::WorkflowRun, job: &types::Job) {
    if !ctx.wants("workflow_job") {
        return;
    }
    let Some(repository) = ctx.repository_value() else {
        return;
    };
    let Some(sender) = ctx.sender_value(None) else {
        return;
    };
    let prepared = prepare(run, job);
    let action = prepared
        .get("status")
        .and_then(Value::as_str)
        .map(job_action)
        .unwrap_or("queued");
    ctx.enqueue(
        "workflow_job",
        &format!("workflow_job-{action}"),
        json!({
            "action": action,
            "workflow_job": prepared,
            "repository": repository,
            "sender": sender,
        }),
    );
}

fn job_action(status: &str) -> &'static str {
    match status {
        "completed" => "completed",
        "in_progress" => "in_progress",
        "waiting" => "waiting",
        _ => "queued",
    }
}

fn prepare(run: &types::WorkflowRun, job: &types::Job) -> Value {
    let mut value = json_of(job);
    let now = crate::time::now();
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
        "queued" | "in_progress" | "completed" | "waiting"
    ) {
        status
    } else {
        "queued".to_owned()
    };
    object.insert("status".to_owned(), Value::from(status.clone()));
    let conclusion = object
        .get("conclusion")
        .and_then(Value::as_str)
        .map(|text| text.to_ascii_lowercase());
    let job_conclusions = ["success", "failure", "cancelled", "skipped"];
    let conclusion = match conclusion {
        Some(text) if job_conclusions.contains(&text.as_str()) => Value::from(text),
        _ if status == "completed" => Value::from("success"),
        _ => Value::Null,
    };
    object.insert("conclusion".to_owned(), conclusion);
    if object.get("html_url").is_none_or(Value::is_null) {
        let url = object
            .get("url")
            .cloned()
            .unwrap_or_else(|| Value::from("https://github.com/actions/jobs"));
        object.insert("html_url".to_owned(), url);
    }
    let fallback_name = run.name.clone().unwrap_or_else(|| "CI".to_owned());
    fill_null(object, "workflow_name", Value::from(fallback_name));
    fill_null(object, "head_branch", Value::from("main"));
    fill_null(object, "run_attempt", Value::from(1));
    fill_null(object, "started_at", Value::from(now.clone()));
    if object.get("steps").is_none_or(|steps| !steps.is_array()) {
        object.insert("steps".to_owned(), Value::Array(Vec::new()));
    }
    if let Some(steps) = object.get_mut("steps") {
        prepare_steps(steps, &now);
    }
    value
}

fn prepare_steps(steps: &mut Value, now: &str) {
    let Some(items) = steps.as_array_mut() else {
        return;
    };
    for step in items {
        let Some(object) = step.as_object_mut() else {
            continue;
        };
        let status = object
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("queued")
            .to_ascii_lowercase();
        let status = match status.as_str() {
            "completed" => "completed",
            "in_progress" => "in_progress",
            _ => "queued",
        };
        object.insert("status".to_owned(), Value::from(status));
        fill_null(object, "name", Value::from("step"));
        fill_null(object, "number", Value::from(1));
        match status {
            "completed" => {
                let conclusion = object
                    .get("conclusion")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_ascii_lowercase();
                let conclusion = match conclusion.as_str() {
                    "failure" | "skipped" | "success" | "cancelled" => conclusion,
                    _ => "success".to_owned(),
                };
                object.insert("conclusion".to_owned(), Value::from(conclusion));
                fill_null(object, "started_at", Value::from(now));
                fill_null(object, "completed_at", Value::from(now));
            }
            "in_progress" => {
                object.insert("conclusion".to_owned(), Value::Null);
                object.insert("completed_at".to_owned(), Value::Null);
                fill_null(object, "started_at", Value::from(now));
            }
            _ => {
                object.insert("conclusion".to_owned(), Value::Null);
                object.insert("started_at".to_owned(), Value::Null);
                object.insert("completed_at".to_owned(), Value::Null);
            }
        }
    }
}
