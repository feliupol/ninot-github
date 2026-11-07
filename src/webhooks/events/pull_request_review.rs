//! The `pull_request_review` event.

use serde_json::json;

use super::{Ctx, json_of, rename_links};
use crate::types;

/// Queues `pull_request_review` `submitted` for a review that is not pending.
///
/// GitHub does not deliver a review while it is still `PENDING`.
pub(crate) fn submitted(
    ctx: &mut Ctx,
    pull: &types::PullRequest,
    review: &types::PullRequestReview,
) {
    if review.state.eq_ignore_ascii_case("pending") {
        return;
    }
    if !ctx.wants("pull_request_review") {
        return;
    }
    let Some(repository) = ctx.repository_value() else {
        return;
    };
    let Some(sender) = ctx.sender_value(None) else {
        return;
    };
    let payload = json!({
        "action": "submitted",
        "review": prepare(review, &pull.head.sha, &sender),
        "pull_request": super::pull_request::prepare(pull, &repository),
        "repository": repository,
        "sender": sender,
    });
    ctx.enqueue(
        "pull_request_review",
        "pull_request_review-submitted",
        payload,
    );
}

fn prepare(
    review: &types::PullRequestReview,
    head: &str,
    sender: &serde_json::Value,
) -> serde_json::Value {
    let mut value = json_of(review);
    rename_links(&mut value);
    let Some(object) = value.as_object_mut() else {
        return value;
    };
    if object
        .get("commit_id")
        .is_none_or(|commit| commit.as_str().unwrap_or("").is_empty())
    {
        object.insert("commit_id".to_owned(), serde_json::Value::from(head));
    }
    if object.get("user").is_none_or(serde_json::Value::is_null) {
        object.insert("user".to_owned(), sender.clone());
    }
    if object.get("body").is_some_and(serde_json::Value::is_null) {
        object.insert("body".to_owned(), serde_json::Value::from(""));
    }
    if let Some(links) = object
        .get_mut("_links")
        .and_then(serde_json::Value::as_object_mut)
        && let Some(self_link) = links.remove("self_")
    {
        links.entry("self".to_owned()).or_insert(self_link);
    }
    value
}
