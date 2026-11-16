//! The reads of stored state that the resolvers share, converted to the schema's types in one
//! place.

#![expect(
    clippy::disallowed_types,
    clippy::disallowed_methods,
    reason = "untyped JSON; remove when this module is typed"
)]

use async_graphql::Error;
use chrono::{DateTime, Utc};

use super::objects::{Actor, Comment, Label, Milestone, StatusContext};
use super::types::{
    IssueState, MilestoneState, PullRequestReviewDecision, PullRequestReviewState, StatusState,
};
use crate::state::{self, Account, Issue, Repo, ReviewDecision, State};
use crate::time;
use crate::urls::Urls;

/// The stored repository `owner/name`.
pub(super) fn repo<'a>(state: &'a State, owner: &str, name: &str) -> Option<&'a Repo> {
    state.account(owner)?.repo(name)
}

/// A repository's `createdAt`, `updatedAt` and `pushedAt`.
pub(super) fn repo_times(repo: &Repo) -> (String, String, Option<String>) {
    (
        time::format(&repo.created_at),
        time::format(&repo.updated_at),
        repo.pushed_at.as_ref().map(time::format),
    )
}

/// When a repository was last pushed to, or created if never.
pub(super) fn repo_pushed_or_created(repo: &Repo) -> DateTime<Utc> {
    repo.pushed_at.unwrap_or(repo.created_at)
}

/// An account's `createdAt`.
pub(super) fn account_created_at(account: &Account) -> String {
    account.created_at.clone()
}

/// A schema enum value spelled `text`.
fn schema_enum<T: serde::de::DeserializeOwned>(text: &str, what: &str) -> Result<T, Error> {
    serde_json::from_value(serde_json::Value::from(text))
        .map_err(|_| Error::new(format!("{what} has the unknown state {text:?}")))
}

/// The commit statuses on `sha`.
pub(super) fn status_contexts(repo: &Repo, sha: &str) -> Vec<StatusContext> {
    repo.commit_statuses
        .get(sha)
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .map(|status| StatusContext {
            context: status.context.clone(),
            state: match status.state {
                state::StatusState::Error => StatusState::Error,
                state::StatusState::Failure => StatusState::Failure,
                state::StatusState::Pending => StatusState::Pending,
                state::StatusState::Success => StatusState::Success,
            },
        })
        .collect()
}

/// A stored review's state, which REST and the schema spell alike.
pub(super) fn review_state(
    review: &crate::types::PullRequestReview,
) -> Result<PullRequestReviewState, Error> {
    schema_enum(&review.state, &format!("review {}", review.id))
}

/// A pull request's seeded review decision.
pub(super) fn seeded_review_decision(
    pull: &crate::state::PullRequest,
) -> Option<PullRequestReviewDecision> {
    pull.review_decision.map(|decision| match decision {
        ReviewDecision::Approved => PullRequestReviewDecision::Approved,
        ReviewDecision::ChangesRequested => PullRequestReviewDecision::ChangesRequested,
        ReviewDecision::ReviewRequired => PullRequestReviewDecision::ReviewRequired,
    })
}

/// Whether `login` may push to `account`'s `repo`, as `State::permission` grants it to a user.
pub(super) fn can_push(account: &Account, repo: &Repo, login: &str) -> bool {
    account.is_administered_by(login)
        || account
            .member_permission(login, &repo.name)
            .is_some_and(|permission| permission.push || permission.admin)
}

/// Whether an issue is open.
pub(super) fn issue_open(issue: &Issue) -> bool {
    issue.is_open()
}

/// An issue's state.
pub(super) fn issue_state(issue: &Issue) -> IssueState {
    if issue_open(issue) {
        IssueState::Open
    } else {
        IssueState::Closed
    }
}

/// An issue's node id.
pub(super) fn issue_id(issue: &Issue) -> &str {
    &issue.node_id
}

/// An issue's title.
pub(super) fn issue_title(issue: &Issue) -> &str {
    &issue.title
}

/// An issue's body.
pub(super) fn issue_body(issue: &Issue) -> Option<&str> {
    issue.body.as_deref()
}

/// An issue's web page.
pub(super) fn issue_url(urls: &Urls<'_>, owner: &str, repo: &Repo, issue: &Issue) -> String {
    urls.repo(owner, &repo.name).issue(issue.number).html
}

/// Whether an issue is locked.
pub(super) fn issue_locked(issue: &Issue) -> bool {
    issue.locked
}

/// Whether `login` opened the issue.
pub(super) fn issue_authored_by(issue: &Issue, login: &str) -> bool {
    issue.authored_by(login)
}

/// An issue's labels.
pub(super) fn issue_labels(issue: &Issue) -> Vec<Label> {
    issue
        .labels
        .iter()
        .map(|name| Label {
            name: name.as_str().to_owned(),
        })
        .collect()
}

/// An issue's comments.
pub(super) fn issue_comments(issue: &Issue) -> Vec<Comment> {
    issue
        .comments
        .iter()
        .map(|comment| Comment {
            body: comment.body.clone(),
            created_at: comment.created_at.to_rfc3339(),
            author: comment
                .author
                .as_ref()
                .map(|login| Actor::of_login(login.as_str(), "User")),
        })
        .collect()
}

/// An issue's milestone.
pub(super) fn issue_milestone(
    urls: &Urls<'_>,
    owner: &str,
    repo: &Repo,
    issue: &Issue,
) -> Option<Milestone> {
    use crate::types::NullableMilestoneState as Rest;

    let milestone = repo.milestones.get(&issue.milestone?)?;
    Some(Milestone {
        id: crate::render::node_id("Milestone", milestone.id),
        number: i64::try_from(milestone.number).expect("milestone number fits i64"),
        title: milestone.title.clone(),
        description: milestone.description.clone(),
        state: match milestone.state {
            Rest::Open => MilestoneState::Open,
            Rest::Closed => MilestoneState::Closed,
        },
        url: urls
            .repo(owner, &repo.name)
            .milestone(milestone.number)
            .html,
    })
}
