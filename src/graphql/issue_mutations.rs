//! Create, update, close and reopen mutations for issues.

use async_graphql::Error;
use serde::Deserialize;

use super::issue_actions::{actor, label_by_id, repo_location, webhook_err};
use super::issues::issue_location as locate_issue;
use super::types::IssueStateReason;
use super::{Env, Map, MutationArgs, Output, database_id, read};
use crate::handlers::issues::{next_subject_number, record, rendered_issue, resolve_issue_label};
use crate::handlers::repos::Found;
use crate::render::node_id;
use crate::state::{Issue as Stored, IssueState, Name, State, StateReason, lock};
use crate::webhooks::events;

pub(super) const RESOLVERS: &[(&str, super::Resolver)] = &[
    ("Mutation.createIssue", create_issue),
    ("Mutation.updateIssue", update_issue),
    ("Mutation.closeIssue", close_issue),
    ("Mutation.reopenIssue", reopen_issue),
];

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateIssueInput {
    repository_id: String,
    title: String,
    body: Option<String>,
    label_ids: Option<Vec<String>>,
    assignee_ids: Option<Vec<String>>,
    milestone_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateIssueInput {
    id: String,
    title: Option<String>,
    body: Option<String>,
    assignee_ids: Option<Vec<String>>,
    milestone_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CloseIssueInput {
    issue_id: String,
    state_reason: Option<IssueStateReason>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct IssueIdInput {
    issue_id: String,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct IssuePayload {
    #[serde(rename = "__typename")]
    typename: &'static str,
    issue: IssueRef,
}

#[derive(serde::Serialize)]
struct IssueRef {
    id: String,
    url: Option<String>,
}

fn create_issue(env: &Env, _: &Map, args: &Map) -> Result<Output, Error> {
    let MutationArgs { input } = read::<MutationArgs<CreateIssueInput>>(args)?;
    if !env.caller.has_scope("repo") && !env.caller.has_scope("public_repo") {
        return Err(super::errors::insufficient_scopes());
    }
    let mut state = lock(&env.state);
    let (owner, name) = repo_location(&state, &input.repository_id)
        .ok_or_else(super::errors::resolve_repository)?;
    let permission = state
        .permission(Some(&env.caller), &owner, &name)
        .ok_or_else(super::errors::integration_forbidden)?;
    if !permission.push {
        return Err(super::errors::pat_forbidden());
    }
    let found = Found {
        owner: owner.clone(),
        name: name.clone(),
        permission,
    };
    let request = env.request();
    let number = next_subject_number(found.repo(&state));
    let now = crate::time::now_utc();
    let mut labels = Vec::new();
    for id in input.label_ids.unwrap_or_default() {
        let Some(label) = label_by_id(&state, &found, &id) else {
            continue;
        };
        let name = label.name.clone();
        let rendered = resolve_issue_label(&mut state, &request, &found, &name, || label);
        if !labels.iter().any(|existing: &crate::types::Label| {
            existing.name.eq_ignore_ascii_case(&rendered.name)
        }) {
            labels.push(rendered);
        }
    }
    let assignees = input
        .assignee_ids
        .unwrap_or_default()
        .iter()
        .filter_map(|id| account_login(&state, id).map(Name::new))
        .collect();
    let milestone = input
        .milestone_id
        .as_deref()
        .and_then(|id| milestone_number(&state, &owner, &name, id));
    state.add_issue(
        &owner,
        &name,
        Stored {
            number,
            node_id: format!("I_{owner}_{name}_{number}"),
            title: input.title,
            body: input.body,
            author: Name::new(env.caller.login.clone()),
            state: IssueState::Open,
            state_reason: None,
            closed_at: None,
            closed_by: None,
            labels: labels
                .iter()
                .map(|label| Name::new(label.name.clone()))
                .collect(),
            assignees,
            milestone,
            locked: false,
            lock_reason: None,
            created_at: now,
            updated_at: now,
            comments: Vec::new(),
            events: Vec::new(),
            is_pinned: false,
            linked_branches: Vec::new(),
        },
    );
    let actor = actor(&state, &request, &env.caller.login)?;
    let after_snap = record::snapshot(&state, &request, &found, number)
        .map_err(|_| super::errors::issue_not_found())?;
    record::record_changes(
        &mut state,
        &found,
        number,
        &actor,
        &record::Snapshot {
            labels: Vec::new(),
            assignees: Vec::new(),
            milestone: None,
            state: "open".to_owned(),
            locked: false,
        },
        &after_snap,
    );
    let issue = rendered_issue(&state, &request, &found, number).expect("created");
    let mut ctx = events::Ctx::new(&mut state, &request, &found.owner, &found.name);
    events::issues::opened(&mut ctx, &issue).map_err(webhook_err)?;
    for label in &labels {
        events::issues::labeled(&mut ctx, &issue, label).map_err(webhook_err)?;
    }
    for user in &issue.assignees {
        events::issues::assigned(&mut ctx, &issue, user).map_err(webhook_err)?;
    }
    if let Some(milestone) = &issue.milestone {
        events::issues::milestoned(&mut ctx, &issue, milestone).map_err(webhook_err)?;
    }
    Output::of(IssuePayload {
        typename: "CreateIssuePayload",
        issue: IssueRef {
            id: issue.node_id,
            url: Some(issue.html_url),
        },
    })
}

fn update_issue(env: &Env, _: &Map, args: &Map) -> Result<Output, Error> {
    let MutationArgs { input } = read::<MutationArgs<UpdateIssueInput>>(args)?;
    let mut state = lock(&env.state);
    let (owner, name, number) =
        locate_issue(&state, &input.id).ok_or_else(|| super::errors::resolve_node(input.id))?;
    let permission = state
        .permission(Some(&env.caller), &owner, &name)
        .ok_or_else(super::errors::integration_forbidden)?;
    let found = Found {
        owner: owner.clone(),
        name: name.clone(),
        permission,
    };
    let request = env.request();
    let before = rendered_issue(&state, &request, &found, number)
        .ok_or_else(super::errors::issue_not_found)?;
    let before_snap = record::snapshot(&state, &request, &found, number)
        .map_err(|_| super::errors::issue_not_found())?;
    let assignees = input.assignee_ids.as_ref().map(|ids| {
        ids.iter()
            .filter_map(|id| account_login(&state, id).map(Name::new))
            .collect::<Vec<_>>()
    });
    let milestone = input
        .milestone_id
        .as_deref()
        .and_then(|id| milestone_number(&state, &owner, &name, id));
    {
        let issue = found
            .repo_mut(&mut state)
            .issues
            .get_mut(&number)
            .ok_or_else(super::errors::issue_not_found)?;
        if let Some(title) = input.title {
            issue.title = title;
        }
        if let Some(body) = input.body {
            issue.body = Some(body);
        }
        if let Some(logins) = assignees {
            issue.assignees = logins;
        }
        if input.milestone_id.is_some() {
            issue.milestone = milestone;
        }
        issue.updated_at = crate::time::now_utc();
    }
    let actor = actor(&state, &request, &env.caller.login)?;
    let after_snap = record::snapshot(&state, &request, &found, number)
        .map_err(|_| super::errors::issue_not_found())?;
    record::record_changes(
        &mut state,
        &found,
        number,
        &actor,
        &before_snap,
        &after_snap,
    );
    let after = rendered_issue(&state, &request, &found, number).expect("issue exists");
    let id = after.node_id.clone();
    let mut ctx = events::Ctx::new(&mut state, &request, &found.owner, &found.name);
    events::issue_changes::from_update(&mut ctx, &before, &after).map_err(webhook_err)?;
    Output::of(IssuePayload {
        typename: "UpdateIssuePayload",
        issue: IssueRef { id, url: None },
    })
}

fn close_issue(env: &Env, _: &Map, args: &Map) -> Result<Output, Error> {
    let MutationArgs { input } = read::<MutationArgs<CloseIssueInput>>(args)?;
    set_issue_state(
        env,
        &input.issue_id,
        IssueState::Closed,
        input.state_reason,
        "CloseIssuePayload",
    )
}

fn reopen_issue(env: &Env, _: &Map, args: &Map) -> Result<Output, Error> {
    let MutationArgs { input } = read::<MutationArgs<IssueIdInput>>(args)?;
    set_issue_state(
        env,
        &input.issue_id,
        IssueState::Open,
        Some(IssueStateReason::Reopened),
        "ReopenIssuePayload",
    )
}

fn set_issue_state(
    env: &Env,
    issue_id: &str,
    next: IssueState,
    reason: Option<IssueStateReason>,
    typename: &'static str,
) -> Result<Output, Error> {
    let mut state = lock(&env.state);
    let (owner, name, number) =
        locate_issue(&state, issue_id).ok_or_else(|| super::errors::resolve_node(issue_id))?;
    let permission = state
        .permission(Some(&env.caller), &owner, &name)
        .ok_or_else(super::errors::integration_forbidden)?;
    let found = Found {
        owner: owner.clone(),
        name: name.clone(),
        permission,
    };
    let request = env.request();
    let before = rendered_issue(&state, &request, &found, number)
        .ok_or_else(super::errors::issue_not_found)?;
    let before_snap = record::snapshot(&state, &request, &found, number)
        .map_err(|_| super::errors::issue_not_found())?;
    {
        let issue = found
            .repo_mut(&mut state)
            .issues
            .get_mut(&number)
            .ok_or_else(super::errors::issue_not_found)?;
        let now = crate::time::now_utc();
        issue.state = next;
        issue.state_reason = Some(match (next, reason) {
            (IssueState::Closed, Some(IssueStateReason::NotPlanned)) => StateReason::NotPlanned,
            (IssueState::Closed, Some(IssueStateReason::Duplicate)) => StateReason::Duplicate,
            (IssueState::Closed, _) => StateReason::Completed,
            (IssueState::Open, _) => StateReason::Reopened,
        });
        if next == IssueState::Closed {
            issue.closed_at = Some(now);
            issue.closed_by = Some(Name::new(env.caller.login.clone()));
        } else {
            issue.closed_at = None;
            issue.closed_by = None;
        }
        issue.updated_at = now;
    }
    let actor = actor(&state, &request, &env.caller.login)?;
    let after_snap = record::snapshot(&state, &request, &found, number)
        .map_err(|_| super::errors::issue_not_found())?;
    record::record_changes(
        &mut state,
        &found,
        number,
        &actor,
        &before_snap,
        &after_snap,
    );
    let after = rendered_issue(&state, &request, &found, number).expect("issue exists");
    let mut ctx = events::Ctx::new(&mut state, &request, &found.owner, &found.name);
    events::issue_changes::from_update(&mut ctx, &before, &after).map_err(webhook_err)?;
    Output::of(IssuePayload {
        typename,
        issue: IssueRef {
            id: after.node_id,
            url: Some(after.html_url),
        },
    })
}

fn account_login(state: &State, id: &str) -> Option<String> {
    let database = database_id(id);
    state.accounts().find_map(|account| {
        let matches = database.is_some_and(|value| account.id == value)
            || node_id("User", account.id) == id
            || node_id("Bot", account.id) == id;
        matches.then(|| account.login.clone())
    })
}

fn milestone_number(state: &State, owner: &str, name: &str, id: &str) -> Option<u64> {
    let database = database_id(id);
    state
        .account(owner)?
        .repo(name)?
        .milestones
        .values()
        .find_map(|milestone| {
            let matches = database.is_some_and(|value| milestone.id == value)
                || node_id("Milestone", milestone.id) == id;
            matches.then_some(milestone.number)
        })
}
