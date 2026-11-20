//! Pull-request GraphQL mutations.

use async_graphql::Error;
use serde::Deserialize;

use super::objects::{AutoMergeRequest, NodeRef};
use super::pulls::{pull_id, pull_location};
use super::types::PullRequestMergeMethod;
use super::{Env, Map, MutationArgs, Output, not_found, read, typed_error};
use crate::render::node_id;
use crate::state::lock;

/// Auto-merge, draft and review-thread resolvers registered from this module.
pub(super) const RESOLVERS: &[(&str, super::Resolver)] = &[
    (
        "Mutation.markPullRequestReadyForReview",
        mark_pull_request_ready,
    ),
    (
        "Mutation.convertPullRequestToDraft",
        convert_pull_request_to_draft,
    ),
    ("Mutation.resolveReviewThread", resolve_review_thread),
    (
        "Mutation.addPullRequestReviewThreadReply",
        reply_to_review_thread,
    ),
    (
        "Mutation.enablePullRequestAutoMerge",
        enable_pull_request_auto_merge,
    ),
    (
        "Mutation.disablePullRequestAutoMerge",
        disable_pull_request_auto_merge,
    ),
];

/// The input of `Mutation.enablePullRequestAutoMerge` and `disablePullRequestAutoMerge` that
/// is applied.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AutoMergeInput {
    pull_request_id: String,
    merge_method: Option<PullRequestMergeMethod>,
    commit_headline: Option<String>,
    commit_body: Option<String>,
    client_mutation_id: Option<String>,
}

/// The payload of the auto-merge mutations.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct AutoMergePayload {
    #[serde(rename = "__typename")]
    typename: &'static str,
    client_mutation_id: Option<String>,
    pull_request: AutoMergePullRequest,
}

/// The pull request of an [`AutoMergePayload`].
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct AutoMergePullRequest {
    #[serde(rename = "__typename")]
    typename: &'static str,
    auto_merge_request: Option<AutoMergeRequest>,
}

/// Enables or disables auto-merge on the persisted pull request.
fn set_pull_request_auto_merge(env: &Env, args: &Map, enabled: bool) -> Result<Output, Error> {
    let MutationArgs { input } = read::<MutationArgs<AutoMergeInput>>(args)?;
    let mut state = lock(&env.state);
    let (owner, repo, number) = pull_location(&state, &input.pull_request_id)
        .ok_or_else(|| not_found("pull request does not exist".to_owned()))?;
    let allowed = state
        .account(&owner)
        .and_then(|account| account.repo(&repo))
        .is_some_and(|repository| repository.settings.allow_auto_merge);
    if enabled && !allowed {
        return Err(typed_error(
            "auto merge is not allowed for this repository",
            "UNPROCESSABLE",
        ));
    }
    let enabled_by = state.account(&env.caller.login).map(|account| {
        crate::render::simple_user(
            &crate::api::ApiRequest::for_urls(&env.web_url, None),
            account,
        )
    });
    let pull = state
        .account_mut(&owner)
        .and_then(|account| account.repo_mut(&repo))
        .and_then(|repository| repository.pull_requests.get_mut(&number))
        .expect("pull location exists");
    let enabled_at = crate::time::now_utc();
    pull.auto_merge_enabled_at = enabled.then_some(enabled_at);
    let data = pull
        .data
        .as_mut()
        .ok_or_else(|| not_found("pull request REST model does not exist".to_owned()))?;
    let auto_merge_request = if enabled {
        let method = match input.merge_method.unwrap_or(PullRequestMergeMethod::Squash) {
            PullRequestMergeMethod::Merge => crate::types::AutoMergeMergeMethod::Merge,
            PullRequestMergeMethod::Rebase => crate::types::AutoMergeMergeMethod::Rebase,
            PullRequestMergeMethod::Squash => crate::types::AutoMergeMergeMethod::Squash,
        };
        data.auto_merge = Some(crate::types::AutoMerge {
            commit_message: input.commit_body.clone().unwrap_or_default(),
            commit_title: input.commit_headline.clone().unwrap_or_default(),
            enabled_by: enabled_by.unwrap_or_else(|| data.user.clone()),
            merge_method: method,
        });
        Some(super::pull_view::auto_merge(data, enabled_at))
    } else {
        data.auto_merge = None;
        None
    };
    Output::of(AutoMergePayload {
        typename: if enabled {
            "EnablePullRequestAutoMergePayload"
        } else {
            "DisablePullRequestAutoMergePayload"
        },
        client_mutation_id: input.client_mutation_id,
        pull_request: AutoMergePullRequest {
            typename: "PullRequest",
            auto_merge_request,
        },
    })
}

fn enable_pull_request_auto_merge(env: &Env, _: &Map, args: &Map) -> Result<Output, Error> {
    set_pull_request_auto_merge(env, args, true)
}

fn disable_pull_request_auto_merge(env: &Env, _: &Map, args: &Map) -> Result<Output, Error> {
    set_pull_request_auto_merge(env, args, false)
}

/// The payload of a mutation that changes a pull request.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct PullRequestPayload {
    #[serde(rename = "__typename")]
    typename: &'static str,
    pull_request: NodeRef,
}

/// The payload of `Mutation.resolveReviewThread`.
#[derive(serde::Serialize)]
struct ResolveReviewThreadPayload {
    #[serde(rename = "__typename")]
    typename: &'static str,
    thread: NodeRef,
}

