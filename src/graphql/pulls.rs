//! Pull request GraphQL objects and field resolvers.

use std::collections::HashMap;

use async_graphql::Error;
use serde::Deserialize;

use super::objects::{self, Actor, Commit, Connection, Review};
use super::query::repository_object;
use super::types::{PullRequestReviewDecision, PullRequestReviewState};
use super::{AccountKey, Env, Map, NumberArgs, Output, RepositoryKey, database_id};
use crate::render::node_id;
use crate::state::{Account, Repo, State, lock};
use ninot_common::git;

/// `Repository.pullRequest` and review-decision resolvers registered from this module.
pub(super) const RESOLVERS: &[(&str, super::Resolver)] = &[
    ("Repository.pullRequest", pull_request),
    (
        "PullRequest.latestOpinionatedReviews",
        latest_opinionated_reviews,
    ),
    (
        "PullRequestReview.authorCanPushToRepository",
        author_can_push_to_repository,
    ),
];

/// Global id for a pull request, combining its repository id and number.
pub(super) fn pull_id(repository_id: u64, number: u64) -> u64 {
    crate::ids::pull_database_id(repository_id, number)
}

/// The result of `Repository.pullRequest`.
#[derive(serde::Serialize)]
#[serde(untagged)]
enum FoundPullRequest {
    /// A pull request with its REST model.
    Full(Box<objects::PullRequest>),
    /// A pull request seeded for GraphQL review state alone.
    Number(Box<objects::PullRequestNumber>),
}

/// `Repository.pullRequest(number)`.
pub(super) fn pull_request(env: &Env, parent: &Map, args: &Map) -> Result<Output, Error> {
    let RepositoryKey { owner, name } = super::read(parent)?;
    let NumberArgs { number } = super::read(args)?;
    let number = u64::try_from(number).unwrap_or_default();
    let state = lock(&env.state);
    let account = state
        .account(&owner.login)
        .ok_or_else(|| Error::new("repository does not exist"))?;
    let repo = account
        .repo(&name)
        .ok_or_else(|| Error::new("repository does not exist"))?;
    let pull = repo.pull_requests.get(&number).ok_or_else(|| {
        Error::new(format!(
            "Could not resolve to a PullRequest with the number of {number}."
        ))
    })?;
    Output::of(match pull_object(env, account, repo, number, pull)? {
        Some(full) => FoundPullRequest::Full(Box::new(full)),
        None => FoundPullRequest::Number(Box::new(objects::PullRequestNumber {
            typename: "PullRequest",
            id: node_id("PullRequest", pull_id(repo.id, number)),
            database_id: pull_id(repo.id, number),
            number,
            repository: repository_object(env, account, repo),
        })),
    })
}

/// Finds the pull request encoded by a Relay global id.
pub(super) fn pull_location(state: &State, encoded: &str) -> Option<(String, String, u64)> {
    let id = database_id(encoded)?;
    let repository_id = id >> 32;
    let number = id & u32::MAX as u64;
    state.accounts().find_map(|account| {
        account
            .repos()
            .find(|repo| repo.id == repository_id && repo.pull_requests.contains_key(&number))
            .map(|repo| (account.login.clone(), repo.name.clone(), number))
    })
}

/// A review as the review rules see it: author login, whether the author can push, state.
pub(super) type Opinion = (Option<String>, bool, PullRequestReviewState);

/// The indices, in submission order, of each pusher's latest approving, changes-requesting or
/// dismissed review: `PullRequest.latestOpinionatedReviews`.
///
/// Only reviewers with push access count, whatever `writersOnly` says: reviews from
/// non-pushers are absent with the default `writersOnly: false`. Logins compare
/// case-insensitively.
///
/// Source: `PullRequest.latestOpinionatedReviews` ("A list of latest reviews per user
/// associated with the pull request") and `PullRequestReview.authorCanPushToRepository` in
/// `spec/schema.docs.graphql`.
/// Source: observed on github.com 2026-11-15:
/// <https://github.com/octocat/Spoon-Knife/pull/4008> has approvals and a changes request, all
/// from reviewers who cannot push, an empty `latestOpinionatedReviews` and a null decision.
fn latest_opinions(reviews: &[Opinion]) -> Vec<usize> {
    let mut latest: HashMap<String, usize> = HashMap::new();
    for (index, (author, can_push, state)) in reviews.iter().enumerate() {
        if let Some(author) = author
            && *can_push
            && matches!(
                state,
                PullRequestReviewState::Approved
                    | PullRequestReviewState::ChangesRequested
                    | PullRequestReviewState::Dismissed
            )
        {
            latest.insert(author.to_lowercase(), index);
        }
    }
    let mut indices: Vec<usize> = latest.into_values().collect();
    indices.sort_unstable();
    indices
}

/// A pull request's `reviewDecision` from its reviews when none is seeded: any changes request
/// among the latest opinionated reviews wins, then any approval, else null. A dismissed review
/// is no opinion.
///
/// Known gap: GitHub answers `REVIEW_REQUIRED` when branch protection requires more approving
/// reviews than were given. The fake models no required review count, so this never derives it.
///
/// Source: `PullRequest.reviewDecision` in `spec/schema.docs.graphql`.
/// Source: observed on github.com 2026-11-15:
/// <https://github.com/octocat/Spoon-Knife/pull/4008> reads null with only non-pusher reviews;
/// <https://github.com/cli/cli/pull/15009> reads `APPROVED` after one reviewer went from
/// changes requested to approved;
/// <https://github.com/cli/cli/pull/8361> reads `REVIEW_REQUIRED` after the same sequence,
/// because protection required more approvals.
pub(super) fn review_decision(reviews: &[Opinion]) -> Option<PullRequestReviewDecision> {
    let latest: Vec<PullRequestReviewState> = latest_opinions(reviews)
        .into_iter()
        .map(|index| reviews[index].2)
        .collect();
    if latest.contains(&PullRequestReviewState::ChangesRequested) {
        Some(PullRequestReviewDecision::ChangesRequested)
    } else if latest.contains(&PullRequestReviewState::Approved) {
        Some(PullRequestReviewDecision::Approved)
    } else {
        None
    }
}

