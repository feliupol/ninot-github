//! The `issue_comment` event.

use serde_json::{Value, json};

use super::{Ctx, empty_reactions, json_of, rename_links};
use crate::types;

/// Queues `issue_comment` `created` for a comment on issue or pull request `number`.
pub(crate) fn created(ctx: &mut Ctx, number: u64, comment: &types::IssueComment) {
    if !ctx.wants("issue_comment") {
        return;
    }
    let Some(repository) = ctx.repository_value() else {
        return;
    };
    let Some(sender) = ctx.sender_value(None) else {
        return;
    };
    let Some(issue) = issue_value(ctx, number, &repository) else {
        return;
    };
    ctx.enqueue(
        "issue_comment",
        "issue_comment-created",
        json!({
            "action": "created",
            "issue": issue,
            "comment": prepare_comment(comment, &sender),
            "repository": repository,
            "sender": sender,
        }),
    );
}

fn issue_value(ctx: &Ctx, number: u64, repository: &Value) -> Option<Value> {
    let repo = ctx.state.account(ctx.owner)?.repo(ctx.name)?;
    if let Some(issue) = repo.issues.get(&number) {
        return Some(super::issues::issue_json(&issue.data));
    }
    let pull = repo.pull_requests.get(&number)?.data.as_ref()?;
    let api = repository
        .get("url")
        .and_then(Value::as_str)
        .unwrap_or("https://api.github.com/repos/unknown/unknown");
    Some(issue_from_pull(pull, api))
}

fn issue_from_pull(pull: &types::PullRequest, api: &str) -> Value {
    let number = pull.number;
    json!({
        "url": pull.issue_url,
        "repository_url": api,
        "labels_url": format!("{api}/issues/{number}/labels{{/name}}"),
        "comments_url": pull.comments_url,
        "events_url": format!("{api}/issues/{number}/events"),
        "html_url": pull.html_url,
        "id": pull.id,
        "node_id": pull.node_id,
        "number": number,
        "title": pull.title,
        "user": json_of(&pull.user),
        "labels": json_of(&pull.labels),
        "state": pull.state.to_string().to_ascii_lowercase(),
        "locked": pull.locked,
        "assignee": pull.assignee.as_ref().map(json_of).unwrap_or(Value::Null),
        "assignees": json_of(&pull.assignees),
        "milestone": Value::Null,
        "comments": pull.comments,
        "created_at": pull.created_at,
        "updated_at": pull.updated_at,
        "closed_at": pull.closed_at,
        "author_association": json_of(&pull.author_association),
        "active_lock_reason": pull.active_lock_reason,
        "body": pull.body,
        "reactions": empty_reactions(&format!("{}/reactions", pull.issue_url)),
        "pull_request": {
            "url": pull.url,
            "html_url": pull.html_url,
            "diff_url": pull.diff_url,
            "patch_url": pull.patch_url,
            "merged_at": pull.merged_at,
        }
    })
}

fn prepare_comment(comment: &types::IssueComment, sender: &Value) -> Value {
    let mut value = json_of(comment);
    rename_links(&mut value);
    let Some(object) = value.as_object_mut() else {
        return value;
    };
    if object.get("author_association").is_none_or(Value::is_null) {
        object.insert("author_association".to_owned(), Value::from("NONE"));
    }
    if object.get("body").is_none_or(Value::is_null) {
        object.insert("body".to_owned(), Value::from(""));
    }
    if object.get("user").is_none_or(Value::is_null) {
        object.insert("user".to_owned(), sender.clone());
    }
    if object.get("performed_via_github_app").is_none() {
        object.insert("performed_via_github_app".to_owned(), Value::Null);
    }
    if object.get("reactions").is_none_or(Value::is_null) {
        let url = object.get("url").and_then(Value::as_str).map_or_else(
            || "https://api.github.com/issues/comments/reactions".to_owned(),
            |url| format!("{url}/reactions"),
        );
        object.insert("reactions".to_owned(), empty_reactions(&url));
    }
    value
}
