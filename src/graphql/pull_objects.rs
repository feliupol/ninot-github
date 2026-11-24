//! Pull request GraphQL objects. Re-exported from [`super::objects`].

use serde::Serialize;

use super::objects::{
    Account, Actor, Connection, Label, PageInfo, ReactionGroup, Ref, Repository, RepositoryName,
    TotalCount,
};
use super::types::{
    CheckConclusionState, CheckStatusState, CommentAuthorAssociation, MergeStateStatus,
    MergeableState, PatchStatus, PullRequestMergeMethod, PullRequestReviewDecision,
    PullRequestReviewState, PullRequestState, StatusState,
};

/// A pull request's auto-merge request.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(super) struct AutoMergeRequest {
    pub enabled_at: chrono::DateTime<chrono::Utc>,
    pub commit_headline: Option<String>,
    pub commit_body: Option<String>,
    pub merge_method: PullRequestMergeMethod,
    pub enabled_by: Option<Actor>,
    /// Nullable. GitHub fills this only when auto-merge records an author email.
    pub author_email: Option<String>,
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
    pub full_database_id: String,
    pub number: u64,
    pub title: String,
    pub body: String,
    pub is_draft: bool,
    pub state: PullRequestState,
    pub url: String,
    pub permalink: String,
    pub closed: bool,
    pub locked: bool,
    pub active_lock_reason: Option<super::types::LockReason>,
    pub created_at: String,
    pub updated_at: String,
    pub closed_at: Option<String>,
    pub merged_at: Option<String>,
    pub merged: bool,
    pub head_ref_name: String,
    pub base_ref_name: String,
    pub head_ref_oid: String,
    pub base_ref_oid: String,
    pub head_ref: Ref,
    pub base_ref: Ref,
    pub is_cross_repository: bool,
    pub mergeable: MergeableState,
    pub merge_state_status: MergeStateStatus,
    pub author: Actor,
    pub author_association: CommentAuthorAssociation,
    pub repository: Repository,
    pub head_repository: Repository,
    pub head_repository_owner: Account,
    pub additions: i64,
    pub deletions: i64,
    pub changed_files: i64,
    pub maintainer_can_modify: bool,
    pub is_in_merge_queue: bool,
    pub is_merge_queue_enabled: bool,
    pub auto_merge_request: Option<AutoMergeRequest>,
    pub merged_by: Option<Actor>,
    pub merge_commit: Option<Commit>,
    /// Always null until a speculative merge commit is stored.
    pub potential_merge_commit: Option<Commit>,
    pub milestone: Option<()>,
    pub labels: Connection<Label>,
    pub assignees: Connection<Actor>,
    pub reviews: Connection<Review>,
    pub latest_reviews: Connection<Review>,
    pub review_requests: Connection<ReviewRequest>,
    pub review_threads: Connection<ReviewThread>,
    pub review_decision: Option<PullRequestReviewDecision>,
    pub comments: Connection<IssueCommentLink>,
    /// Every commit the base does not reach. Only the head commit when git stores none.
    pub commits: Connection<PullRequestCommit>,
    pub files: Connection<ChangedFile>,
    pub status_check_rollup: Option<StatusCheckRollup>,
    pub reaction_groups: Vec<ReactionGroup>,
    pub project_cards: Connection<()>,
    pub project_items: Connection<()>,
    pub closing_issues_references: Connection<()>,
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
    pub author_association: CommentAuthorAssociation,
    pub includes_created_edit: bool,
    pub url: String,
    pub reaction_groups: Vec<ReactionGroup>,
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

/// An issue comment link stored on a pull request object.
///
/// Named apart from [`super::objects::IssueComment`] so both can be re-exported.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct IssueCommentLink {
    #[serde(rename = "__typename")]
    pub typename: &'static str,
    pub body: Option<String>,
    pub id: String,
    pub url: String,
    pub author: Option<Actor>,
    pub author_association: CommentAuthorAssociation,
    pub created_at: String,
    pub updated_at: String,
    pub last_edited_at: Option<String>,
    pub includes_created_edit: bool,
    pub is_minimized: bool,
    pub minimized_reason: Option<String>,
    pub reaction_groups: Vec<ReactionGroup>,
    pub viewer_did_author: bool,
}