/// A pull request's reviews, in submission order, with how the review rules see each.
pub(super) fn review_objects(
    env: &Env,
    account: &Account,
    repo: &Repo,
    number: u64,
    pull: &crate::state::PullRequest,
    head_sha: &str,
) -> Result<(Vec<Review>, Vec<Opinion>), Error> {
    let mut opinions = Vec::new();
    let mut reviews = Vec::new();
    for (index, review) in pull.reviews.iter().enumerate() {
        let id = u64::try_from(review.id).unwrap_or(number * 100 + index as u64 + 1);
        let state = super::read::review_state(review)?;
        let author = review.user.as_ref().map(|user| user.login.clone());
        let can_push = author
            .as_deref()
            .is_some_and(|login| super::read::can_push(account, repo, login));
        opinions.push((author, can_push, state));
        reviews.push(Review {
            typename: "PullRequestReview",
            id: node_id("PullRequestReview", id),
            state,
            body: review.body.clone(),
            submitted_at: review.submitted_at,
            commit: Commit {
                typename: "Commit",
                oid: review
                    .commit_id
                    .clone()
                    .unwrap_or_else(|| head_sha.to_owned()),
                message_headline: None,
                message_body: None,
            },
            author: review
                .user
                .as_ref()
                .map(|user| Actor::of_login(&user.login, &user.type_)),
            author_association: super::issue_view::association(
                account,
                review.user.as_ref().map_or("", |user| user.login.as_str()),
            ),
            includes_created_edit: false,
            url: review.html_url.clone(),
            reaction_groups: Vec::new(),
            repository: repository_object(env, account, repo),
        });
    }
    Ok((reviews, opinions))
}

/// The parent `PullRequest` of a pull request field.
#[derive(Deserialize)]
struct PullRequestKey {
    number: u64,
    repository: RepositoryKey,
}

/// `PullRequest.latestOpinionatedReviews`: see [`latest_opinions`]. The connection is complete.
fn latest_opinionated_reviews(env: &Env, parent: &Map, _: &Map) -> Result<Output, Error> {
    let PullRequestKey { number, repository } = super::read(parent)?;
    let state = lock(&env.state);
    let account = state
        .account(&repository.owner.login)
        .ok_or_else(|| Error::new("repository does not exist"))?;
    let repo = account
        .repo(&repository.name)
        .ok_or_else(|| Error::new("repository does not exist"))?;
    let pull = repo
        .pull_requests
        .get(&number)
        .ok_or_else(|| Error::new("pull request does not exist"))?;
    let data = pull
        .data
        .as_ref()
        .ok_or_else(|| Error::new("pull request REST model does not exist"))?;
    let (reviews, opinions) = review_objects(env, account, repo, number, pull, &data.head.sha)?;
    let latest = latest_opinions(&opinions);
    let mut reviews: Vec<Option<Review>> = reviews.into_iter().map(Some).collect();
    let nodes = latest
        .into_iter()
        .filter_map(|index| reviews[index].take())
        .collect();
    Output::of(Connection::complete(nodes))
}

/// The parent `PullRequestReview` of `authorCanPushToRepository`.
#[derive(Deserialize)]
struct ReviewKey {
    author: Option<AccountKey>,
    repository: RepositoryKey,
}

/// `PullRequestReview.authorCanPushToRepository`; false for a review without an author.
fn author_can_push_to_repository(env: &Env, parent: &Map, _: &Map) -> Result<Output, Error> {
    let ReviewKey { author, repository } = super::read(parent)?;
    let state = lock(&env.state);
    let account = state
        .account(&repository.owner.login)
        .ok_or_else(|| Error::new("repository does not exist"))?;
    let repo = account
        .repo(&repository.name)
        .ok_or_else(|| Error::new("repository does not exist"))?;
    Output::of(author.is_some_and(|author| super::read::can_push(account, repo, &author.login)))
}

/// The `PullRequest` object of a pull request with a REST model; `None` without one.
pub(super) fn pull_object(
    env: &Env,
    account: &Account,
    repo: &Repo,
    number: u64,
    pull: &crate::state::PullRequest,
) -> Result<Option<objects::PullRequest>, Error> {
    super::pull_view::pull_object(env, account, repo, number, pull)
}

/// Commit `sha` from git storage at `git_dir`; `None` without storage or for a seeded sha the
/// storage does not contain. A failed read is an error.
pub(super) fn stored_commit(
    git_dir: Option<&std::path::Path>,
    sha: &str,
) -> Result<Option<git::Commit>, Error> {
    let Some(git_dir) = git_dir else {
        return Ok(None);
    };
    let read_error =
        |error: &dyn std::fmt::Display| Error::new(format!("could not read commit {sha}: {error}"));
    let Ok(id) = gix::ObjectId::from_hex(sha.as_bytes()) else {
        return Ok(None);
    };
    let stored = gix::open_opts(git_dir, gix::open::Options::isolated())
        .map_err(|error| read_error(&error))?;
    if !stored.has_object(id) {
        return Ok(None);
    }
    git::commit(git_dir, sha)
        .map(Some)
        .map_err(|error| read_error(&error))
}

#[cfg(test)]
#[path = "pulls_tests.rs"]
mod tests;
