//! The `status` event.

use serde_json::{Value, json};

use super::{Ctx, stamp};
use crate::types;

/// Queues `status` for a commit status on `sha`.
pub(crate) fn commit_status(ctx: &mut Ctx, sha: &str, status: &types::Status) {
    if !ctx.wants("status") {
        return;
    }
    let Some(repository) = ctx.repository_value() else {
        return;
    };
    let Some(sender) = ctx.sender_value(None) else {
        return;
    };
    let api = repository
        .get("url")
        .and_then(Value::as_str)
        .unwrap_or("https://api.github.com/repos/unknown/unknown");
    let html = repository
        .get("html_url")
        .and_then(Value::as_str)
        .unwrap_or("https://github.com/unknown/unknown");
    ctx.enqueue(
        "status",
        "status-event",
        json!({
            "id": status.id,
            "sha": sha,
            "name": status.context,
            "target_url": status.target_url,
            "context": status.context,
            "description": status.description,
            "state": status.state.to_ascii_lowercase(),
            "commit": commit_object(ctx, sha, api, html),
            "branches": branches(ctx, sha, api),
            "created_at": status.created_at,
            "updated_at": status.updated_at,
            "repository": repository,
            "sender": sender,
        }),
    );
}

fn commit_object(ctx: &Ctx, sha: &str, api: &str, html: &str) -> Value {
    let commit = ctx
        .git_dir()
        .and_then(|dir| ninot_common::git::commit(&dir, sha).ok());
    let (tree, message, author, committer, parents) = match commit {
        Some(commit) => (
            commit.tree,
            commit.message,
            person(&commit.author),
            person(&commit.committer),
            commit.parents,
        ),
        None => (
            sha.to_owned(),
            String::new(),
            person_stub(),
            person_stub(),
            Vec::new(),
        ),
    };
    json!({
        "sha": sha,
        "node_id": format!("C_{sha}"),
        "commit": {
            "author": author,
            "committer": committer,
            "message": message,
            "tree": {"sha": tree, "url": format!("{api}/git/trees/{tree}")},
            "url": format!("{api}/git/commits/{sha}"),
            "comment_count": 0,
            "verification": {
                "verified": false,
                "reason": "unsigned",
                "signature": Value::Null,
                "payload": Value::Null,
            }
        },
        "url": format!("{api}/commits/{sha}"),
        "html_url": format!("{html}/commit/{sha}"),
        "comments_url": format!("{api}/commits/{sha}/comments"),
        "author": Value::Null,
        "committer": Value::Null,
        "parents": parents.iter().map(|parent| json!({
            "sha": parent,
            "url": format!("{api}/commits/{parent}"),
            "html_url": format!("{html}/commit/{parent}"),
        })).collect::<Vec<_>>(),
    })
}

fn person(signature: &ninot_common::git::Signature) -> Value {
    json!({
        "name": signature.name,
        "email": signature.email,
        "date": stamp(&signature.date),
    })
}

fn person_stub() -> Value {
    json!({
        "name": "ninot-github",
        "email": Value::Null,
        "date": crate::time::now(),
    })
}

fn branches(ctx: &Ctx, sha: &str, api: &str) -> Value {
    let Some(dir) = ctx.git_dir() else {
        return Value::Array(Vec::new());
    };
    let Ok(refs) = ninot_common::git::refs(&dir, "refs/heads/") else {
        return Value::Array(Vec::new());
    };
    Value::Array(
        refs.into_iter()
            .filter(|reference| reference.sha == sha)
            .take(10)
            .map(|reference| {
                let name = reference
                    .name
                    .strip_prefix("refs/heads/")
                    .unwrap_or(&reference.name)
                    .to_owned();
                json!({
                    "name": name,
                    "commit": {"sha": reference.sha, "url": format!("{api}/commits/{}", reference.sha)},
                    "protected": false,
                })
            })
            .collect(),
    )
}
