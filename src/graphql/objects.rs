//! The GraphQL objects the resolvers build, typed with the schema's enums and serialized with
//! its field names. A field missing from a struct is unimplemented: the dynamic schema answers
//! it with "Not implemented by ninot-github: Type.field". A field present as `None` is null.

use serde::Serialize;

use super::types::{
    IssueState, MergeableState, MilestoneState, PullRequestReviewDecision, PullRequestReviewState,
    PullRequestState, RepositoryVisibility, StatusState,
};

/// An exhausted connection's pagination metadata.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PageInfo {
    pub has_next_page: bool,
    pub has_previous_page: bool,
    pub start_cursor: Option<String>,
    pub end_cursor: Option<String>,
}

impl PageInfo {
    /// The only page of a connection.
    pub fn single() -> Self {
        Self {
            has_next_page: false,
            has_previous_page: false,
            start_cursor: None,
            end_cursor: None,
        }
    }
}

/// A connection: a page of nodes, the size of the whole collection and the page's position.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Connection<T> {
    pub total_count: usize,
    pub nodes: Vec<T>,
    pub page_info: PageInfo,
}

impl<T> Connection<T> {
    /// Every node of the collection on one page.
    pub fn complete(nodes: Vec<T>) -> Self {
        Self {
            total_count: nodes.len(),
            nodes,
            page_info: PageInfo::single(),
        }
    }
}

/// Some nodes of a connection, without its size or position.
#[derive(Serialize)]
pub(super) struct Nodes<T> {
    pub nodes: Vec<T>,
}

/// A connection's size alone.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TotalCount {
    pub total_count: usize,
}

/// A label.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Label {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_default: Option<bool>,
}

impl Label {
    /// A label named only by `name`, as pull-request objects historically serialized it.
    pub fn named(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            id: None,
            color: None,
            description: None,
            is_default: None,
        }
    }
}

/// The `User` or `Bot` behind an `Actor`, `Author` or `RequestedReviewer`.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Actor {
    #[serde(rename = "__typename")]
    pub typename: &'static str,
    pub login: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Nullable display name. Always present so `User.name` does not fall through to a resolver.
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub database_id: Option<u64>,
}

impl Actor {
    /// The actor for a REST user's `login` and `type`: a `Bot` when the type is `Bot`.
    pub fn of_login(login: &str, kind: &str) -> Self {
        Self {
            typename: if kind == "Bot" { "Bot" } else { "User" },
            login: login.to_owned(),
            id: None,
            name: None,
            database_id: None,
        }
    }
}

/// Any node identified only by its type and id.
#[derive(Serialize)]
pub(super) struct NodeRef {
    #[serde(rename = "__typename")]
    pub typename: &'static str,
    pub id: String,
}

/// A `User` or `Organization`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Account {
    #[serde(rename = "__typename")]
    pub typename: &'static str,
    pub id: String,
    pub database_id: u64,
    pub login: String,
    pub name: Option<String>,
    pub url: String,
    pub created_at: String,
}

/// A `Repository`; the fields that need git or permissions have resolvers.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Repository {
    #[serde(rename = "__typename")]
    pub typename: &'static str,
    pub id: String,
    pub database_id: u64,
    pub name: String,
    pub name_with_owner: String,
    pub owner: Account,
    pub description: Option<String>,
    pub url: String,
    pub ssh_url: String,
    pub is_private: bool,
    pub visibility: RepositoryVisibility,
    pub is_fork: bool,
    pub is_archived: bool,
    pub is_template: bool,
    pub is_mirror: bool,
    pub parent: Option<Box<Repository>>,
    pub created_at: String,
    pub updated_at: String,
    pub pushed_at: Option<String>,
    pub has_issues_enabled: bool,
    pub has_wiki_enabled: bool,
    pub merge_commit_allowed: bool,
    pub rebase_merge_allowed: bool,
    pub squash_merge_allowed: bool,
}

