//! The `create` event, sent when a branch or tag ref is created.

use std::collections::BTreeMap;
use std::sync::Mutex;

use serde_json::json;

use super::{Ctx, push};
use crate::api::ApiRequest;
use crate::state::{State, lock};

/// The short name and `branch` or `tag` kind of `reference`, when it is a head or a tag.
pub(crate) fn ref_kind(reference: &str) -> Option<(&str, &'static str)> {
    let (name, kind) = if let Some(name) = reference.strip_prefix("refs/heads/") {
        (name, "branch")
    } else {
        let name = reference.strip_prefix("refs/tags/")?;
        (name, "tag")
    };
    (!name.is_empty()).then_some((name, kind))
}

/// Queues `create` for a new branch or tag named `name`.
pub(crate) fn reference(ctx: &mut Ctx, name: &str, ref_type: &str) {
    if name.is_empty() || !ctx.wants("create") {
        return;
    }
    let Some(repository) = ctx.repository_value() else {
        return;
    };
    let Some(sender) = ctx.sender_value(None) else {
        return;
    };
    let Some((master_branch, description)) = ctx.branch_and_description() else {
        return;
    };
    ctx.enqueue(
        "create",
        "create-event",
        json!({
            "ref": name,
            "ref_type": ref_type,
            "master_branch": master_branch,
            "description": description,
            "pusher_type": "user",
            "repository": repository,
            "sender": sender,
        }),
    );
}

/// Queues `create` for each branch or tag a push added.
///
/// Uses the same ref limits as [`push::enqueue_push`]: no events when too many branches moved,
/// and no tag events when too many tags moved.
pub(crate) fn from_push(
    state: &Mutex<State>,
    request: &ApiRequest,
    owner: &str,
    name: &str,
    before: &BTreeMap<String, String>,
    after: &BTreeMap<String, String>,
) {
    let created: Vec<(String, &'static str)> = push::changed_refs(before, after)
        .into_iter()
        .filter(|(_, old, _)| *old == push::ZERO_SHA)
        .filter_map(|(reference, _, _)| {
            ref_kind(reference).map(|(short, kind)| (short.to_owned(), kind))
        })
        .collect();
    if created.is_empty() {
        return;
    }
    let mut state = lock(state);
    let mut ctx = Ctx::new(&mut state, request, owner, name);
    for (short, kind) in created {
        reference(&mut ctx, &short, kind);
    }
}
