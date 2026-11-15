//! The reads of stored state that the resolvers share, converted to the schema's types in one
//! place.

use async_graphql::Error;

use super::objects::{Actor, Comment, Label, Milestone, StatusContext};
use super::types::{
    IssueState, MilestoneState, PullRequestReviewDecision, PullRequestReviewState, StatusState,
};
use crate::state::{self, Account, Issue, Repo, ReviewDecision, State};

/// The stored repository `owner/name`.
pub(super) fn repo<'a>(state: &'a State, owner: &str, name: &str) -> Option<&'a Repo> {
    state.account(owner)?.repo(name)
}

/// A repository's `createdAt`, `updatedAt` and `pushedAt`.
pub(super) fn repo_times(repo: &Repo) -> (String, String, Option<String>) {
    (
        repo.created_at.clone(),
        repo.updated_at.clone(),
        repo.pushed_at.clone(),
    )
}

/// When a repository was last pushed to, or created if never.
pub(super) fn repo_pushed_or_created(repo: &Repo) -> String {
    repo.pushed_at
        .clone()
        .unwrap_or_else(|| repo.created_at.clone())
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
    issue.data.state == "open"
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
    &issue.data.node_id
}

/// An issue's title.
pub(super) fn issue_title(issue: &Issue) -> &str {
    &issue.data.title
}

/// An issue's body.
pub(super) fn issue_body(issue: &Issue) -> Option<&str> {
    issue.data.body.as_deref()
}

/// An issue's web page.
pub(super) fn issue_url(issue: &Issue) -> &str {
    &issue.data.html_url
}

/// Whether an issue is locked.
pub(super) fn issue_locked(issue: &Issue) -> bool {
    issue.data.locked
}

/// Whether `login` opened the issue.
pub(super) fn issue_authored_by(issue: &Issue, login: &str) -> bool {
    issue
        .data
        .user
        .as_ref()
        .is_some_and(|author| author.login.eq_ignore_ascii_case(login))
}

/// An issue's labels.
pub(super) fn issue_labels(issue: &Issue) -> Vec<Label> {
    use crate::types::IssueLabelsItem;

    issue
        .data
        .labels
        .iter()
        .filter_map(|label| match label {
            IssueLabelsItem::String(name) => Some(name.clone()),
            IssueLabelsItem::Object { name, .. } => name.clone(),
        })
        .map(|name| Label { name })
        .collect()
}

/// An issue's comments.
pub(super) fn issue_comments(issue: &Issue) -> Vec<Comment> {
    issue
        .comments
        .iter()
        .map(|comment| Comment {
            body: comment.body.clone().unwrap_or_default(),
            created_at: comment.created_at.to_rfc3339(),
            author: comment
                .user
                .as_ref()
                .map(|user| Actor::of_login(&user.login, &user.type_)),
        })
        .collect()
}

/// An issue's milestone.
pub(super) fn issue_milestone(issue: &Issue) -> Option<Milestone> {
    use crate::types::NullableMilestoneState as Rest;

    issue.data.milestone.as_ref().map(|milestone| Milestone {
        id: milestone.node_id.clone(),
        number: milestone.number,
        title: milestone.title.clone(),
        description: milestone.description.clone(),
        state: match milestone.state {
            Rest::Open => MilestoneState::Open,
            Rest::Closed => MilestoneState::Closed,
        },
        url: milestone.html_url.clone(),
    })
}
