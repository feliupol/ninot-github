//! Payload builders, one module per webhook event.

pub mod ping;
pub mod push;

pub(crate) mod check_run;
pub(crate) mod check_suite;
pub(crate) mod create;
pub(crate) mod delete;
pub(crate) mod issue_changes;
pub(crate) mod issue_comment;
pub(crate) mod issues;
pub(crate) mod project;
pub(crate) mod pull_request;
pub(crate) mod pull_request_review;
pub(crate) mod pull_request_review_comment;
pub(crate) mod release;
pub(crate) mod status;
pub(crate) mod workflow_job;
pub(crate) mod workflow_run;

#[cfg(test)]
mod schema;

use std::path::PathBuf;

use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::api::ApiRequest;
use crate::render;
use crate::state::{Permission, State};
use crate::types;
use crate::webhook_types;
use crate::webhooks::enqueue;

/// Projects the REST repository into the webhook contract.
fn repository(repository: &types::FullRepository) -> webhook_types::Repository {
    repository.clone().into()
}

/// The repository, request and caller a delivery is built from.
pub(crate) struct Ctx<'a> {
    /// Mutable server state. The caller holds the lock.
    pub state: &'a mut State,
    /// The request that caused the state change, or a fixture request with no caller.
    pub request: &'a ApiRequest,
    /// Repository owner login.
    pub owner: &'a str,
    /// Repository name.
    pub name: &'a str,
}

impl<'a> Ctx<'a> {
    /// A delivery context for `owner/name`.
    pub(crate) fn new(
        state: &'a mut State,
        request: &'a ApiRequest,
        owner: &'a str,
        name: &'a str,
    ) -> Self {
        Self {
            state,
            request,
            owner,
            name,
        }
    }

    /// Whether any active hook of the repository wants `event`.
    pub(crate) fn wants(&self, event: &str) -> bool {
        self.state
            .account(self.owner)
            .and_then(|account| account.repo(self.name))
            .is_some_and(|repo| repo.hooks.iter().any(|hook| hook.wants(event)))
    }

    /// The webhook repository object, when the repository still exists.
    pub(crate) fn repository_value(&self) -> Option<Value> {
        let account = self.state.account(self.owner)?;
        let repo = account.repo(self.name)?;
        let full = render::full_repository(self.request, account, repo, Permission::default());
        Some(json_of(&webhook_types::Repository::from(full)))
    }

    /// [`repository_value`] projected onto the `repository-lite` definition.
    pub(crate) fn repository_lite(&self) -> Option<Value> {
        self.repository_value()
            .map(|value| project::project("repository-lite", value))
    }

    /// The webhook user for `explicit`, or the caller, or the repository owner.
    pub(crate) fn sender_value(&self, explicit: Option<&types::SimpleUser>) -> Option<Value> {
        if let Some(user) = explicit {
            return Some(user_value(user));
        }
        let login = self
            .request
            .caller
            .as_ref()
            .map_or(self.owner, |caller| caller.login.as_str());
        let account = self.state.account(login)?;
        Some(user_value(&render::simple_user(self.request, account)))
    }

    /// The default branch and description stored for the repository.
    pub(crate) fn branch_and_description(&self) -> Option<(String, Option<String>)> {
        let repo = self.state.account(self.owner)?.repo(self.name)?;
        Some((repo.default_branch.clone(), repo.description.clone()))
    }

    /// The repository's git directory, when it has one.
    pub(crate) fn git_dir(&self) -> Option<PathBuf> {
        self.state
            .account(self.owner)
            .and_then(|account| account.repo(self.name))
            .and_then(|repo| repo.git_dir.clone())
    }

    /// Projects `payload` onto `definition` and records a delivery when a hook wants `event`.
    pub(crate) fn enqueue(&mut self, event: &str, definition: &str, payload: Value) {
        if !self.wants(event) {
            return;
        }
        let payload = project::project(definition, payload);
        enqueue(self.state, self.owner, self.name, event, &payload, None);
    }
}

/// Serializes `value` as JSON.
pub(crate) fn json_of(value: &impl Serialize) -> Value {
    serde_json::to_value(value).expect("webhook values serialize to JSON")
}

