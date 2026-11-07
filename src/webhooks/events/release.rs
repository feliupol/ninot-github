//! The `release` event.

use serde_json::{Map, Value, json};

use super::{Ctx, json_of};
use crate::types;

/// Queues `release` `action` for `release`.
///
/// `changes` is the edited `name` and `body` (`{"name": {"from": ...}}`). Other actions omit it.
pub(crate) fn deliver(
    ctx: &mut Ctx,
    action: &str,
    release: &types::Release,
    changes: Option<Map<String, Value>>,
) {
    if !ctx.wants("release") {
        return;
    }
    let Some(repository) = ctx.repository_value() else {
        return;
    };
    let Some(sender) = ctx.sender_value(None) else {
        return;
    };
    let mut payload = json!({
        "action": action,
        "release": prepare(release),
        "repository": repository,
        "sender": sender,
    });
    if let Some(changes) = changes.filter(|changes| !changes.is_empty())
        && let Some(object) = payload.as_object_mut()
    {
        object.insert("changes".to_owned(), Value::Object(changes));
    }
    ctx.enqueue("release", &format!("release-{action}"), payload);
}

fn prepare(release: &types::Release) -> Value {
    let mut value = json_of(release);
    let Some(object) = value.as_object_mut() else {
        return value;
    };
    if object.get("name").is_none_or(Value::is_null) {
        object.insert("name".to_owned(), Value::from(""));
    }
    if object.get("body").is_none_or(Value::is_null) {
        object.insert("body".to_owned(), Value::from(""));
    }
    if object.get("assets").is_none_or(Value::is_null) {
        object.insert("assets".to_owned(), Value::Array(Vec::new()));
    }
    value
}
