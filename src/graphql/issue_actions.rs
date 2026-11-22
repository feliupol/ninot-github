//! Lock, pin, delete, transfer, develop and label mutations.

use async_graphql::Error;
use serde::Deserialize;

use super::issues::issue_location as locate_issue;
use super::objects::NodeRef;
use super::pulls::pull_location;
use super::types::LockReason as GqlLock;
use super::{Env, Map, MutationArgs, Output, database_id, read};
use crate::handlers::issues::{record, rendered_issue};
use crate::handlers::repos::Found;
use crate::render::node_id;
use crate::state::{LinkedBranch, LockReason, State, lock};
use crate::webhooks::events;
use ninot_common::git;

pub(super) const RESOLVERS: &[(&str, super::Resolver)] = &[
    ("Mutation.lockLockable", lock_lockable),
    ("Mutation.unlockLockable", unlock_lockable),
    ("Mutation.pinIssue", pin_issue),
    ("Mutation.unpinIssue", unpin_issue),
    ("Mutation.deleteIssue", delete_issue),
    ("Mutation.transferIssue", transfer_issue),
    ("Mutation.createLinkedBranch", create_linked_branch),
];

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct IssueIdInput {
    issue_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LockInput {
    lockable_id: String,
    lock_reason: Option<GqlLock>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UnlockInput {
    lockable_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TransferInput {
    issue_id: String,
    repository_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LinkedBranchInput {
    issue_id: String,
    oid: String,
    repository_id: Option<String>,
    name: Option<String>,
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

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct LockPayload {
    #[serde(rename = "__typename")]
    typename: &'static str,
    locked_record: Locked,
}

#[derive(serde::Serialize)]
struct Locked {
    #[serde(rename = "__typename")]
    typename: &'static str,
    locked: bool,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct UnlockPayload {
    #[serde(rename = "__typename")]
    typename: &'static str,
    unlocked_record: Locked,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct LinkedPayload {
    #[serde(rename = "__typename")]
    typename: &'static str,
    linked_branch: LinkedOut,
}

#[derive(serde::Serialize)]
struct LinkedOut {
    id: String,
    #[serde(rename = "ref")]
    ref_: NameRef,
}

#[derive(serde::Serialize)]
struct NameRef {
    name: String,
}

fn lock_lockable(env: &Env, _: &Map, args: &Map) -> Result<Output, Error> {
    let MutationArgs { input } = read::<MutationArgs<LockInput>>(args)?;
    set_locked(
        env,
        &input.lockable_id,
        true,
        input.lock_reason.map(rest_lock),
    )
}

fn unlock_lockable(env: &Env, _: &Map, args: &Map) -> Result<Output, Error> {
    let MutationArgs { input } = read::<MutationArgs<UnlockInput>>(args)?;
    set_locked(env, &input.lockable_id, false, None)
}

fn rest_lock(reason: GqlLock) -> LockReason {
    match reason {
        GqlLock::OffTopic => LockReason::OffTopic,
        GqlLock::TooHeated => LockReason::TooHeated,
        GqlLock::Resolved => LockReason::Resolved,
        GqlLock::Spam => LockReason::Spam,
    }
}

fn set_locked(
    env: &Env,
    id: &str,
    locked: bool,
    reason: Option<LockReason>,
) -> Result<Output, Error> {
    let mut state = lock(&env.state);
    let (owner, name, number) = locate_issue(&state, id)
        .or_else(|| pull_as_issue(&state, id))
        .ok_or_else(|| super::errors::resolve_node(id))?;
    let permission = state
        .permission(Some(&env.caller), &owner, &name)
        .ok_or_else(super::errors::push_access)?;
    if !permission.push {
        return Err(super::errors::push_access());
    }
    let found = Found {
        owner: owner.clone(),
        name: name.clone(),
        permission,
    };
    let request = env.request();
    let was_locked = found
        .repo(&state)
        .issues
        .get(&number)
        .map(|issue| issue.locked)
        .or_else(|| {
            found
                .repo(&state)
                .pull_requests
                .get(&number)
                .and_then(|pull| pull.data.as_ref())
                .map(|data| data.locked)
        })
        .unwrap_or(false);
    let reason_text = reason.map(|reason| reason.as_str().to_owned());
    // `lockedRecord` is the `Lockable` interface, so the payload names the concrete type.
    let mut concrete = "Issue";
    if let Some(issue) = found.repo_mut(&mut state).issues.get_mut(&number) {
        issue.locked = locked;
        issue.lock_reason = if locked { reason } else { None };
        issue.updated_at = crate::time::now_utc();
    } else if let Some(pull) = found
        .repo_mut(&mut state)
        .pull_requests
        .get_mut(&number)
        .and_then(|pull| pull.data.as_mut())
    {
        concrete = "PullRequest";
        pull.locked = locked;
        pull.active_lock_reason = if locked { reason_text.clone() } else { None };
        pull.updated_at = crate::time::now_utc();
    }
    if was_locked != locked {
        if let Ok(actor) = actor(&state, &request, &env.caller.login) {
            if locked {
                record::record_locked(&mut state, &found, number, actor, reason_text);
            } else {
                record::record_unlocked(&mut state, &found, number, actor);
            }
        }
        if let Some(issue) = rendered_issue(&state, &request, &found, number) {
            let mut ctx = events::Ctx::new(&mut state, &request, &found.owner, &found.name);
            if locked {
                events::issues::locked(&mut ctx, &issue).map_err(webhook_err)?;
            } else {
                events::issues::unlocked(&mut ctx, &issue).map_err(webhook_err)?;
            }
        }
    }
    if locked {
        Output::of(LockPayload {
            typename: "LockLockablePayload",
            locked_record: Locked {
                typename: concrete,
                locked: true,
            },
        })
    } else {
        Output::of(UnlockPayload {
            typename: "UnlockLockablePayload",
            unlocked_record: Locked {
                typename: concrete,
                locked: false,
            },
        })
    }
}

fn pin_issue(env: &Env, _: &Map, args: &Map) -> Result<Output, Error> {
    let MutationArgs { input } = read::<MutationArgs<IssueIdInput>>(args)?;
    set_pinned(env, &input.issue_id, true)
}

fn unpin_issue(env: &Env, _: &Map, args: &Map) -> Result<Output, Error> {
    let MutationArgs { input } = read::<MutationArgs<IssueIdInput>>(args)?;
    set_pinned(env, &input.issue_id, false)
}

fn set_pinned(env: &Env, issue_id: &str, pinned: bool) -> Result<Output, Error> {
    let mut state = lock(&env.state);
    let (owner, name, number) =
        locate_issue(&state, issue_id).ok_or_else(|| super::errors::resolve_node(issue_id))?;
    let pinned_count = state
        .account(&owner)
        .and_then(|account| account.repo(&name))
        .map(|repo| repo.issues.values().filter(|issue| issue.is_pinned).count())
        .unwrap_or(0);
    if pinned && pinned_count >= 3 {
        return Err(super::errors::pinned_limit());
    }
    let issue = state
        .account_mut(&owner)
        .and_then(|account| account.repo_mut(&name))
        .and_then(|repo| repo.issues.get_mut(&number))
        .ok_or_else(super::errors::issue_not_found)?;
    issue.is_pinned = pinned;
    issue.updated_at = crate::time::now_utc();
    let id = issue.node_id.clone();
    Output::of(IssuePayload {
        typename: if pinned {
            "PinIssuePayload"
        } else {
            "UnpinIssuePayload"
        },
        issue: IssueRef { id, url: None },
    })
}

fn delete_issue(env: &Env, _: &Map, args: &Map) -> Result<Output, Error> {
    let MutationArgs { input } = read::<MutationArgs<IssueIdInput>>(args)?;
    let mut state = lock(&env.state);
    let (owner, name, number) = locate_issue(&state, &input.issue_id)
        .ok_or_else(|| super::errors::resolve_node(input.issue_id))?;
    let permission = state
        .permission(Some(&env.caller), &owner, &name)
        .ok_or_else(super::errors::admin_required)?;
    if !permission.admin {
        return Err(super::errors::admin_required());
    }
    let repo_id = node_id(
        "Repository",
        state
            .account(&owner)
            .and_then(|account| account.repo(&name))
            .map(|repo| repo.id)
            .unwrap_or(0),
    );
    found_repo_mut(&mut state, &owner, &name)
        .ok_or_else(super::errors::repository_not_found)?
        .issues
        .remove(&number);
    #[derive(serde::Serialize)]
    #[serde(rename_all = "camelCase")]
    struct DeletePayload {
        #[serde(rename = "__typename")]
        typename: &'static str,
        repository: NodeRef,
    }
    Output::of(DeletePayload {
        typename: "DeleteIssuePayload",
        repository: NodeRef {
            typename: "Repository",
            id: repo_id,
        },
    })
}

fn transfer_issue(env: &Env, _: &Map, args: &Map) -> Result<Output, Error> {
    let MutationArgs { input } = read::<MutationArgs<TransferInput>>(args)?;
    let mut state = lock(&env.state);
    let (owner, name, number) = locate_issue(&state, &input.issue_id)
        .ok_or_else(|| super::errors::resolve_node(input.issue_id))?;
    let (dest_owner, dest_name) = repo_location(&state, &input.repository_id)
        .ok_or_else(super::errors::resolve_destination)?;
    if !owner.eq_ignore_ascii_case(&dest_owner) {
        return Err(super::errors::transfer_same_owner());
    }
    let mut issue = found_repo_mut(&mut state, &owner, &name)
        .and_then(|repo| repo.issues.remove(&number))
        .ok_or_else(super::errors::issue_not_found)?;
    let dest = found_repo_mut(&mut state, &dest_owner, &dest_name)
        .ok_or_else(super::errors::destination_repository_missing)?;
    // Labels and milestones stay only when the destination already defines them.
    issue
        .labels
        .retain(|label| dest.labels.contains_key(label.folded()));
    if issue
        .milestone
        .is_some_and(|number| !dest.milestones.contains_key(&number))
    {
        issue.milestone = None;
    }
    let new_number = crate::handlers::issues::next_subject_number(dest);
    issue.number = new_number;
    issue.node_id = format!("I_{dest_owner}_{dest_name}_{new_number}");
    issue.updated_at = crate::time::now_utc();
    dest.issues.insert(new_number, issue);
    let url = env
        .urls()
        .repo(&dest_owner, &dest_name)
        .issue(new_number)
        .html;
    Output::of(IssuePayload {
        typename: "TransferIssuePayload",
        issue: IssueRef {
            id: format!("I_{dest_owner}_{dest_name}_{new_number}"),
            url: Some(url),
        },
    })
}

fn create_linked_branch(env: &Env, _: &Map, args: &Map) -> Result<Output, Error> {
    let MutationArgs { input } = read::<MutationArgs<LinkedBranchInput>>(args)?;
    let mut state = lock(&env.state);
    let (owner, name, number) = locate_issue(&state, &input.issue_id)
        .ok_or_else(|| super::errors::resolve_node(input.issue_id))?;
    let (repo_owner, repo_name) = match &input.repository_id {
        Some(id) => {
            repo_location(&state, id).ok_or_else(super::errors::resolve_repository_short)?
        }
        None => (owner.clone(), name.clone()),
    };
    let git_dir = state
        .account(&repo_owner)
        .and_then(|account| account.repo(&repo_name))
        .and_then(|repo| repo.git_dir.clone())
        .ok_or_else(super::errors::no_git_storage)?;
    let title = state
        .account(&owner)
        .and_then(|account| account.repo(&name))
        .and_then(|repo| repo.issues.get(&number))
        .map(|issue| issue.title.clone())
        .unwrap_or_default();
    let branch = input
        .name
        .clone()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| {
            let slug = title
                .to_lowercase()
                .chars()
                .map(|character| {
                    if character.is_ascii_alphanumeric() {
                        character
                    } else {
                        '-'
                    }
                })
                .collect::<String>();
            let slug = slug.trim_matches('-');
            format!("{number}-{slug}")
        });
    let reference = format!("refs/heads/{branch}");
    git::create_ref(&git_dir, &reference, &input.oid, &crate::handlers::BOT)
        .map_err(|error| Error::new(error.to_string()))?;
    let id = state.next_id();
    found_repo_mut(&mut state, &owner, &name)
        .and_then(|repo| repo.issues.get_mut(&number))
        .ok_or_else(super::errors::issue_not_found)?
        .linked_branches
        .push(LinkedBranch {
            id,
            name: branch.clone(),
        });
    let request = env.request();
    let mut ctx = events::Ctx::new(&mut state, &request, &repo_owner, &repo_name);
    events::create::reference(&mut ctx, &branch, "branch").map_err(webhook_err)?;
    Output::of(LinkedPayload {
        typename: "CreateLinkedBranchPayload",
        linked_branch: LinkedOut {
            id: node_id("LinkedBranch", id),
            ref_: NameRef { name: branch },
        },
    })
}

pub(super) fn repo_location(state: &State, id: &str) -> Option<(String, String)> {
    let database = database_id(id);
    state.accounts().find_map(|account| {
        account.repos().find_map(|repo| {
            let matches =
                database.is_some_and(|id| repo.id == id) || node_id("Repository", repo.id) == id;
            matches.then(|| (account.login.clone(), repo.name.clone()))
        })
    })
}

pub(super) fn label_by_id(state: &State, found: &Found, id: &str) -> Option<crate::types::Label> {
    let request = crate::api::ApiRequest::for_urls("", None);
    crate::handlers::issues::repository_labels(state, &request, found)
        .into_iter()
        .find(|label| label.node_id == *id)
}

pub(super) fn pull_as_issue(state: &State, id: &str) -> Option<(String, String, u64)> {
    pull_location(state, id)
}

pub(super) fn found_repo_mut<'a>(
    state: &'a mut State,
    owner: &str,
    name: &str,
) -> Option<&'a mut crate::state::Repo> {
    state
        .account_mut(owner)
        .and_then(|account| account.repo_mut(name))
}

pub(super) fn webhook_err(error: events::PayloadError) -> Error {
    Error::new(error.to_string())
}

/// The caller as the REST user recorded on issue events and webhooks.
pub(super) fn actor(
    state: &State,
    request: &crate::api::ApiRequest,
    login: &str,
) -> Result<crate::types::SimpleUser, Error> {
    state
        .account(login)
        .map(|account| crate::render::simple_user(request, account))
        .ok_or_else(super::errors::viewer_not_found)
}
