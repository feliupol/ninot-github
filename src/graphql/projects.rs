//! The `Issue` objects of search results and project items.

use super::objects::{Connection, Issue, RepositoryName};
use super::types::IssueState;
use super::{Env, read};
use crate::state::{ProjectItem, Repo, State};

/// The `Issue` of a stored issue.
pub(super) fn stored_issue_object(
    env: &Env,
    owner: &str,
    repo: &Repo,
    issue: &crate::state::Issue,
) -> Issue {
    let urls = env.urls();
    Issue {
        typename: "Issue",
        id: read::issue_id(issue).to_owned(),
        number: issue.number,
        title: read::issue_title(issue).to_owned(),
        body: read::issue_body(issue).unwrap_or_default().to_owned(),
        state: read::issue_state(issue),
        url: read::issue_url(&urls, owner, repo, issue),
        locked: read::issue_locked(issue),
        milestone: read::issue_milestone(&urls, owner, repo, issue),
        repository: repository_name(owner, &repo.name),
        labels: Connection::complete(read::issue_labels(issue)),
        comments: Connection::complete(read::issue_comments(issue)),
        blocked_by: Connection::complete(Vec::new()),
        closed_by_pull_requests_references: Connection::complete(Vec::new()),
    }
}

/// The `Issue` of a project item, a placeholder when the issue no longer exists.
pub(super) fn issue_object(env: &Env, state: &State, item: &ProjectItem) -> Issue {
    let number = item.issue_number;
    if let Some(repo) = read::repo(state, &item.owner, &item.repository)
        && let Some(issue) = repo.issues.get(&number)
    {
        return stored_issue_object(env, &item.owner, repo, issue);
    }
    Issue {
        typename: "Issue",
        id: crate::render::node_id("Issue", number),
        number,
        title: format!("Issue #{number}"),
        body: String::new(),
        state: IssueState::Open,
        url: env
            .urls()
            .repo(&item.owner, &item.repository)
            .issue(number)
            .html,
        locked: false,
        milestone: None,
        repository: repository_name(&item.owner, &item.repository),
        labels: Connection::complete(Vec::new()),
        comments: Connection::complete(Vec::new()),
        blocked_by: Connection::complete(Vec::new()),
        closed_by_pull_requests_references: Connection::complete(Vec::new()),
    }
}

/// A repository named by its owner and name.
fn repository_name(owner: &str, repository: &str) -> RepositoryName {
    RepositoryName {
        typename: "Repository",
        name_with_owner: format!("{owner}/{repository}"),
    }
}
