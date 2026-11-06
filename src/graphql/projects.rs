//! Typed issue projections for the Projects GraphQL boundary.

use serde::Serialize;

use crate::state::{ProjectItem, State};
use crate::types::IssueLabelsItem;

/// An exhausted connection's pagination metadata.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PageInfo {
    /// No later page is omitted by this projection.
    has_next_page: bool,
    /// No earlier page is omitted by this projection.
    has_previous_page: bool,
    /// Cursor for an empty connection.
    start_cursor: Option<String>,
    /// Cursor for an empty connection.
    end_cursor: Option<String>,
}

/// A complete collection returned through a GraphQL connection.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Connection<T> {
    /// Number of objects in the collection.
    total_count: usize,
    /// The persisted objects.
    nodes: Vec<T>,
    /// The collection has no further pages.
    page_info: PageInfo,
}

impl<T> Connection<T> {
    /// Wraps all persisted nodes in an exhausted connection.
    fn complete(nodes: Vec<T>) -> Self {
        Self {
            total_count: nodes.len(),
            nodes,
            page_info: PageInfo {
                has_next_page: false,
                has_previous_page: false,
                start_cursor: None,
                end_cursor: None,
            },
        }
    }
}

/// A label associated with a project issue.
#[derive(Serialize)]
struct Label {
    /// Label text as returned by GitHub.
    name: String,
}

/// The concrete user behind GraphQL's Actor interface.
#[derive(Serialize)]
struct Actor {
    /// Type needed to resolve the Actor interface.
    #[serde(rename = "__typename")]
    type_name: &'static str,
    /// The author's account name.
    login: String,
}

/// A persisted issue comment projected into GraphQL casing.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Comment {
    /// Markdown content; GraphQL requires a non-null string.
    body: String,
    /// Creation timestamp in RFC 3339 format.
    created_at: String,
    /// Absent when the comment has no visible author.
    author: Option<Actor>,
}

/// Repository identity used by the project item adapter.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Repository {
    /// Owner and repository separated by a slash.
    name_with_owner: String,
}

/// An issue's GraphQL fields, distinct from its REST representation.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct IssueNode {
    /// Concrete member of the ProjectV2ItemContent union.
    #[serde(rename = "__typename")]
    type_name: &'static str,
    /// Relay node identifier, not the REST numeric database identifier.
    id: String,
    /// Repository-local issue number.
    number: u64,
    /// Issue title.
    title: String,
    /// Markdown content with GraphQL's non-null default.
    body: String,
    /// GraphQL IssueState uses uppercase enum values.
    state: IssueState,
    /// Repository containing the issue.
    repository: Repository,
    /// Current labels.
    labels: Connection<Label>,
    /// Current comments, including complete pagination metadata.
    comments: Connection<Comment>,
    /// No dependency relationships are currently seeded.
    blocked_by: Connection<()>,
    /// No closing pull-request relationships are currently seeded.
    closed_by_pull_requests_references: Connection<()>,
}

/// GraphQL's issue lifecycle enumeration.
#[derive(Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum IssueState {
    /// An active issue.
    Open,
    /// A closed issue.
    Closed,
}

/// Projects a persisted issue without reinterpreting its REST JSON representation.
pub(super) fn issue_node(state: &State, item: &ProjectItem) -> IssueNode {
    let issue = state
        .account(&item.owner)
        .and_then(|account| account.repo(&item.repository))
        .and_then(|repo| repo.issues.get(&item.issue_number));
    let labels = issue
        .map(|issue| {
            issue
                .data
                .labels
                .iter()
                .filter_map(|label| {
                    let name = match label {
                        IssueLabelsItem::String(name) => Some(name.as_str()),
                        IssueLabelsItem::Object { name, .. } => name.as_deref(),
                    }?;
                    Some(Label {
                        name: name.to_owned(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let comments = issue
        .map(|issue| {
            issue
                .comments
                .iter()
                .map(|comment| Comment {
                    body: comment.body.clone().unwrap_or_default(),
                    created_at: comment.created_at.to_rfc3339(),
                    author: comment.user.as_ref().map(|user| Actor {
                        type_name: if user.type_ == "Bot" { "Bot" } else { "User" },
                        login: user.login.clone(),
                    }),
                })
                .collect()
        })
        .unwrap_or_default();
    IssueNode {
        type_name: "Issue",
        id: issue.map_or_else(
            || crate::render::node_id("Issue", item.issue_number),
            |issue| issue.data.node_id.clone(),
        ),
        number: item.issue_number,
        title: issue.map_or_else(
            || format!("Issue #{}", item.issue_number),
            |issue| issue.data.title.clone(),
        ),
        body: issue
            .and_then(|issue| issue.data.body.clone())
            .unwrap_or_default(),
        state: if issue.is_some_and(|issue| issue.data.state == "closed") {
            IssueState::Closed
        } else {
            IssueState::Open
        },
        repository: Repository {
            name_with_owner: format!("{}/{}", item.owner, item.repository),
        },
        labels: Connection::complete(labels),
        comments: Connection::complete(comments),
        blocked_by: Connection::complete(Vec::new()),
        closed_by_pull_requests_references: Connection::complete(Vec::new()),
    }
}
