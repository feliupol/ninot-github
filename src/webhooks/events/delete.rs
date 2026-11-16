//! The `delete` event, sent when a branch or tag ref is deleted.

use std::collections::BTreeMap;
use std::sync::Mutex;

use super::{Ctx, PayloadError, create::ref_kind, push};
use crate::api::ApiRequest;
use crate::state::{State, lock};
use crate::webhook_types as wt;

/// Queues `delete` for a removed branch or tag named `name`.
pub(crate) fn reference(ctx: &mut Ctx, name: &str, ref_type: &str) -> Result<(), PayloadError> {
    if name.is_empty() || !ctx.wants("delete") {
        return Ok(());
    }
    let Some((repository, sender)) = ctx.parties("delete", None)? else {
        return Ok(());
    };
    let payload = wt::DeleteEvent {
        installation: None,
        organization: None,
        pusher_type: "user".to_owned(),
        ref_: name.to_owned(),
        ref_type: match ref_type {
            "tag" => wt::DeleteEventRefType::Tag,
            _ => wt::DeleteEventRefType::Branch,
        },
        repository,
        sender,
    };
    ctx.enqueue("delete", &payload);
    Ok(())
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
        // The push has landed, so a payload that cannot be built only fails its delivery,
        // which records the error.
        if let Err(error) = reference(&mut ctx, &short, kind) {
            eprintln!("ninot-github: no delete payload for {short}: {error}");
        }
    }
}