/// A `PullRequestCommit`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PullRequestCommit {
    #[serde(rename = "__typename")]
    pub typename: &'static str,
    pub commit: HeadCommit,
}

/// A `Commit` named by its object id.
#[derive(Serialize, Clone)]
pub(super) struct Commit {
    #[serde(rename = "__typename")]
    pub typename: &'static str,
    pub oid: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_headline: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_body: Option<String>,
}

/// A pull request's head `Commit`, with its statuses. Earlier commits use this shape with empty
/// statuses.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct HeadCommit {
    #[serde(rename = "__typename")]
    pub typename: &'static str,
    pub oid: String,
    pub message_headline: String,
    pub message_body: String,
    pub authored_date: String,
    pub committed_date: String,
    pub authors: Connection<GitActor>,
    pub parents: TotalCount,
    pub status: Status,
    pub status_check_rollup: Option<StatusCheckRollup>,
}

/// An author of a commit.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct GitActor {
    #[serde(rename = "__typename")]
    pub typename: &'static str,
    pub name: String,
    pub email: String,
    pub user: Option<Actor>,
}

/// A commit's combined `Status`.
#[derive(Serialize)]
pub(super) struct Status {
    pub contexts: Vec<StatusContext>,
}

/// A `StatusContext`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct StatusContext {
    #[serde(rename = "__typename")]
    pub typename: &'static str,
    pub id: String,
    pub context: String,
    pub state: StatusState,
    pub target_url: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub description: Option<String>,
}

/// A `StatusCheckRollup`. Extra keys let [`super::pull_fields`] rebuild `contexts`.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(super) struct StatusCheckRollup {
    #[serde(rename = "__typename")]
    pub typename: &'static str,
    pub id: String,
    pub state: StatusState,
    pub commit: Commit,
    #[serde(rename = "_owner")]
    pub owner: String,
    #[serde(rename = "_name")]
    pub name: String,
}

/// One file a pull request changes.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ChangedFile {
    pub path: String,
    pub additions: i64,
    pub deletions: i64,
    pub change_type: PatchStatus,
}

/// A member of `StatusCheckRollupContext`.
#[derive(Serialize)]
#[serde(tag = "__typename")]
pub(super) enum RollupNode {
    #[serde(rename = "StatusContext", rename_all = "camelCase")]
    StatusContext {
        id: String,
        context: String,
        state: StatusState,
        target_url: Option<String>,
        created_at: String,
        updated_at: String,
        description: Option<String>,
    },
    #[serde(rename = "CheckRun", rename_all = "camelCase")]
    CheckRun {
        id: String,
        name: String,
        status: CheckStatusState,
        conclusion: Option<CheckConclusionState>,
        started_at: Option<String>,
        completed_at: Option<String>,
        details_url: Option<String>,
        check_suite: CheckSuiteNode,
    },
}

/// The check suite a rollup check run points at. `workflowRun` stays null.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CheckSuiteNode {
    #[serde(rename = "__typename")]
    pub typename: &'static str,
    pub workflow_run: Option<RepositoryName>,
}

/// `StatusCheckRollup.contexts`, including the counts `gh pr checks` reads.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RollupContexts {
    pub total_count: usize,
    pub nodes: Vec<RollupNode>,
    pub page_info: PageInfo,
    pub check_run_count: usize,
    pub status_context_count: usize,
    pub check_run_counts_by_state: Vec<StateCount>,
    pub status_context_counts_by_state: Vec<StatusCount>,
}

/// A `CheckRunStateCount`.
#[derive(Serialize)]
pub(super) struct StateCount {
    pub count: usize,
    pub state: super::types::CheckRunState,
}

/// A `StatusContextStateCount`.
#[derive(Serialize)]
pub(super) struct StatusCount {
    pub count: usize,
    pub state: StatusState,
}
