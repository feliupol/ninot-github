//! Stored issue events served by the issue-event REST endpoints.

use chrono::{DateTime, Utc};

/// One issue event GitHub lists on an issue or a repository.
#[derive(Clone, Debug)]
pub struct IssueEvent {
    /// Event id, unique across the server.
    pub id: u64,
    /// What happened, named as the issue-event `event` field names it.
    pub kind: IssueEventKind,
    /// Who did it.
    pub actor: crate::types::SimpleUser,
    /// When it happened.
    pub created_at: DateTime<Utc>,
    /// The merge commit of `merged`, or the new head of `head_ref_force_pushed`.
    pub commit_id: Option<String>,
    /// The user asked to review, for review-request events.
    pub requested_reviewer: Option<crate::types::SimpleUser>,
    /// The team asked to review, for review-request events.
    pub requested_team: Option<crate::types::TeamSimple>,
    /// The label of `labeled` and `unlabeled`.
    pub label: Option<IssueEventLabel>,
    /// The user of `assigned` and `unassigned`.
    pub assignee: Option<crate::types::SimpleUser>,
    /// The milestone title of `milestoned` and `demilestoned`.
    pub milestone_title: Option<String>,
    /// The lock reason of `locked`. Absent reason is JSON null.
    pub lock_reason: Option<String>,
}

/// The label a `labeled` or `unlabeled` event names.
#[derive(Clone, Debug)]
pub struct IssueEventLabel {
    /// Label name.
    pub name: String,
    /// Six-digit hex colour without a leading `#`.
    pub color: String,
}

/// What a stored issue event records, named as the issue-event `event` field names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IssueEventKind {
    /// A draft pull request was marked ready for review.
    ReadyForReview,
    /// A pull request was converted back to a draft.
    ConvertToDraft,
    /// A user or team was asked to review.
    ReviewRequested,
    /// A review request was withdrawn.
    ReviewRequestRemoved,
    /// The head branch was moved to a commit that does not descend from the previous head.
    HeadRefForcePushed,
    /// The pull request was merged.
    Merged,
    /// The issue or pull request was closed.
    Closed,
    /// A closed issue or pull request was reopened.
    Reopened,
    /// A label was added.
    Labeled,
    /// A label was removed.
    Unlabeled,
    /// A user was assigned.
    Assigned,
    /// A user was unassigned.
    Unassigned,
    /// A milestone was set.
    Milestoned,
    /// A milestone was cleared.
    Demilestoned,
    /// The conversation was locked.
    Locked,
    /// The conversation was unlocked.
    Unlocked,
}

impl IssueEventKind {
    /// The issue-event `event` value.
    pub fn name(self) -> &'static str {
        match self {
            Self::ReadyForReview => "ready_for_review",
            Self::ConvertToDraft => "convert_to_draft",
            Self::ReviewRequested => "review_requested",
            Self::ReviewRequestRemoved => "review_request_removed",
            Self::HeadRefForcePushed => "head_ref_force_pushed",
            Self::Merged => "merged",
            Self::Closed => "closed",
            Self::Reopened => "reopened",
            Self::Labeled => "labeled",
            Self::Unlabeled => "unlabeled",
            Self::Assigned => "assigned",
            Self::Unassigned => "unassigned",
            Self::Milestoned => "milestoned",
            Self::Demilestoned => "demilestoned",
            Self::Locked => "locked",
            Self::Unlocked => "unlocked",
        }
    }
}

impl IssueEvent {
    /// An event with no type-specific payload.
    pub(crate) fn new(
        id: u64,
        kind: IssueEventKind,
        actor: crate::types::SimpleUser,
        created_at: DateTime<Utc>,
    ) -> Self {
        Self {
            id,
            kind,
            actor,
            created_at,
            commit_id: None,
            requested_reviewer: None,
            requested_team: None,
            label: None,
            assignee: None,
            milestone_title: None,
            lock_reason: None,
        }
    }
}
