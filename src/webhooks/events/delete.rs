//! The `delete` event, sent when a branch or tag ref is deleted.

use std::collections::BTreeMap;
use std::sync::Mutex;

use serde_json::json;

use super::{Ctx, create::ref_kind, push};
use crate::api::ApiRequest;
use crate::state::{State, lock};

/// Queues `delete` for a removed branch or tag named `name`.
pub(crate) fn reference(ctx: &mut Ctx, name: &str, ref_type: &str) {
    if name.is_empty() || !ctx.wants("delete") {
        return;
    }
    let Some(repository) = ctx.repository_value() else {
        return;
    };
    let Some(sender) = ctx.sender_value(None) else {
        return;
    };
    ctx.enqueue(
        "delete",
        "delete-event",
        json!({
            "ref": name,
            "ref_type": ref_type,
            "pusher_type": "user",
            "repository": repository,
            "sender": sender,
        }),
    );
}

/// Queues `delete` for each branch or tag a push removed.
///
/// Uses the same ref limits as [`push::enqueue_push`].
pub(crate) fn from_push(
    state: &Mutex<State>,
    request: &ApiRequest,
    owner: &str,
    name: &str,
    before: &BTreeMap<String, String>,
    after: &BTreeMap<String, String>,
) {
    let deleted: Vec<(String, &'static str)> = push::changed_refs(before, after)
        .into_iter()
        .filter(|(_, _, new)| *new == push::ZERO_SHA)
        .filter_map(|(reference, _, _)| {
            ref_kind(reference).map(|(short, kind)| (short.to_owned(), kind))
        })
        .collect();
    if deleted.is_empty() {
        return;
    }
    let mut state = lock(state);
    let mut ctx = Ctx::new(&mut state, request, owner, name);
    for (short, kind) in deleted {
        reference(&mut ctx, &short, kind);
    }
}
