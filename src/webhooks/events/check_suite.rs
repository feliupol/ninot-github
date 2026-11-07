//! The `check_suite` event.

use serde_json::json;

use super::{Ctx, check_run, commit_simple};
use crate::types;

/// Queues `check_suite` `requested` for the first check run on a commit.
pub(crate) fn requested(ctx: &mut Ctx, check: &types::CheckRun) {
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
    let id = suite
        .get("id")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(1);
    let sha = suite
        .get("head_sha")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_owned();
    let repo_url = repository
        .get("url")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("https://api.github.com/repos/unknown/unknown");
    if let Some(object) = suite.as_object_mut() {
        object.insert(
            "node_id".to_owned(),
            serde_json::Value::from(format!("CS_{id}")),
        );
        object.insert(
            "latest_check_runs_count".to_owned(),
            serde_json::Value::from(1),
        );
        object.insert(
            "check_runs_url".to_owned(),
            serde_json::Value::from(format!("{repo_url}/check-suites/{id}/check-runs")),
        );
        object.insert("head_commit".to_owned(), commit_simple(ctx, &sha));
    }
    ctx.enqueue(
        "check_suite",
        "check_suite-requested",
        json!({
            "action": "requested",
            "check_suite": suite,
            "repository": repository,
            "sender": sender,
        }),
    );
}
