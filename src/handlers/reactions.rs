//! Reactions on issues and pull requests, their comments, review comments and releases.
//!
//! Anyone who can read the repository lists reactions; an authenticated reader adds them, and
//! a reaction is deleted by the user who made it. Adding the same reaction twice returns the
//! first with 200.
//! Source: <https://docs.github.com/en/rest/reactions/reactions>

use std::sync::Mutex;

use super::Handler;
use crate::api::{ApiRequest, ApiResponse, ApiResult, paginate};
use crate::handlers::repos::{Found, find_repo};
use crate::operation::ops;
use crate::render;
use crate::state::{Name, Reaction, ReactionContent, ReactionSubject, State, lock};
use crate::types;

/// Which object an operation's path names.
#[derive(Clone, Copy)]
enum Kind {
    Issue,
    IssueComment,
    ReviewComment,
    Release,
}

fn id_param(request: &ApiRequest, name: &str) -> Result<u64, ApiResponse> {
    request
        .param(name)
        .parse()
        .map_err(|_| ApiResponse::not_found())
}

/// The subject the request's path names, or 404 when the caller cannot see it.
fn subject(
    state: &State,
    request: &ApiRequest,
    found: &Found,
    kind: Kind,
) -> Result<ReactionSubject, ApiResponse> {
    let repo = found.repo(state);
    let exists = match kind {
        Kind::Issue => {
            let number = id_param(request, "issue_number")?;
            let exists = repo.issues.contains_key(&number)
                || repo
                    .pull_requests
                    .get(&number)
                    .is_some_and(|pull| pull.data.is_some());
            exists.then_some(ReactionSubject::Issue {
                repo: repo.id,
                number,
            })
        }
        Kind::IssueComment => {
            let id = id_param(request, "comment_id")?;
            let exists = repo
                .issues
                .values()
                .any(|issue| issue.comment_index(id).is_some())
                || repo.pull_requests.values().any(|pull| {
                    pull.issue_comments
                        .iter()
                        .any(|comment| u64::try_from(comment.id) == Ok(id))
                });
            exists.then_some(ReactionSubject::IssueComment { repo: repo.id, id })
        }
        Kind::ReviewComment => {
            let id = id_param(request, "comment_id")?;
            let exists = repo.pull_requests.values().any(|pull| {
                pull.review_comments
                    .iter()
                    .any(|comment| u64::try_from(comment.id) == Ok(id))
            });
            exists.then_some(ReactionSubject::ReviewComment { repo: repo.id, id })
        }
        Kind::Release => {
            let id = id_param(request, "release_id")?;
            // Drafts are visible only with push access.
            // Source: <https://docs.github.com/en/rest/releases/releases#list-releases>
            let exists = repo
                .releases
                .iter()
                .any(|release| release.id == id && (found.permission.push || !release.draft));
            exists.then_some(ReactionSubject::Release { repo: repo.id, id })
        }
    };
    exists.ok_or_else(ApiResponse::not_found)
}

fn rendered(state: &State, request: &ApiRequest, reaction: &Reaction) -> types::Reaction {
    types::Reaction {
        content: reaction
            .content
            .as_str()
            .parse()
            .expect("every reaction content is in the spec's enum"),
        created_at: reaction.created_at,
        id: render::id(reaction.id),
        node_id: render::node_id("Reaction", reaction.id),
        user: Some(render::nullable_simple_user(
            request,
            state
                .account(reaction.user.as_str())
                .expect("reacting users are accounts"),
        )),
    }
}

/// The rollup of `subject`'s reactions served on the object at `url`.
pub(crate) fn rollup(state: &State, subject: ReactionSubject, url: &str) -> types::ReactionRollup {
    let ([plus1, minus1, laugh, confused, heart, hooray, rocket, eyes], total_count) =
        state.reactions.counts(subject);
    types::ReactionRollup {
        confused,
        eyes,
        heart,
        hooray,
        laugh,
        minus1,
        plus1,
        rocket,
        total_count,
        url: format!("{url}/reactions"),
    }
}