/// The webhook user for `user`. A null `name` is omitted: the schema types it as a string and
/// does not require it.
pub(crate) fn user_value(user: &types::SimpleUser) -> Value {
    let mut value = json_of(&webhook_types::User::from(user.clone()));
    if value.get("name").is_some_and(Value::is_null) {
        value
            .as_object_mut()
            .expect("a user is an object")
            .remove("name");
    }
    value
}

/// Renames a REST `links` object to the webhook `_links` key when the model used the former.
///
/// A `self_` key inside that object becomes `self`.
pub(crate) fn rename_links(value: &mut Value) {
    let Some(object) = value.as_object_mut() else {
        return;
    };
    if let Some(links) = object.remove("links") {
        object.entry("_links".to_owned()).or_insert(links);
    }
    if let Some(links) = object.get_mut("_links").and_then(Value::as_object_mut)
        && let Some(self_link) = links.remove("self_")
    {
        links.entry("self".to_owned()).or_insert(self_link);
    }
}

/// Inserts `value` when `key` is absent or JSON null.
pub(crate) fn fill_null(object: &mut Map<String, Value>, key: &str, value: Value) {
    if object.get(key).is_none_or(Value::is_null) {
        object.insert(key.to_owned(), value);
    }
}

/// Label JSON with a real URL when the REST model left `url` or `node_id` empty.
pub(crate) fn label_value(label: &types::Label) -> Value {
    let mut value = json_of(label);
    if let Some(object) = value.as_object_mut() {
        if object
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or("")
            .is_empty()
        {
            object.insert(
                "url".to_owned(),
                Value::from(format!("https://github.com/labels/{}", label.name)),
            );
        }
        if object
            .get("node_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .is_empty()
        {
            object.insert("node_id".to_owned(), Value::from(format!("L_{}", label.id)));
        }
    }
    value
}

/// An empty reactions object. `url` is the reactions collection URL.
pub(crate) fn empty_reactions(url: &str) -> Value {
    json!({
        "url": url,
        "total_count": 0,
        "+1": 0,
        "-1": 0,
        "laugh": 0,
        "hooray": 0,
        "confused": 0,
        "heart": 0,
        "rocket": 0,
        "eyes": 0
    })
}

/// A stand-in GitHub App used when a check payload has no app of its own.
pub(crate) fn stub_app(owner: &Value, now: &str) -> Value {
    json!({
        "id": 1,
        "node_id": "A_ninot",
        "owner": owner,
        "name": "ninot-github",
        "description": null,
        "external_url": "https://github.com/apps/ninot-github",
        "html_url": "https://github.com/apps/ninot-github",
        "created_at": now,
        "updated_at": now
    })
}

/// A [`commit-simple`](https://github.com/octokit/webhooks) object for `sha`.
///
/// Uses the git commit when the repository has it, and a stub signature otherwise.
pub(crate) fn commit_simple(ctx: &Ctx, sha: &str) -> Value {
    if let Some(dir) = ctx.git_dir()
        && let Ok(commit) = ninot_common::git::commit(&dir, sha)
    {
        let person = |signature: &ninot_common::git::Signature| {
            json!({
                "name": signature.name,
                "email": signature.email,
                "date": stamp(&signature.date),
            })
        };
        return json!({
            "id": commit.sha,
            "tree_id": commit.tree,
            "message": commit.message,
            "timestamp": stamp(&commit.author.date),
            "author": person(&commit.author),
            "committer": person(&commit.committer),
        });
    }
    let now = crate::time::now();
    let person = json!({"name": "ninot-github", "email": null, "date": now});
    json!({
        "id": sha,
        "tree_id": sha,
        "message": "",
        "timestamp": now,
        "author": person,
        "committer": {"name": "ninot-github", "email": null, "date": now},
    })
}

/// `text` as an RFC 3339 timestamp, or the current time when `text` is not one.
pub(crate) fn stamp(text: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(text)
        .map_or_else(|_| crate::time::now(), |_| text.to_owned())
}