/// The payload of `Mutation.addPullRequestReviewThreadReply`.
#[derive(serde::Serialize)]
struct ReplyPayload {
    #[serde(rename = "__typename")]
    typename: &'static str,
    comment: NodeRef,
}

/// The input of the mutations that change one pull request.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PullRequestInput {
    pull_request_id: String,
}

fn mark_pull_request_ready(env: &Env, _: &Map, args: &Map) -> Result<Output, Error> {
    set_pull_request_draft(env, args, false)
}

fn convert_pull_request_to_draft(env: &Env, _: &Map, args: &Map) -> Result<Output, Error> {
    set_pull_request_draft(env, args, true)
}

fn set_pull_request_draft(env: &Env, args: &Map, draft: bool) -> Result<Output, Error> {
    let MutationArgs { input } = read::<MutationArgs<PullRequestInput>>(args)?;
    let id = input.pull_request_id;
    let mut state = lock(&env.state);
    let actor = state.account(&env.caller.login).map(|account| {
        crate::render::simple_user(
            &crate::api::ApiRequest::for_urls(&env.web_url, None),
            account,
        )
    });
    let event_id = state.next_id();
    for account in state.accounts.values_mut() {
        for repo in account.repos.values_mut() {
            for (number, pull) in &mut repo.pull_requests {
                let expected = node_id("PullRequest", pull_id(repo.id, *number));
                if id == expected || pull.data.as_ref().is_some_and(|data| data.node_id == id) {
                    let flips = pull
                        .data
                        .as_ref()
                        .is_some_and(|data| data.draft.unwrap_or(false) != draft);
                    if flips {
                        let actor = actor.clone().ok_or_else(|| {
                            not_found(format!(
                                "Could not resolve to a User '{}'",
                                env.caller.login
                            ))
                        })?;
                        pull.apply_event(crate::state::NewTimelineEvent {
                            id: event_id,
                            kind: if draft {
                                crate::state::TimelineEventKind::ConvertToDraft
                            } else {
                                crate::state::TimelineEventKind::ReadyForReview
                            },
                            actor,
                            created_at: crate::time::now_utc(),
                            commit_id: None,
                            requested_reviewer: None,
                            requested_team: None,
                        })
                        .map_err(|message| typed_error(message, "UNPROCESSABLE"))?;
                    }
                    let typename = if draft {
                        "ConvertPullRequestToDraftPayload"
                    } else {
                        "MarkPullRequestReadyForReviewPayload"
                    };
                    let pull = NodeRef {
                        typename: "PullRequest",
                        id: expected,
                    };
                    return Output::of(PullRequestPayload {
                        typename,
                        pull_request: pull,
                    });
                }
            }
        }
    }
    Err(not_found(format!(
        "Could not resolve to a node with the global id of '{id}'"
    )))
}

/// The input of `Mutation.resolveReviewThread` that is applied.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResolveReviewThreadInput {
    thread_id: String,
}

fn resolve_review_thread(env: &Env, _: &Map, args: &Map) -> Result<Output, Error> {
    let MutationArgs { input } = read::<MutationArgs<ResolveReviewThreadInput>>(args)?;
    let id = input.thread_id;
    let mut state = lock(&env.state);
    for account in state.accounts.values_mut() {
        for repo in account.repos.values_mut() {
            for pull in repo.pull_requests.values_mut() {
                for thread in &mut pull.review_threads {
                    if thread.id == id {
                        thread.is_resolved = true;
                        let thread = NodeRef {
                            typename: "PullRequestReviewThread",
                            id,
                        };
                        return Output::of(ResolveReviewThreadPayload {
                            typename: "ResolveReviewThreadPayload",
                            thread,
                        });
                    }
                }
            }
        }
    }
    Err(not_found(format!("Could not resolve review thread '{id}'")))
}

/// The input of `Mutation.addPullRequestReviewThreadReply` that is applied.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReplyInput {
    pull_request_review_thread_id: String,
    body: String,
}

fn reply_to_review_thread(env: &Env, _: &Map, args: &Map) -> Result<Output, Error> {
    let MutationArgs { input } = read::<MutationArgs<ReplyInput>>(args)?;
    let id = input.pull_request_review_thread_id;
    let mut state = lock(&env.state);
    for account in state.accounts.values_mut() {
        for repo in account.repos.values_mut() {
            for (number, pull) in &mut repo.pull_requests {
                for (index, thread) in pull.review_threads.iter_mut().enumerate() {
                    if thread.id == id {
                        let comment_id = node_id(
                            "PullRequestReviewComment",
                            pull_id(repo.id, *number)
                                .wrapping_mul(1000)
                                .wrapping_add(index as u64 + 1),
                        );
                        thread.comments.push(crate::state::ReviewThreadComment {
                            id: comment_id.clone(),
                            body: input.body,
                            updated_at: crate::state::now(),
                        });
                        let comment = NodeRef {
                            typename: "PullRequestReviewComment",
                            id: comment_id,
                        };
                        return Output::of(ReplyPayload {
                            typename: "AddPullRequestReviewThreadReplyPayload",
                            comment,
                        });
                    }
                }
            }
        }
    }
    Err(not_found(format!("Could not resolve review thread '{id}'")))
}