/// A repository named only by its owner and name.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RepositoryName {
    #[serde(rename = "__typename")]
    pub typename: &'static str,
    pub name_with_owner: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// The `Ref` of a branch.
#[derive(Serialize)]
pub(super) struct Ref {
    #[serde(rename = "__typename")]
    pub typename: &'static str,
    pub name: String,
    pub prefix: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<GitObject>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository: Option<RepositoryName>,
}

/// A `GitObject` identified by its object id, typically a `Commit`.
#[derive(Serialize)]
pub(super) struct GitObject {
    #[serde(rename = "__typename")]
    pub typename: &'static str,
    pub oid: String,
}

/// A pull request's auto-merge request.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AutoMergeRequest {
    pub enabled_at: chrono::DateTime<chrono::Utc>,
}

/// A `PullRequest` with the fields read from its stored REST model, reviews, threads and head
/// commit.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PullRequest {
    #[serde(rename = "__typename")]
    pub typename: &'static str,
    pub id: String,
    pub database_id: u64,
    pub number: u64,
    pub title: String,
    pub is_draft: bool,
    pub state: PullRequestState,
    pub url: String,
    pub head_ref_name: String,
    pub base_ref_name: String,
    pub mergeable: MergeableState,
    pub auto_merge_request: Option<AutoMergeRequest>,
    pub author: Actor,
    pub repository: Repository,
    pub labels: Connection<Label>,
    pub reviews: Connection<Review>,
    pub review_requests: Connection<ReviewRequest>,
    pub review_threads: Connection<ReviewThread>,
    pub review_decision: Option<PullRequestReviewDecision>,
    pub comments: Connection<IssueComment>,
    /// Only the head commit.
    pub commits: Nodes<PullRequestCommit>,
}

/// A pull request found by `Repository.pullRequest`, before its other fields exist there.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PullRequestNumber {
    #[serde(rename = "__typename")]
    pub typename: &'static str,
    pub id: String,
    pub database_id: u64,
    pub number: u64,
    pub repository: Repository,
}

/// A `PullRequestReview`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Review {
    #[serde(rename = "__typename")]
    pub typename: &'static str,
    pub id: String,
    pub state: PullRequestReviewState,
    pub body: String,
    pub submitted_at: Option<chrono::DateTime<chrono::Utc>>,
    pub commit: Commit,
    pub author: Option<Actor>,
    pub repository: Repository,
}

/// A `ReviewRequest`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ReviewRequest {
    pub requested_reviewer: Actor,
}

/// A `PullRequestReviewThread`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ReviewThread {
    #[serde(rename = "__typename")]
    pub typename: &'static str,
    pub id: String,
    pub is_resolved: bool,
    pub path: Option<String>,
    pub line: Option<i64>,
    pub comments: Connection<ReviewComment>,
}

/// A `PullRequestReviewComment` in a thread.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ReviewComment {
    #[serde(rename = "__typename")]
    pub typename: &'static str,
    pub id: String,
    pub body: String,
    pub updated_at: String,
}

/// An `IssueComment` on a pull request or issue.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct IssueComment {
    #[serde(rename = "__typename")]
    pub typename: &'static str,
    pub body: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<Actor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author_association: Option<super::types::CommentAuthorAssociation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub includes_created_edit: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_minimized: Option<bool>,
    /// Null when the comment is not minimized. Always present so the field resolves.
    pub minimized_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub viewer_did_author: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reaction_groups: Option<Vec<ReactionGroup>>,
}

/// A `PullRequestCommit`.
#[derive(Serialize)]
pub(super) struct PullRequestCommit {
    #[serde(rename = "__typename")]
    pub typename: &'static str,
    pub commit: HeadCommit,
}

/// A `Commit` named by its object id.
#[derive(Serialize)]
pub(super) struct Commit {
    #[serde(rename = "__typename")]
    pub typename: &'static str,
    pub oid: String,
}

