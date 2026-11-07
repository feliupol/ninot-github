//! The `issues` event.
//!
//! `opened` and `labeled` are delivered from the issue handlers. `edited`, `closed`, `reopened`
//! and `assigned` have payload builders only: the fake has no operation that performs those
//! transitions.

use serde_json::{Value, json};

use super::{Ctx, empty_reactions, json_of, label_value};
use crate::types;

/// Queues `issues` `opened`.
pub(crate) fn opened(ctx: &mut Ctx, issue: &types::Issue) {
    deliver(ctx, "opened", issue, json!({}));
}

/// Queues one `issues` `labeled` event for `label`. Pull requests use `pull_request` instead.
pub(crate) fn labeled(ctx: &mut Ctx, issue: &types::Issue, label: &types::Label) {
    deliver(
        ctx,
        "labeled",
        issue,
        json!({ "label": label_value(label) }),
    );
}

/// Labels on `issue` as the webhook label model.
pub(crate) fn issue_labels(issue: &types::Issue) -> Vec<types::Label> {
    issue
        .labels
        .iter()
        .map(|label| match label {
            types::IssueLabelsItem::String(name) => types::Label {
                archived_at: None,
                archived_by: None,
                color: "ededed".to_owned(),
                default: false,
                description: Some(String::new()),
                id: 0,
                name: name.clone(),
                node_id: String::new(),
                url: String::new(),
            },
            types::IssueLabelsItem::Object {
                archived_by,
                color,
                default,
                description,
                id,
                name,
                node_id,
                url,
            } => types::Label {
                archived_at: None,
                archived_by: archived_by.clone(),
                color: color.clone().unwrap_or_else(|| "ededed".to_owned()),
                default: default.unwrap_or(false),
                description: description.clone(),
                id: id.unwrap_or_default(),
                name: name.clone().unwrap_or_default(),
                node_id: node_id.clone().unwrap_or_default(),
                url: url.clone().unwrap_or_default(),
            },
        })
        .collect()
}

/// An `issues` `edited` payload. Nothing in the fake edits an issue, so this is not delivered.
#[allow(
    dead_code,
    reason = "no issue edit operation exists yet; unit tests build the payload"
)]
pub(crate) fn edited_payload(
    ctx: &Ctx,
    issue: &types::Issue,
    title_from: &str,
    body_from: &str,
) -> Option<Value> {
    project(
        ctx,
        "edited",
        issue,
        json!({ "changes": { "title": { "from": title_from }, "body": { "from": body_from } } }),
    )
}

/// An `issues` `closed` payload. Nothing in the fake closes an issue, so this is not delivered.
#[allow(
    dead_code,
    reason = "no issue close operation exists yet; unit tests build the payload"
)]
pub(crate) fn closed_payload(ctx: &Ctx, issue: &types::Issue) -> Option<Value> {
    let mut issue = issue.clone();
    issue.state = "closed".to_owned();
    if issue.closed_at.is_none() {
        issue.closed_at = Some(crate::render::timestamp(&crate::time::now()));
    }
    project(ctx, "closed", &issue, json!({}))
}

/// An `issues` `reopened` payload. Nothing in the fake reopens an issue, so this is not delivered.
#[allow(
    dead_code,
    reason = "no issue reopen operation exists yet; unit tests build the payload"
)]
pub(crate) fn reopened_payload(ctx: &Ctx, issue: &types::Issue) -> Option<Value> {
    let mut issue = issue.clone();
    issue.state = "open".to_owned();
    issue.closed_at = None;
    project(ctx, "reopened", &issue, json!({}))
}

/// An `issues` `assigned` payload. Nothing in the fake assigns an issue, so this is not delivered.
#[allow(
    dead_code,
    reason = "no issue assign operation exists yet; unit tests build the payload"
)]
pub(crate) fn assigned_payload(ctx: &Ctx, issue: &types::Issue) -> Option<Value> {
    let mut extra = json!({});
    if let Some(assignee) = issue_user(issue) {
        extra["assignee"] = assignee;
    }
    project(ctx, "assigned", issue, extra)
}

fn deliver(ctx: &mut Ctx, action: &str, issue: &types::Issue, extra: Value) {
    if !ctx.wants("issues") {
        return;
    }
    let Some(payload) = project(ctx, action, issue, extra) else {
        return;
    };
    // `project` already applied the schema; enqueue projects again, which is stable.
    ctx.enqueue("issues", &format!("issues-{action}"), payload);
}

fn project(ctx: &Ctx, action: &str, issue: &types::Issue, extra: Value) -> Option<Value> {
    let repository = ctx.repository_value()?;
    let sender = ctx.sender_value(None)?;
    let mut payload = json!({
        "action": action,
        "issue": issue_json(issue),
        "repository": repository,
        "sender": sender,
    });
    if let (Some(object), Some(extra)) = (payload.as_object_mut(), extra.as_object()) {
        object.extend(extra.clone());
    }
    Some(super::project::project(
        &format!("issues-{action}"),
        payload,
    ))
}

/// Issue JSON with the associations and reactions a webhook requires.
pub(crate) fn issue_json(issue: &types::Issue) -> Value {
    let mut value = json_of(issue);
    let Some(object) = value.as_object_mut() else {
        return value;
    };
    if object.get("author_association").is_none_or(Value::is_null) {
        object.insert("author_association".to_owned(), Value::from("NONE"));
    }
    if object.get("state").is_none_or(Value::is_null) {
        object.insert("state".to_owned(), Value::from("open"));
    }
    if object.get("locked").is_none_or(Value::is_null) {
        object.insert("locked".to_owned(), Value::Bool(false));
    }
    if object.get("reactions").is_none_or(Value::is_null) {
        let url = object.get("url").and_then(Value::as_str).map_or_else(
            || "https://github.com/issues/reactions".to_owned(),
            |url| format!("{url}/reactions"),
        );
        object.insert("reactions".to_owned(), empty_reactions(&url));
    }
    value
}

fn issue_user(issue: &types::Issue) -> Option<Value> {
    issue.user.as_ref().map(json_of)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::ApiRequest;
    use crate::fixtures::IssueSeed;
    use crate::render;
    use crate::state::State;
    use crate::validate::check_webhook;

    #[test]
    fn edited_closed_reopened_and_assigned_payloads_match_the_schema() {
        let mut state = State::default();
        state.add_user("admin");
        state.create_repo("admin", "tool", false, None).unwrap();
        let request = ApiRequest::for_urls("http://127.0.0.1:1", None);
        let user = {
            let account = state.account("admin").unwrap();
            render::simple_user(&request, account)
        };
        let issue = crate::fixtures::seeded_issue(
            &mut state,
            &request,
            "admin",
            "tool",
            user,
            1,
            IssueSeed::default(),
        )
        .unwrap();
        state.add_issue("admin", "tool", 1, issue.clone());
        let ctx = Ctx::new(&mut state, &request, "admin", "tool");
        for payload in [
            edited_payload(&ctx, &issue, "Earlier", "").unwrap(),
            closed_payload(&ctx, &issue).unwrap(),
            reopened_payload(&ctx, &issue).unwrap(),
            assigned_payload(&ctx, &issue).unwrap(),
        ] {
            check_webhook("issues", &payload).unwrap_or_else(|error| {
                panic!("{error}\n{payload}");
            });
        }
    }
}
