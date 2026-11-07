//! The `check_suite` event.

use serde_json::{Value, json};

use super::{Ctx, check_run, commit_simple, json_of};
use crate::types;

/// Queues `check_suite` `requested` for the first check run on a commit.
pub(crate) fn requested(ctx: &mut Ctx, check: &types::CheckRun) {
    deliver_from_run(ctx, "requested", check);
}

/// Queues `check_suite` `requested` for a suite created through the Checks API.
pub(crate) fn requested_suite(ctx: &mut Ctx, suite: &types::CheckSuite) {
    deliver_from_suite(ctx, "requested", suite);
}

/// Queues `check_suite` `rerequested`.
pub(crate) fn rerequested(ctx: &mut Ctx, suite: &types::CheckSuite) {
    deliver_from_suite(ctx, "rerequested", suite);
}

fn deliver_from_run(ctx: &mut Ctx, action: &str, check: &types::CheckRun) {
    if !ctx.wants("check_suite") {
        return;
    }
    let Some(repository) = ctx.repository_value() else {
        return;
    };
    let Some(sender) = ctx.sender_value(None) else {
        return;
    };
    let now = crate::time::now();
    let mut suite = check_run::suite_payload(check, &repository, &sender, &now);
    let id = suite.get("id").and_then(Value::as_i64).unwrap_or(1);
    let sha = suite
        .get("head_sha")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let repo_url = repository
        .get("url")
        .and_then(Value::as_str)
        .unwrap_or("https://api.github.com/repos/unknown/unknown");
    if let Some(object) = suite.as_object_mut() {
        object.insert("node_id".to_owned(), Value::from(format!("CS_{id}")));
        object.insert("latest_check_runs_count".to_owned(), Value::from(1));
        object.insert(
            "check_runs_url".to_owned(),
            Value::from(format!("{repo_url}/check-suites/{id}/check-runs")),
        );
        object.insert("head_commit".to_owned(), commit_simple(ctx, &sha));
    }
    enqueue(ctx, action, suite, repository, sender);
}

fn deliver_from_suite(ctx: &mut Ctx, action: &str, suite: &types::CheckSuite) {
    if !ctx.wants("check_suite") {
        return;
    }
    let Some(repository) = ctx.repository_value() else {
        return;
    };
    let Some(sender) = ctx.sender_value(None) else {
        return;
    };
    let mut value = json_of(suite);
    let sha = value
        .get("head_sha")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    if let Some(object) = value.as_object_mut() {
        object.insert("head_commit".to_owned(), commit_simple(ctx, &sha));
        object.remove("repository");
    }
    enqueue(ctx, action, value, repository, sender);
}

fn enqueue(ctx: &mut Ctx, action: &str, suite: Value, repository: Value, sender: Value) {
    ctx.enqueue(
        "check_suite",
        &format!("check_suite-{action}"),
        json!({
            "action": action,
            "check_suite": suite,
            "repository": repository,
            "sender": sender,
        }),
    );
}
