//! The `pull_request` event.

use serde_json::{Map, Value, json};

use super::{Ctx, json_of, label_value, rename_links, user_value};
use crate::types;

/// Queues `pull_request` `opened` for a pull request that was just created.
pub(crate) fn opened(ctx: &mut Ctx, pull: &types::PullRequest) {
    emit(ctx, "opened", pull, Map::new(), None);
}

/// Queues `pull_request` `edited` describing `changes` (`title`, `body`, and/or `base`).
pub(crate) fn edited(ctx: &mut Ctx, pull: &types::PullRequest, changes: Map<String, Value>) {
    if changes.is_empty() {
        return;
    }
    let mut extra = Map::new();
    extra.insert("changes".to_owned(), Value::Object(changes));
    emit(ctx, "edited", pull, extra, None);
}

/// Queues `pull_request` `closed`. A merged pull request keeps `action` `closed`.
pub(crate) fn closed(ctx: &mut Ctx, pull: &types::PullRequest) {
    emit(ctx, "closed", pull, Map::new(), None);
}

/// Queues `pull_request` `reopened`.
pub(crate) fn reopened(ctx: &mut Ctx, pull: &types::PullRequest) {
    emit(ctx, "reopened", pull, Map::new(), None);
}

/// Queues `pull_request` `synchronize` for a head that moved from `before` to `after`.
pub(crate) fn synchronize(
    ctx: &mut Ctx,
    pull: &types::PullRequest,
    before: &str,
    after: &str,
    sender: &types::SimpleUser,
) {
    let mut extra = Map::new();
    extra.insert("before".to_owned(), Value::from(before));
    extra.insert("after".to_owned(), Value::from(after));
    emit(ctx, "synchronize", pull, extra, Some(sender));
}

/// Queues one `pull_request` `labeled` event for `label`.
pub(crate) fn labeled(ctx: &mut Ctx, pull: &types::PullRequest, label: &types::Label) {
    let mut extra = Map::new();
    extra.insert("label".to_owned(), label_value(label));
    emit(ctx, "labeled", pull, extra, None);
}

/// Queues `pull_request` `review_requested` for one user.
pub(crate) fn review_requested_user(
    ctx: &mut Ctx,
    pull: &types::PullRequest,
    reviewer: &types::SimpleUser,
) {
    let mut extra = Map::new();
    extra.insert("requested_reviewer".to_owned(), user_value(reviewer));
    emit(ctx, "review_requested", pull, extra, None);
}

/// Queues `pull_request` `review_requested` for one team.
pub(crate) fn review_requested_team(
    ctx: &mut Ctx,
    pull: &types::PullRequest,
    team: &types::TeamSimple,
) {
    let mut extra = Map::new();
    extra.insert("requested_team".to_owned(), json_of(team));
    emit(ctx, "review_requested", pull, extra, None);
}

fn emit(
    ctx: &mut Ctx,
    action: &str,
    pull: &types::PullRequest,
    extra: Map<String, Value>,
    sender: Option<&types::SimpleUser>,
) {
    if !ctx.wants("pull_request") {
        return;
    }
    let Some(repository) = ctx.repository_value() else {
        return;
    };
    let Some(sender) = ctx.sender_value(sender) else {
        return;
    };
    let mut payload = json!({
        "action": action,
        "number": pull.number,
        "pull_request": prepare(pull, &repository),
        "repository": repository,
        "sender": sender,
    });
    if let Some(object) = payload.as_object_mut() {
        object.extend(extra);
    }
    ctx.enqueue("pull_request", &format!("pull_request-{action}"), payload);
}

/// Labels on `pull` as the webhook label model.
pub(crate) fn pull_labels(pull: &types::PullRequest) -> Vec<types::Label> {
    pull.labels
        .iter()
        .map(|label| types::Label {
            archived_at: None,
            archived_by: label.archived_by.clone(),
            color: label.color.clone(),
            default: label.default,
            description: label.description.clone(),
            id: label.id,
            name: label.name.clone(),
            node_id: label.node_id.clone(),
            url: label.url.clone(),
        })
        .collect()
}

/// REST pull request JSON with the webhook repository substituted for each embedded repo.
pub(crate) fn prepare(pull: &types::PullRequest, repository: &Value) -> Value {
    let mut value = json_of(pull);
    rename_links(&mut value);
    let Some(object) = value.as_object_mut() else {
        return value;
    };
    if object.get("draft").is_none_or(Value::is_null) {
        object.insert("draft".to_owned(), Value::Bool(false));
    }
    if let Some(head) = object.get_mut("head").and_then(Value::as_object_mut)
        && head.get("repo").is_some_and(|repo| !repo.is_null())
    {
        head.insert("repo".to_owned(), repository.clone());
    }
    if let Some(base) = object.get_mut("base").and_then(Value::as_object_mut) {
        base.insert("repo".to_owned(), repository.clone());
    }
    value
}
