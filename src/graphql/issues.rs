//! Repository and issue GraphQL fields that read REST issue state.

use async_graphql::Error;
use serde::Deserialize;

use super::issue_view::{
    actor_for, comment_object, issue_from_stored, linked_branch_object, repo_labels,
};
use super::objects::{Connection, GitObject, Issue, Label, Milestone, Ref};
use super::page::page;
use super::query::stored_repo;
use super::types::{IssueState, LabelOrderField, MilestoneState, OrderDirection};
use super::{Env, Map, NumberArgs, Output, PageArgs, RepositoryKey, not_found, read};
use crate::render::node_id;
use crate::state::{Issue as Stored, State, lock};
use ninot_common::git;

pub(super) const RESOLVERS: &[(&str, super::Resolver)] = &[
    ("Repository.labels", repository_labels_field),
    ("Repository.issues", repository_issues_field),
    ("Repository.issue", repository_issue_field),
    ("Repository.issueOrPullRequest", issue_or_pull_request),
    ("Repository.milestone", repository_milestone_field),
    ("Repository.ref", repository_ref_field),
    ("Issue.labels", issue_labels_field),
    ("Issue.comments", issue_comments_field),
    ("Issue.assignees", issue_assignees_field),
    ("Issue.assignedActors", issue_assignees_field),
    ("Issue.linkedBranches", issue_linked_branches_field),
];

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LabelsArgs {
    first: Option<i64>,
    last: Option<i64>,
    after: Option<String>,
    query: Option<String>,
    order_by: Option<LabelOrder>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LabelOrder {
    field: LabelOrderField,
    direction: OrderDirection,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct IssuesArgs {
    first: Option<i64>,
    last: Option<i64>,
    after: Option<String>,
    states: Option<OneOrMany<IssueState>>,
    order_by: Option<IssueOrder>,
    filter_by: Option<IssueFilter>,
}

/// A GraphQL list argument, which accepts one value or a list.
#[derive(Deserialize, Clone)]
#[serde(untagged)]
enum OneOrMany<T> {
    One(T),
    Many(Vec<T>),
}

impl<T> OneOrMany<T> {
    fn into_vec(self) -> Vec<T> {
        match self {
            Self::One(value) => vec![value],
            Self::Many(values) => values,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct IssueOrder {
    field: super::types::IssueOrderField,
    direction: OrderDirection,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct IssueFilter {
    assignee: Option<String>,
    created_by: Option<String>,
    mentioned: Option<String>,
    labels: Option<Vec<String>>,
    states: Option<OneOrMany<IssueState>>,
}

#[derive(Deserialize)]
struct RefArgs {
    #[serde(rename = "qualifiedName")]
    qualified_name: String,
}

fn repository_labels_field(env: &Env, parent: &Map, args: &Map) -> Result<Output, Error> {
    let args: LabelsArgs = read(args)?;
    let RepositoryKey { owner, name } = read(parent)?;
    let state = lock(&env.state);
    let request = env.request();
    let Some(account) = state.account(&owner.login) else {
        return Output::of(Connection::<Label>::complete(Vec::new()));
    };
    let Some(repo) = account.repo(&name) else {
        return Output::of(Connection::<Label>::complete(Vec::new()));
    };
    let mut labels = repo_labels(&state, &request, account, repo);
    if let Some(query) = args.query.as_deref().filter(|query| !query.is_empty()) {
        let needle = query.to_lowercase();
        labels.retain(|label| label.name.to_lowercase().contains(&needle));
    }
    if let Some(order) = args.order_by {
        match order.field {
            LabelOrderField::Name | LabelOrderField::IssueCount => {
                labels.sort_by_key(|label| label.name.to_lowercase())
            }
            LabelOrderField::CreatedAt => {}
        }
        if order.direction == OrderDirection::Desc {
            labels.reverse();
        }
    }
    Output::of(page(labels, args.first, args.last, args.after))
}

fn repository_issues_field(env: &Env, parent: &Map, args: &Map) -> Result<Output, Error> {
    let args: IssuesArgs = read(args)?;
    let RepositoryKey { owner, name } = read(parent)?;
    let state = lock(&env.state);
    let account = state
        .account(&owner.login)
        .ok_or_else(|| Error::new("repository does not exist"))?;
    let repo = account
        .repo(&name)
        .ok_or_else(|| Error::new("repository does not exist"))?;
    let mut issues: Vec<&Stored> = repo.issues.values().collect();
    let states = args
        .filter_by
        .as_ref()
        .and_then(|filter| filter.states.clone())
        .or(args.states)
        .map(OneOrMany::into_vec)
        .unwrap_or_else(|| vec![IssueState::Open]);
    issues.retain(|issue| {
        let state = if issue.is_open() {
            IssueState::Open
        } else {
            IssueState::Closed
        };
        states.contains(&state)
    });
    if let Some(filter) = &args.filter_by {
        if let Some(assignee) = &filter.assignee {
            issues.retain(|issue| issue.is_assigned(assignee));
        }
        if let Some(author) = &filter.created_by {
            issues.retain(|issue| issue.authored_by(author));
        }
        if let Some(labels) = &filter.labels {
            issues.retain(|issue| labels.iter().all(|name| issue.has_label(name)));
        }
        let _ = &filter.mentioned;
    }
    match args.order_by.as_ref().map(|order| order.field) {
        Some(super::types::IssueOrderField::UpdatedAt) => issues.sort_by(|a, b| {
            a.updated_at
                .cmp(&b.updated_at)
                .then(a.number.cmp(&b.number))
        }),
        Some(super::types::IssueOrderField::Comments) => issues.sort_by(|a, b| {
            a.comments
                .len()
                .cmp(&b.comments.len())
                .then(a.number.cmp(&b.number))
        }),
        _ => issues.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then(a.number.cmp(&b.number))
        }),
    }
    if args
        .order_by
        .as_ref()
        .is_none_or(|order| order.direction == OrderDirection::Desc)
    {
        issues.reverse();
    }
    let nodes: Vec<Issue> = issues
        .into_iter()
        .map(|issue| issue_from_stored(env, &state, account, repo, issue))
        .collect();
    Output::of(page(nodes, args.first, args.last, args.after))
}

fn repository_issue_field(env: &Env, parent: &Map, args: &Map) -> Result<Output, Error> {
    let NumberArgs { number } = read(args)?;
    let number = u64::try_from(number).unwrap_or_default();
    match load_issue(env, parent, number)? {
        Some(issue) => Output::of(issue),
        None => Err(not_found(format!(
            "Could not resolve to an issue with the number of {number}."
        ))),
    }
}

fn issue_or_pull_request(env: &Env, parent: &Map, args: &Map) -> Result<Output, Error> {
    let NumberArgs { number } = super::read(args)?;
    let number = u64::try_from(number).unwrap_or_default();
    if let Some(issue) = load_issue(env, parent, number)? {
        return Output::of(issue);
    }
    match super::pulls::pull_request(env, parent, args) {
        Ok(output) => Ok(output),
        Err(_) => Err(not_found(format!(
            "Could not resolve to an Issue or PullRequest with the number of {number}."
        ))),
    }
}

fn load_issue(env: &Env, parent: &Map, number: u64) -> Result<Option<Issue>, Error> {
    let RepositoryKey { owner, name } = read(parent)?;
    let state = lock(&env.state);
    let Some(account) = state.account(&owner.login) else {
        return Ok(None);
    };
    let Some(repo) = account.repo(&name) else {
        return Ok(None);
    };
    let Some(issue) = repo.issues.get(&number) else {
        return Ok(None);
    };
    Ok(Some(issue_from_stored(env, &state, account, repo, issue)))
}

fn repository_milestone_field(env: &Env, parent: &Map, args: &Map) -> Result<Output, Error> {
    let NumberArgs { number } = read(args)?;
    let number = u64::try_from(number).unwrap_or_default();
    let RepositoryKey { owner, name } = read(parent)?;
    let state = lock(&env.state);
    let Some(repo) = super::read::repo(&state, &owner.login, &name) else {
        return Output::of(None::<Milestone>);
    };
    let Some(milestone) = repo.milestones.get(&number) else {
        return Output::of(None::<Milestone>);
    };
    Output::of(Some(Milestone {
        id: node_id("Milestone", milestone.id),
        number: i64::try_from(milestone.number).unwrap_or(i64::MAX),
        title: milestone.title.clone(),
        description: milestone.description.clone(),
        state: match milestone.state {
            crate::types::NullableMilestoneState::Open => MilestoneState::Open,
            crate::types::NullableMilestoneState::Closed => MilestoneState::Closed,
        },
        url: env
            .urls()
            .repo(&owner.login, &name)
            .milestone(milestone.number)
            .html,
        due_on: None,
    }))
}

fn repository_ref_field(env: &Env, parent: &Map, args: &Map) -> Result<Output, Error> {
    let RefArgs { qualified_name } = read(args)?;
    let Some(git_dir) =
        stored_repo(&lock(&env.state), parent)?.and_then(|repo| repo.git_dir.clone())
    else {
        return Output::of(None::<Ref>);
    };
    let qualified = if qualified_name.starts_with("refs/") {
        qualified_name
    } else {
        format!("refs/heads/{qualified_name}")
    };
    let found = git::refs(&git_dir, &qualified)
        .ok()
        .and_then(|refs| refs.into_iter().find(|found| found.name == qualified));
    let Some(found) = found else {
        return Output::of(None::<Ref>);
    };
    let name = found
        .name
        .strip_prefix("refs/heads/")
        .unwrap_or(&found.name)
        .to_owned();
    Output::of(Some(Ref {
        typename: "Ref",
        name,
        prefix: "refs/heads/",
        target: Some(GitObject {
            typename: "Commit",
            oid: found.sha,
        }),
        repository: None,
    }))
}

fn issue_parent<'a>(
    env: &'a Env,
    parent: &Map,
) -> Result<(std::sync::MutexGuard<'a, State>, String, String, u64), Error> {
    #[derive(Deserialize)]
    struct IssueKey {
        number: u64,
        repository: RepositoryKey,
    }
    let IssueKey { number, repository } = read(parent)?;
    Ok((
        lock(&env.state),
        repository.owner.login,
        repository.name,
        number,
    ))
}

fn issue_labels_field(env: &Env, parent: &Map, args: &Map) -> Result<Output, Error> {
    let PageArgs { first, last, after } = read(args)?;
    let (state, owner, name, number) = issue_parent(env, parent)?;
    let account = state.account(&owner).ok_or_else(|| Error::new("missing"))?;
    let repo = account.repo(&name).ok_or_else(|| Error::new("missing"))?;
    let issue = repo
        .issues
        .get(&number)
        .ok_or_else(|| Error::new("missing"))?;
    let request = env.request();
    let labels = repo_labels(&state, &request, account, repo)
        .into_iter()
        .filter(|label| issue.has_label(&label.name))
        .collect();
    Output::of(page(labels, first, last, after))
}

fn issue_comments_field(env: &Env, parent: &Map, args: &Map) -> Result<Output, Error> {
    let PageArgs { first, last, after } = read(args)?;
    let (state, owner, name, number) = issue_parent(env, parent)?;
    let account = state.account(&owner).ok_or_else(|| Error::new("missing"))?;
    let repo = account.repo(&name).ok_or_else(|| Error::new("missing"))?;
    let issue = repo
        .issues
        .get(&number)
        .ok_or_else(|| Error::new("missing"))?;
    let comments = issue
        .comments
        .iter()
        .map(|comment| comment_object(env, &state, account, repo, issue, comment))
        .collect();
    Output::of(page(comments, first, last, after))
}

fn issue_assignees_field(env: &Env, parent: &Map, args: &Map) -> Result<Output, Error> {
    let PageArgs { first, last, after } = read(args)?;
    let (state, owner, name, number) = issue_parent(env, parent)?;
    let repo = super::read::repo(&state, &owner, &name).ok_or_else(|| Error::new("missing"))?;
    let issue = repo
        .issues
        .get(&number)
        .ok_or_else(|| Error::new("missing"))?;
    let assignees = issue
        .assignees
        .iter()
        .filter_map(|login| actor_for(env, &state, login.as_str()))
        .collect();
    Output::of(page(assignees, first, last, after))
}

fn issue_linked_branches_field(env: &Env, parent: &Map, args: &Map) -> Result<Output, Error> {
    let PageArgs { first, last, after } = read(args)?;
    let (state, owner, name, number) = issue_parent(env, parent)?;
    let account = state.account(&owner).ok_or_else(|| Error::new("missing"))?;
    let repo = account.repo(&name).ok_or_else(|| Error::new("missing"))?;
    let issue = repo
        .issues
        .get(&number)
        .ok_or_else(|| Error::new("missing"))?;
    let branches = issue
        .linked_branches
        .iter()
        .map(|branch| linked_branch_object(env, account, repo, branch))
        .collect();
    Output::of(page(branches, first, last, after))
}

/// Locates a stored issue by GraphQL id.
pub(super) fn issue_location(state: &State, id: &str) -> Option<(String, String, u64)> {
    state.accounts().find_map(|account| {
        account.repos().find_map(|repo| {
            repo.issues.values().find_map(|issue| {
                (issue.node_id == id)
                    .then(|| (account.login.clone(), repo.name.clone(), issue.number))
            })
        })
    })
}
