//! The `issues` event.

use serde_json::{Value, json};

use super::{Ctx, empty_reactions, json_of, label_value, user_value};
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

/// Queues one `issues` `unlabeled` event for `label`.
pub(crate) fn unlabeled(ctx: &mut Ctx, issue: &types::Issue, label: &types::Label) {
    deliver(
        ctx,
        "unlabeled",
        issue,
        json!({ "label": label_value(label) }),
    );
}

/// Queues `issues` `edited` describing the previous title and/or body.
pub(crate) fn edited(
    ctx: &mut Ctx,
    issue: &types::Issue,
    title_from: Option<&str>,
    body_from: Option<&str>,
) {
    let mut changes = json!({});
    if let Some(from) = title_from {
        changes["title"] = json!({ "from": from });
    }
    if let Some(from) = body_from {
        changes["body"] = json!({ "from": from });
    }
    if changes.as_object().is_none_or(|object| object.is_empty()) {
        return;
    }
    deliver(ctx, "edited", issue, json!({ "changes": changes }));
}

/// Queues `issues` `closed`.
pub(crate) fn closed(ctx: &mut Ctx, issue: &types::Issue) {
    deliver(ctx, "closed", issue, json!({}));
}

/// Queues `issues` `reopened`.
pub(crate) fn reopened(ctx: &mut Ctx, issue: &types::Issue) {
    deliver(ctx, "reopened", issue, json!({}));
}

/// Queues `issues` `assigned` for `assignee`.
pub(crate) fn assigned(ctx: &mut Ctx, issue: &types::Issue, assignee: &types::SimpleUser) {
    deliver(
        ctx,
        "assigned",
        issue,
        json!({ "assignee": user_value(assignee) }),
    );
}

/// Queues `issues` `unassigned` for `assignee`.
pub(crate) fn unassigned(ctx: &mut Ctx, issue: &types::Issue, assignee: &types::SimpleUser) {
    deliver(
        ctx,
        "unassigned",
        issue,
        json!({ "assignee": user_value(assignee) }),
    );
}

/// Queues `issues` `locked`.
pub(crate) fn locked(ctx: &mut Ctx, issue: &types::Issue) {
    deliver(ctx, "locked", issue, json!({}));
}

/// Queues `issues` `unlocked`.
pub(crate) fn unlocked(ctx: &mut Ctx, issue: &types::Issue) {
    deliver(ctx, "unlocked", issue, json!({}));
}

/// Queues `issues` `milestoned` for `milestone`.
pub(crate) fn milestoned(
    ctx: &mut Ctx,
    issue: &types::Issue,
    milestone: &types::NullableMilestone,
) {
    deliver(
        ctx,
        "milestoned",
        issue,
        json!({ "milestone": json_of(milestone) }),
    );
}

/// Queues `issues` `demilestoned` for the removed `milestone`.
pub(crate) fn demilestoned(
    ctx: &mut Ctx,
    issue: &types::Issue,
    milestone: &types::NullableMilestone,
) {
    deliver(
        ctx,
        "demilestoned",
        issue,
        json!({ "milestone": json_of(milestone) }),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::ApiRequest;
    use crate::fixtures::IssueSeed;
    use crate::render;
    use crate::state::{Hook, State};
    use crate::validate::check_webhook;

    #[test]
    fn update_actions_match_the_schema() {
        let mut state = State::default();
        state.add_user("admin");
        state.create_repo("admin", "tool", false, None).unwrap();
        let now = crate::state::now();
        state
            .account_mut("admin")
            .unwrap()
            .repo_mut("tool")
            .unwrap()
            .hooks
            .push(Hook {
                id: 1,
                active: true,
                events: vec!["*".to_owned()],
                url: "http://127.0.0.1:1/hook".to_owned(),
                content_type: "json".to_owned(),
                secret: None,
                insecure_ssl: "0".to_owned(),
                created_at: now.clone(),
                updated_at: now,
            });
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
            user.clone(),
            1,
            IssueSeed::default(),
        )
        .unwrap();
        state.add_issue("admin", "tool", 1, issue.clone());
        let mut ctx = Ctx::new(&mut state, &request, "admin", "tool");
        edited(&mut ctx, &issue, Some("Earlier"), Some(""));
        let mut closed_issue = issue.clone();
        closed_issue.state = "closed".to_owned();
        closed_issue.closed_at = Some(crate::render::timestamp(&crate::time::now()));
        closed(&mut ctx, &closed_issue);
        reopened(&mut ctx, &issue);
        assigned(&mut ctx, &issue, &user);
        unassigned(&mut ctx, &issue, &user);
        let labels = issue_labels(&issue);
        if let Some(label) = labels.first() {
            unlabeled(&mut ctx, &issue, label);
        }
        let mut locked_issue = issue.clone();
        locked_issue.locked = true;
        locked(&mut ctx, &locked_issue);
        unlocked(&mut ctx, &issue);
        for delivery in &ctx.state.deliveries {
            check_webhook("issues", &delivery.payload()).unwrap_or_else(|error| {
                panic!("{} {}\n{error}", delivery.event, delivery.payload());
            });
        }
    }
}
