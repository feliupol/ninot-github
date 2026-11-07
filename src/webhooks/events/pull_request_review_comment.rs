//! The `pull_request_review_comment` event.

use serde_json::{Value, json};

use super::{Ctx, empty_reactions, json_of, rename_links};
use crate::types;

/// Queues `pull_request_review_comment` `created`.
pub(crate) fn created(
    ctx: &mut Ctx,
    pull: &types::PullRequest,
    comment: &types::PullRequestReviewComment,
) {
    if !ctx.wants("pull_request_review_comment") {
        return;
    }
    let Some(repository) = ctx.repository_value() else {
        return;
    };
    let Some(sender) = ctx.sender_value(None) else {
        return;
    };
    ctx.enqueue(
        "pull_request_review_comment",
        "pull_request_review_comment-created",
        json!({
            "action": "created",
            "comment": prepare(comment, &sender),
            "pull_request": super::pull_request::prepare(pull, &repository),
            "repository": repository,
            "sender": sender,
        }),
    );
}

fn prepare(comment: &types::PullRequestReviewComment, sender: &Value) -> Value {
    let mut value = json_of(comment);
    rename_links(&mut value);
    let Some(object) = value.as_object_mut() else {
        return value;
    };
    if object.get("original_position").is_none_or(Value::is_null) {
        object.insert("original_position".to_owned(), Value::from(1));
    }
    if object.get("original_line").is_none_or(Value::is_null) {
        object.insert("original_line".to_owned(), Value::from(1));
    }
    if object.get("side").is_none_or(Value::is_null) {
        object.insert("side".to_owned(), Value::from("RIGHT"));
    }
    if object.get("user").is_none_or(Value::is_null) {
        object.insert("user".to_owned(), sender.clone());
    }
    if object.get("body").is_none_or(Value::is_null) {
        object.insert("body".to_owned(), Value::from(""));
    }
    if object
        .get("pull_request_review_id")
        .is_none_or(Value::is_null)
    {
        object.insert("pull_request_review_id".to_owned(), Value::from(0));
    }
    for key in ["in_reply_to_id", "subject_type"] {
        if object.get(key).is_some_and(Value::is_null) {
            object.remove(key);
        }
    }
    if object.get("reactions").is_none_or(Value::is_null) {
        let url = object.get("url").and_then(Value::as_str).map_or_else(
            || "https://api.github.com/pulls/comments/reactions".to_owned(),
            |url| format!("{url}/reactions"),
        );
        object.insert("reactions".to_owned(), empty_reactions(&url));
    }
    value
}
