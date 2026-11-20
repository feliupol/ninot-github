//! Issues: what GitHub stores about one, with users, labels and milestones held by reference.

use chrono::{DateTime, Utc};

use super::{Comment, IssueEvent, Name};

/// An issue, its comments and its issue events.
///
/// Users are logins, labels are names in the repository's label store and the milestone is a
/// milestone number. URLs, the comment count, `assignee` and the embedded users, labels and
/// milestone are derived when the issue is rendered.
#[derive(Clone, Debug)]
pub struct Issue {
    /// Number within the repository. The REST `id` is the same number.
    pub number: u64,
    /// GraphQL node id, fixed when the issue is created.
    pub node_id: String,
    /// Title.
    pub title: String,
    /// Markdown description, when one has been set.
    pub body: Option<String>,
    /// Login of the user who opened the issue.
    pub author: Name,
    /// Open or closed.
    pub state: IssueState,
    /// Why the issue was last closed or reopened. Absent until it is first closed.
    pub state_reason: Option<StateReason>,
    /// When the issue was closed. Absent while it is open.
    pub closed_at: Option<DateTime<Utc>>,
    /// Login of the user who closed the issue. Absent while it is open.
    pub closed_by: Option<Name>,
    /// Names of the attached labels, in the order they were attached.
    pub labels: Vec<Name>,
    /// Logins of the assignees, in the order they were assigned. The first is `assignee`.
    pub assignees: Vec<Name>,
    /// Number of the milestone, when one is set.
    pub milestone: Option<u64>,
    /// Whether the conversation is locked.
    pub locked: bool,
    /// Why the conversation is locked, when a reason was given.
    pub lock_reason: Option<LockReason>,
    /// When the issue was created.
    pub created_at: DateTime<Utc>,
    /// When the issue was last changed.
    pub updated_at: DateTime<Utc>,
    /// Comments, oldest first.
    pub comments: Vec<Comment>,
    /// Issue events, oldest first, as `GET .../issues/{number}/events` lists them.
    pub events: Vec<IssueEvent>,
    /// Whether the issue is pinned to the repository.
    pub is_pinned: bool,
    /// Branches created with `gh issue develop`, oldest first.
    pub linked_branches: Vec<LinkedBranch>,
}

/// A git branch linked to an issue.
#[derive(Clone, Debug)]
pub struct LinkedBranch {
    /// GraphQL database id of the linked-branch node.
    pub id: u64,
    /// Branch name without `refs/heads/`.
    pub name: String,
}

/// Whether an issue is open or closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IssueState {
    Open,
    Closed,
}

impl IssueState {
    /// The REST `state` value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Closed => "closed",
        }
    }

    /// Parses a REST `state` value.
    pub fn parse(text: &str) -> Option<Self> {
        [Self::Open, Self::Closed]
            .into_iter()
            .find(|state| state.as_str() == text)
    }
}

/// Why an issue was closed or reopened, as the REST `state_reason` names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StateReason {
    Completed,
    NotPlanned,
    Duplicate,
    Reopened,
}

/// Why a conversation is locked, as the REST `lock_reason` names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LockReason {
    OffTopic,
    TooHeated,
    Resolved,
    Spam,
}

impl LockReason {
    const ALL: [Self; 4] = [Self::OffTopic, Self::TooHeated, Self::Resolved, Self::Spam];

    /// The REST `lock_reason` value, such as `too heated`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OffTopic => "off-topic",
            Self::TooHeated => "too heated",
            Self::Resolved => "resolved",
            Self::Spam => "spam",
        }
    }

    /// Parses a REST `lock_reason` value.
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|reason| reason.as_str() == text)
    }
}

impl Issue {
    /// Whether the issue is open.
    pub fn is_open(&self) -> bool {
        self.state == IssueState::Open
    }

    /// Whether `login` opened the issue.
    pub fn authored_by(&self, login: &str) -> bool {
        self.author.matches(login)
    }

    /// Whether `login` is assigned.
    pub fn is_assigned(&self, login: &str) -> bool {
        self.assignees.iter().any(|user| user.matches(login))
    }

    /// Whether the label `name` is attached.
    pub fn has_label(&self, name: &str) -> bool {
        self.labels.iter().any(|label| label.matches(name))
    }

    /// The position of the comment `id`.
    pub fn comment_index(&self, id: u64) -> Option<usize> {
        self.comments.iter().position(|comment| comment.id == id)
    }
}
