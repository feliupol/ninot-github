//! Add and remove labels on an issue, recording the same events as the REST label routes.

use async_graphql::Error;
use serde::Deserialize;

use super::issue_actions::{actor, label_by_id, webhook_err};
use super::issues::issue_location as locate_issue;
use super::{Env, Map, MutationArgs, Output, not_found, read, typed_error};
use crate::handlers::issues::{record, rendered_issue};
use crate::handlers::repos::Found;
use crate::state::{Name, lock};
use crate::webhooks::events;

pub(super) const RESOLVERS: &[(&str, super::Resolver)] = &[
    ("Mutation.addLabelsToLabelable", add_labels),
    ("Mutation.removeLabelsFromLabelable", remove_labels),
];

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LabelsInput {
    labelable_id: String,
    label_ids: Vec<String>,
}

#[derive(serde::Serialize)]
struct LabelPayload {
    #[serde(rename = "__typename")]
    typename: &'static str,
}

fn add_labels(env: &Env, _: &Map, args: &Map) -> Result<Output, Error> {
    change_labels(env, args, true)
}

fn remove_labels(env: &Env, _: &Map, args: &Map) -> Result<Output, Error> {
    change_labels(env, args, false)
}

fn change_labels(env: &Env, args: &Map, add: bool) -> Result<Output, Error> {
    let MutationArgs { input } = read::<MutationArgs<LabelsInput>>(args)?;
    let mut state = lock(&env.state);
    let (owner, name, number) = locate_issue(&state, &input.labelable_id)
        .ok_or_else(|| not_found("Could not resolve labelable.".to_owned()))?;
    let permission = state
        .permission(Some(&env.caller), &owner, &name)
        .ok_or_else(|| typed_error("Must have push access to this repository", "FORBIDDEN"))?;
    if !permission.push {
        return Err(typed_error(
            "Must have push access to this repository",
            "FORBIDDEN",
        ));
    }
    let found = Found {
        owner: owner.clone(),
        name: name.clone(),
        permission,
    };
    let request = env.request();
    let before = rendered_issue(&state, &request, &found, number)
        .ok_or_else(|| not_found("issue does not exist".to_owned()))?;
    let before_snap = record::snapshot(&state, &request, &found, number)
        .map_err(|_| not_found("issue does not exist".to_owned()))?;
    let names: Vec<String> = input
        .label_ids
        .iter()
        .filter_map(|id| label_by_id(&state, &found, id).map(|label| label.name))
        .collect();
    {
        let issue = found
            .repo_mut(&mut state)
            .issues
            .get_mut(&number)
            .ok_or_else(|| not_found("issue does not exist".to_owned()))?;
        for label_name in &names {
            if add && !issue.has_label(label_name) {
                issue.labels.push(Name::new(label_name.clone()));
            } else if !add {
                issue.labels.retain(|label| !label.matches(label_name));
            }
        }
        issue.updated_at = crate::time::now_utc();
    }
    let actor = actor(&state, &request, &env.caller.login)?;
    let after_snap = record::snapshot(&state, &request, &found, number)
        .map_err(|_| not_found("issue does not exist".to_owned()))?;
    record::record_changes(
        &mut state,
        &found,
        number,
        &actor,
        &before_snap,
        &after_snap,
    );
    let after = rendered_issue(&state, &request, &found, number)
        .ok_or_else(|| not_found("issue does not exist".to_owned()))?;
    let mut ctx = events::Ctx::new(&mut state, &request, &found.owner, &found.name);
    events::issue_changes::from_update(&mut ctx, &before, &after).map_err(webhook_err)?;
    Output::of(LabelPayload {
        typename: if add {
            "AddLabelsToLabelablePayload"
        } else {
            "RemoveLabelsFromLabelablePayload"
        },
    })
}