/// A pull request's head `Commit`, with its statuses.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct HeadCommit {
    #[serde(rename = "__typename")]
    pub typename: &'static str,
    pub oid: String,
    pub committed_date: String,
    pub parents: TotalCount,
    pub status: Status,
    pub status_check_rollup: Option<StatusCheckRollup>,
}

/// A commit's combined `Status`.
#[derive(Serialize)]
pub(super) struct Status {
    pub contexts: Vec<StatusContext>,
}

/// A `StatusContext`.
#[derive(Serialize)]
pub(super) struct StatusContext {
    pub context: String,
    pub state: StatusState,
}

/// A `StatusCheckRollup`.
#[derive(Serialize)]
pub(super) struct StatusCheckRollup {
    pub state: StatusState,
}

/// An `Issue`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Issue {
    #[serde(rename = "__typename")]
    pub typename: &'static str,
    pub id: String,
    pub database_id: Option<u64>,
    pub number: u64,
    pub title: String,
    pub body: String,
    pub state: IssueState,
    pub state_reason: Option<super::types::IssueStateReason>,
    pub url: String,
    pub closed: bool,
    pub locked: bool,
    pub active_lock_reason: Option<super::types::LockReason>,
    pub is_pinned: bool,
    pub created_at: String,
    pub updated_at: String,
    pub closed_at: Option<String>,
    pub author: Option<Actor>,
    pub author_association: super::types::CommentAuthorAssociation,
    pub milestone: Option<Milestone>,
    pub repository: Repository,
    pub labels: Connection<Label>,
    pub comments: Connection<IssueComment>,
    pub assignees: Connection<Actor>,
    pub assigned_actors: Connection<Actor>,
    pub reaction_groups: Vec<ReactionGroup>,
    pub project_cards: Connection<()>,
    pub project_items: Connection<()>,
    pub issue_type: Option<()>,
    pub parent: Option<()>,
    pub sub_issues: Connection<()>,
    pub sub_issues_summary: SubIssuesSummary,
    /// No dependency relationships are seeded.
    pub blocked_by: Connection<()>,
    pub blocking: Connection<()>,
    /// No closing pull-request relationships are seeded.
    pub closed_by_pull_requests_references: Connection<()>,
    pub linked_branches: Connection<LinkedBranch>,
}

/// Completion stats for sub-issues, always empty until relationships are stored.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SubIssuesSummary {
    pub total: i64,
    pub completed: i64,
    pub percent_completed: f64,
}

impl SubIssuesSummary {
    pub fn empty() -> Self {
        Self {
            total: 0,
            completed: 0,
            percent_completed: 0.0,
        }
    }
}

/// A `Milestone`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Milestone {
    pub id: String,
    pub number: i64,
    pub title: String,
    pub description: Option<String>,
    pub state: MilestoneState,
    pub url: String,
    /// Nullable due date. Always present so `Milestone.dueOn` resolves to null.
    pub due_on: Option<String>,
}

/// A `ReactionGroup` with a user count.
#[derive(Serialize)]
pub(super) struct ReactionGroup {
    pub content: super::types::ReactionContent,
    pub users: TotalCount,
}

/// A branch linked to an issue.
#[derive(Serialize)]
pub(super) struct LinkedBranch {
    #[serde(rename = "__typename")]
    pub typename: &'static str,
    pub id: String,
    #[serde(rename = "ref")]
    pub ref_: Ref,
}

/// `SearchResultItemConnection`, which names its size `issueCount`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SearchConnection<T> {
    pub issue_count: usize,
    pub nodes: Vec<T>,
    pub page_info: PageInfo,
}

/// A member of the `SearchResultItem` union.
#[derive(Serialize)]
#[serde(untagged)]
pub(super) enum SearchResultItem {
    PullRequest(Box<PullRequest>),
    Issue(Box<Issue>),
}