fn list(state: &Mutex<State>, request: &ApiRequest, kind: Kind) -> ApiResult {
    let state = lock(state);
    let found = find_repo(&state, request)?;
    let subject = subject(&state, request, &found, kind)?;
    let wanted = match request.query("content") {
        None => None,
        Some(value) => Some(
            ReactionContent::parse(value)
                .ok_or_else(|| ApiResponse::validation_failed("Reaction", "content", "invalid"))?,
        ),
    };
    let reactions = state
        .reactions
        .on(subject)
        .iter()
        .filter(|reaction| wanted.is_none_or(|content| reaction.content == content))
        .map(|reaction| rendered(&state, request, reaction))
        .collect();
    Ok(paginate(request, &request.path, reactions))
}

fn create(
    state: &Mutex<State>,
    request: &ApiRequest,
    kind: Kind,
    content: &impl std::fmt::Display,
) -> ApiResult {
    let caller = request
        .require_scope(&["repo", "public_repo"])?
        .login
        .clone();
    let content = ReactionContent::parse(&content.to_string())
        .expect("the request model only accepts reaction contents");
    let mut state = lock(state);
    let found = find_repo(&state, request)?;
    let subject = subject(&state, request, &found, kind)?;
    // Source: OpenAPI reactions/create-for-issue 200 ("Reaction exists")
    if let Some(existing) = state.reactions.find(subject, &caller, content) {
        return Ok(ApiResponse::ok_typed(&rendered(&state, request, existing)));
    }
    let reaction = Reaction {
        id: state.next_id(),
        user: Name::new(caller),
        content,
        created_at: crate::time::now_utc(),
    };
    let body = rendered(&state, request, &reaction);
    state.reactions.add(subject, reaction);
    Ok(ApiResponse::created(&body))
}

fn delete(state: &Mutex<State>, request: &ApiRequest, kind: Kind) -> ApiResult {
    let caller = request
        .require_scope(&["repo", "public_repo"])?
        .login
        .clone();
    let id = id_param(request, "reaction_id")?;
    let mut state = lock(state);
    let found = find_repo(&state, request)?;
    let subject = subject(&state, request, &found, kind)?;
    let reaction = state
        .reactions
        .on(subject)
        .iter()
        .find(|reaction| reaction.id == id)
        .ok_or_else(ApiResponse::not_found)?;
    // Unsourced: the docs name no status for deleting another user's reaction.
    if !reaction.user.matches(&caller) {
        return Err(crate::errors::GithubError::PatForbidden.to_response());
    }
    state.reactions.remove(subject, id);
    Ok(ApiResponse::no_content())
}

macro_rules! reaction_handlers {
    ($kind:expr, $list:ty, $create:ty, $body:ty, $delete:ty) => {
        impl Handler for $list {
            fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
                list(state, request, $kind)
            }
        }

        impl Handler for $create {
            fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
                let body: $body = request.json_as()?;
                create(state, request, $kind, &body.content)
            }
        }

        impl Handler for $delete {
            fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
                delete(state, request, $kind)
            }
        }
    };
}

reaction_handlers!(
    Kind::Issue,
    ops::ReactionsListForIssue,
    ops::ReactionsCreateForIssue,
    types::ReactionsCreateForIssueRequest,
    ops::ReactionsDeleteForIssue
);
reaction_handlers!(
    Kind::IssueComment,
    ops::ReactionsListForIssueComment,
    ops::ReactionsCreateForIssueComment,
    types::ReactionsCreateForIssueCommentRequest,
    ops::ReactionsDeleteForIssueComment
);
reaction_handlers!(
    Kind::ReviewComment,
    ops::ReactionsListForPullRequestReviewComment,
    ops::ReactionsCreateForPullRequestReviewComment,
    types::ReactionsCreateForPullRequestReviewCommentRequest,
    ops::ReactionsDeleteForPullRequestComment
);
reaction_handlers!(
    Kind::Release,
    ops::ReactionsListForRelease,
    ops::ReactionsCreateForRelease,
    types::ReactionsCreateForReleaseRequest,
    ops::ReactionsDeleteForRelease
);
