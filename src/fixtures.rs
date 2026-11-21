//! Completes convenient partial fixture seeds into generated GitHub API models.

use std::io;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::api::ApiRequest;
use crate::render;
use crate::state::{
    Account, CheckRunApp, CheckRunRecord, CommitStatusRecord, Issue, IssueState, JobRecord, Name,
    SeededCheckRun, State, StatusState, WorkflowRunRecord, fold,
};
use crate::time;
use crate::types;
use crate::urls::RepoUrls;

/// Public key supplied to the repository Actions-secret fixture.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActionsPublicKeySeed {
    /// GitHub key identifier.
    pub key_id: String,
    /// Base64 encoded X25519 public key.
    pub key: String,
}

/// An existing encrypted repository Actions secret fixture.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActionsSecretSeed {
    /// GitHub Actions secret name.
    pub name: String,
    /// Ciphertext encrypted with the repository's seeded public key.
    pub encrypted_value: String,
    /// Identifier of the repository public key used for encryption.
    pub key_id: String,
}

pub use crate::state::ActionsSecretsPermission;

macro_rules! apply {
    ($target:expr, $seed:expr) => {
        $seed.apply(&mut $target)
    };
}

macro_rules! apply_optional {
    ($target:expr, $seed:expr) => {
        if let SeedField::Present(value) = $seed {
            $target = Some(value);
        }
    };
}

/// Presence-aware sparse fixture field, preserving omitted versus explicit null values.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum SeedField<T> {
    /// Leave the generated response default unchanged.
    #[default]
    Missing,
    /// Supply this explicit fixture value.
    Present(T),
}

impl<T> SeedField<T> {
    /// Creates a field that explicitly supplies `value`.
    pub fn present(value: T) -> Self {
        Self::Present(value)
    }

    /// Whether serialization should omit this seed field.
    pub fn is_missing(&self) -> bool {
        matches!(self, Self::Missing)
    }

    /// Replaces a generated model field only when the fixture explicitly supplied it.
    pub fn apply(self, target: &mut T) {
        if let Self::Present(value) = self {
            *target = value;
        }
    }
}

impl<T: Serialize> Serialize for SeedField<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Missing => serializer.serialize_none(),
            Self::Present(value) => value.serialize(serializer),
        }
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for SeedField<T> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        T::deserialize(deserializer).map(Self::Present)
    }
}

/// Sparse user values accepted while seeding a response model.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct UserSeed {
    /// Stable database identifier.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub id: SeedField<i64>,
    /// GitHub login.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub login: SeedField<String>,
    /// REST node identifier.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub node_id: SeedField<String>,
    /// Avatar URL.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub avatar_url: SeedField<String>,
    /// Email address.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub email: SeedField<Option<String>>,
    /// Events API URL.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub events_url: SeedField<String>,
    /// Followers API URL.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub followers_url: SeedField<String>,
    /// Following API URL template.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub following_url: SeedField<String>,
    /// Gists API URL template.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub gists_url: SeedField<String>,
    /// Legacy Gravatar identifier.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub gravatar_id: SeedField<Option<String>>,
    /// REST profile URL.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub url: SeedField<String>,
    /// Browser profile URL.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub html_url: SeedField<String>,
    /// Account name.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub name: SeedField<Option<String>>,
    /// Account type such as `User` or `Bot`.
    #[serde(rename = "type")]
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub account_type: SeedField<String>,
    /// Organizations API URL.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub organizations_url: SeedField<String>,
    /// Received-events API URL.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub received_events_url: SeedField<String>,
    /// Repositories API URL.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub repos_url: SeedField<String>,
    /// Whether this account is a site administrator.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub site_admin: SeedField<bool>,
    /// Date the user starred the repository, when requested.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub starred_at: SeedField<Option<String>>,
    /// Starred API URL template.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub starred_url: SeedField<String>,
    /// Subscriptions API URL.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub subscriptions_url: SeedField<String>,
    /// API user-view classification.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub user_view_type: SeedField<Option<String>>,
}

impl UserSeed {
    /// The response names of the set fields other than `login`, in alphabetical order.
    fn fields_besides_login(&self) -> Vec<&'static str> {
        [
            ("avatar_url", self.avatar_url.is_missing()),
            ("email", self.email.is_missing()),
            ("events_url", self.events_url.is_missing()),
            ("followers_url", self.followers_url.is_missing()),
            ("following_url", self.following_url.is_missing()),
            ("gists_url", self.gists_url.is_missing()),
            ("gravatar_id", self.gravatar_id.is_missing()),
            ("html_url", self.html_url.is_missing()),
            ("id", self.id.is_missing()),
            ("name", self.name.is_missing()),
            ("node_id", self.node_id.is_missing()),
            ("organizations_url", self.organizations_url.is_missing()),
            ("received_events_url", self.received_events_url.is_missing()),
            ("repos_url", self.repos_url.is_missing()),
            ("site_admin", self.site_admin.is_missing()),
            ("starred_at", self.starred_at.is_missing()),
            ("starred_url", self.starred_url.is_missing()),
            ("subscriptions_url", self.subscriptions_url.is_missing()),
            ("type", self.account_type.is_missing()),
            ("url", self.url.is_missing()),
            ("user_view_type", self.user_view_type.is_missing()),
        ]
        .into_iter()
        .filter(|(_, missing)| !missing)
        .map(|(field, _)| field)
        .collect()
    }
}

/// A team of the repository owner's organization, created when it does not exist.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TeamSeed {
    /// Team slug.
    pub slug: String,
}

/// Sparse repository label values used by issue and pull-request seeds.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct LabelSeed {
    /// Numeric label id.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub id: SeedField<i64>,
    /// REST node identifier.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub node_id: SeedField<String>,
    /// Label API URL.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub url: SeedField<String>,
    /// Label name.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub name: SeedField<String>,
    /// Label description.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub description: SeedField<Option<String>>,
    /// Hexadecimal label color.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub color: SeedField<String>,
    /// Whether GitHub supplies this label by default.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub default: SeedField<bool>,
}

impl LabelSeed {
    /// The response names of the set fields other than `name` that differ from `label`, in
    /// alphabetical order.
    fn conflicts_with(&self, label: &types::Label) -> Vec<&'static str> {
        /// Whether `field` is set to something other than `stored`.
        fn differs<T: PartialEq>(field: &SeedField<T>, stored: &T) -> bool {
            matches!(field, SeedField::Present(value) if value != stored)
        }
        [
            ("color", differs(&self.color, &label.color)),
            ("default", differs(&self.default, &label.default)),
            (
                "description",
                differs(&self.description, &label.description),
            ),
            ("id", differs(&self.id, &label.id)),
            ("node_id", differs(&self.node_id, &label.node_id)),
            ("url", differs(&self.url, &label.url)),
        ]
        .into_iter()
        .filter(|(_, differs)| *differs)
        .map(|(field, _)| field)
        .collect()
    }
}

/// Sparse branch information nested in a pull-request seed.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct PullRequestBranchSeed {
    /// Branch display label.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub label: SeedField<String>,
    /// Branch name.
    #[serde(rename = "ref")]
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub branch_ref: SeedField<String>,
    /// Commit SHA at the branch head.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub sha: SeedField<String>,
    /// Repository hosting this branch, when supplied.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub repo: SeedField<RepositoryReferenceSeed>,
    /// Branch account.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub user: SeedField<Option<UserSeed>>,
}

/// Sparse repository identity nested in a pull-request branch.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct RepositoryReferenceSeed {
    /// Owner and name joined by `/`.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub full_name: SeedField<String>,
}

/// Sparse auto-merge metadata nested in a pull-request seed.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct AutoMergeSeed {
    /// Commit message.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub commit_message: SeedField<String>,
    /// Commit title.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub commit_title: SeedField<String>,
    /// User who enabled auto-merge.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub enabled_by: SeedField<UserSeed>,
    /// Merge strategy.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub merge_method: SeedField<String>,
}

/// Sparse pull-request response values accepted by [`crate::server::Repository`].
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct PullRequestSeed {
    /// Numeric pull-request identifier.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub id: SeedField<i64>,
    /// Pull-request number within the repository.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub number: SeedField<i64>,
    /// Pull-request title.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub title: SeedField<String>,
    /// Browser URL.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub html_url: SeedField<String>,
    /// Creation timestamp.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub created_at: SeedField<String>,
    /// Last update timestamp.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub updated_at: SeedField<String>,
    /// Pull-request author.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub user: SeedField<UserSeed>,
    /// Markdown description.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub body: SeedField<Option<String>>,
    /// Open or closed state.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub state: SeedField<String>,
    /// Whether the pull request is a draft.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub draft: SeedField<bool>,
    /// Whether GitHub considers the branch mergeable.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub mergeable: SeedField<Option<bool>>,
    /// Labels attached to the pull request.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub labels: SeedField<Vec<LabelSeed>>,
    /// Base branch.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub base: SeedField<PullRequestBranchSeed>,
    /// Head branch.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub head: SeedField<PullRequestBranchSeed>,
    /// Users requested to review the pull request.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub requested_reviewers: SeedField<Vec<UserSeed>>,
    /// Teams of the owning organization requested to review the pull request.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub requested_teams: SeedField<Vec<TeamSeed>>,
    /// Auto-merge settings, if enabled.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub auto_merge: SeedField<Option<AutoMergeSeed>>,
    /// Current merge state.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub merged: SeedField<bool>,
    /// Number of issue comments.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub comments: SeedField<i64>,
    /// Whether the pull request is locked.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub locked: SeedField<bool>,
    /// Whether maintainers may modify the head branch.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub maintainer_can_modify: SeedField<bool>,
    /// Number of commits.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub commits: SeedField<i64>,
    /// Number of changed files.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub changed_files: SeedField<i64>,
    /// Number of additions.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub additions: SeedField<i64>,
    /// Number of deletions.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub deletions: SeedField<i64>,
    /// When the pull request was merged; implies `merged` and a closed state.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub merged_at: SeedField<String>,
    /// When the pull request was closed; implies a closed state. A merged pull request closes
    /// when it merges.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub closed_at: SeedField<String>,
    /// Who merged the pull request; the author by default.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub merged_by: SeedField<UserSeed>,
    /// Files listed by `GET /pulls/{number}/files`, instead of those git has between base and
    /// head. Unless seeded too, `changed_files`,
    /// `additions` and `deletions` are their count and sums.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub files: SeedField<Vec<PullRequestFileSeed>>,
}

/// One changed file of a pull-request seed.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PullRequestFileSeed {
    /// Path of the file.
    pub filename: String,
    /// `added`, `removed`, `modified`, `renamed`, `copied`, `changed` or `unchanged`;
    /// `modified` by default.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub status: SeedField<String>,
    /// Lines added.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub additions: SeedField<i64>,
    /// Lines deleted.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub deletions: SeedField<i64>,
    /// Path before a rename.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub previous_filename: SeedField<String>,
}

/// A timeline event seeded on a pull request, applied to it as the action would be.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TimelineEventSeed {
    /// `ready_for_review`, `convert_to_draft`, `review_requested`, `review_request_removed`,
    /// `merged`, `closed` or `reopened`. Force-pushes come from moving the head with
    /// [`crate::server::PullRequest::push_head`].
    pub event: String,
    /// When it happened: no earlier than the pull request's creation or its latest event.
    pub created_at: String,
    /// Who did it; the pull request's author by default.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub actor: SeedField<UserSeed>,
    /// The user a review-request event names.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub requested_reviewer: SeedField<UserSeed>,
    /// The team a review-request event names.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub requested_team: SeedField<TeamSeed>,
    /// The merge commit of a `merged` event; the head by default.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub commit_id: SeedField<String>,
}

/// The author or committer of a fixture commit.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GitIdentitySeed {
    /// Name.
    pub name: String,
    /// Email address.
    pub email: String,
    /// RFC 3339 timestamp.
    pub date: String,
}

/// Sparse issue response values accepted by [`crate::server::Repository`].
///
/// Issues reference users, labels and their comments, so a seed names users by login only and
/// cannot set values the fake derives from them.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct IssueSeed {
    /// Issue title.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub title: SeedField<String>,
    /// Markdown description.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub body: SeedField<Option<String>>,
    /// `open` or `closed`; any other value is rejected.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub state: SeedField<String>,
    /// Labels attached to the issue. A label the repository lacks is added to it; one it has
    /// may repeat but not contradict the stored label's fields.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub labels: SeedField<Vec<LabelSeed>>,
    /// Number of issue comments. Only `0` is accepted: the count is derived from the comments
    /// seeded with `Issue::add_comment`.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub comments: SeedField<i64>,
    /// Whether the issue is locked.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub locked: SeedField<bool>,
    /// User assigned to the issue, by login only. Alone it is the only assignee; with
    /// `assignees` it must be the first of them.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub assignee: SeedField<Option<UserSeed>>,
    /// Users assigned to the issue, by login only.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub assignees: SeedField<Vec<UserSeed>>,
    /// Issue author, by login only. The repository owner when omitted.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub user: SeedField<UserSeed>,
}

/// Sparse review response values accepted by [`crate::server::PullRequest`].
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ReviewSeed {
    /// Numeric review identifier.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub id: SeedField<i64>,
    /// Review state.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub state: SeedField<String>,
    /// Review text.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub body: SeedField<String>,
    /// Review author.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub user: SeedField<Option<UserSeed>>,
    /// Commit reviewed.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub commit_id: SeedField<Option<String>>,
    /// Review submission timestamp.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub submitted_at: SeedField<Option<String>>,
}

/// Sparse issue-comment response values accepted by issue and pull-request handles.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct IssueCommentSeed {
    /// Comment text.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub body: SeedField<String>,
    /// Comment author. On an issue, by login only; a pull-request comment also takes the other
    /// user fields.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub user: SeedField<Option<UserSeed>>,
    /// Creation timestamp.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub created_at: SeedField<String>,
    /// Update timestamp.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub updated_at: SeedField<String>,
}

/// Sparse pull-request review-comment response values.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ReviewCommentSeed {
    /// Source path.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub path: SeedField<String>,
    /// Diff line number.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub line: SeedField<Option<i64>>,
    /// Position in the diff.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub position: SeedField<Option<i64>>,
    /// Comment text.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub body: SeedField<String>,
    /// Comment author.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub user: SeedField<Option<UserSeed>>,
    /// Creation timestamp.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub created_at: SeedField<String>,
    /// Update timestamp.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub updated_at: SeedField<String>,
    /// The review the comment belongs to.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub pull_request_review_id: SeedField<i64>,
    /// The comment this one replies to in its thread.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub in_reply_to_id: SeedField<i64>,
}

/// Sparse review-thread comment used in GraphQL fixture state.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ReviewThreadCommentSeed {
    /// Comment text.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub body: SeedField<String>,
    /// Last update timestamp.
    #[serde(rename = "updatedAt")]
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub updated_at: SeedField<String>,
}

/// Sparse persisted GraphQL review-thread state.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ReviewThreadSeed {
    /// Stable thread node id.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub id: SeedField<String>,
    /// Whether the thread is resolved.
    #[serde(rename = "isResolved")]
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub is_resolved: SeedField<bool>,
    /// File path.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub path: SeedField<Option<String>>,
    /// Diff line.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub line: SeedField<Option<i64>>,
    /// Comments in the thread.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub comments: SeedField<Vec<ReviewThreadCommentSeed>>,
}

/// Sparse workflow-run values accepted by [`crate::server::Repository`].
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct WorkflowRunSeed {
    /// Run id.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub id: SeedField<u64>,
    /// Run name.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub name: SeedField<Option<String>>,
    /// Branch that triggered the run.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub head_branch: SeedField<Option<String>>,
    /// Head commit SHA.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub head_sha: SeedField<String>,
    /// Browser URL for this run.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub html_url: SeedField<String>,
    /// Trigger event.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub event: SeedField<String>,
    /// Run status.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub status: SeedField<Option<String>>,
    /// Completed run conclusion.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub conclusion: SeedField<Option<String>>,
    /// Display title.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub display_title: SeedField<String>,
    /// Workflow path.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub path: SeedField<String>,
    /// Workflow id.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub workflow_id: SeedField<u64>,
    /// Run sequence number.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub run_number: SeedField<u64>,
    /// Created timestamp.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub created_at: SeedField<String>,
    /// Updated timestamp.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub updated_at: SeedField<String>,
}

/// Sparse Actions job-step values.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct JobStepSeed {
    /// Step sequence number.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub number: SeedField<i64>,
    /// Step name.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub name: SeedField<String>,
    /// Step status.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub status: SeedField<String>,
    /// Step conclusion.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub conclusion: SeedField<String>,
}

/// Sparse Actions job values accepted by [`crate::server::Repository`].
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct JobSeed {
    /// Job id.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub id: SeedField<u64>,
    /// Job name.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub name: SeedField<String>,
    /// Job status.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub status: SeedField<String>,
    /// Job conclusion.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub conclusion: SeedField<Option<String>>,
    /// Job start timestamp.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub started_at: SeedField<Option<String>>,
    /// Job completion timestamp.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub completed_at: SeedField<Option<String>>,
    /// Job steps.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub steps: SeedField<Vec<JobStepSeed>>,
}

/// Sparse check-run output fields.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct CheckRunOutputSeed {
    /// Check output title.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub title: SeedField<Option<String>>,
    /// Check summary.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub summary: SeedField<Option<String>>,
    /// Check text.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub text: SeedField<Option<String>>,
    /// Annotation count.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub annotations_count: SeedField<i64>,
    /// Annotation endpoint URL.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub annotations_url: SeedField<String>,
}

/// Sparse check-suite identity embedded in a check-run seed.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct CheckRunCheckSuiteSeed {
    /// Numeric check-suite identifier.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub id: SeedField<i64>,
}

/// Sparse permissions assigned to a GitHub App integration.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct IntegrationPermissionsSeed {
    /// Checks permission.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub checks: SeedField<String>,
    /// Contents permission.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub contents: SeedField<String>,
    /// Deployments permission.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub deployments: SeedField<String>,
    /// Issues permission.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub issues: SeedField<String>,
    /// Metadata permission.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub metadata: SeedField<String>,
}

/// Sparse GitHub App integration embedded in a check-run seed.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct IntegrationSeed {
    /// Numeric application identifier.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub id: SeedField<i64>,
    /// REST node identifier.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub node_id: SeedField<String>,
    /// URL slug.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub slug: SeedField<Option<String>>,
    /// Display name.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub name: SeedField<String>,
    /// Application description.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub description: SeedField<Option<String>>,
    /// External application URL.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub external_url: SeedField<String>,
    /// Browser URL.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub html_url: SeedField<String>,
    /// Creation timestamp.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub created_at: SeedField<String>,
    /// Update timestamp.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub updated_at: SeedField<String>,
    /// Application owner.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub owner: SeedField<UserSeed>,
    /// Application permissions.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub permissions: SeedField<IntegrationPermissionsSeed>,
    /// Subscribed webhook events.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub events: SeedField<Vec<String>>,
}

/// Sparse check-run values accepted by [`crate::server::Commit`].
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct CheckRunSeed {
    /// Check id.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub id: SeedField<u64>,
    /// REST node identifier.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub node_id: SeedField<String>,
    /// Check name.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub name: SeedField<String>,
    /// SHA of the checked commit. Defaults to the owning commit.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub head_sha: SeedField<String>,
    /// API URL.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub url: SeedField<String>,
    /// Browser URL.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub html_url: SeedField<Option<String>>,
    /// Check status.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub status: SeedField<String>,
    /// Check conclusion.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub conclusion: SeedField<Option<String>>,
    /// Details URL.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub details_url: SeedField<Option<String>>,
    /// External system id.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub external_id: SeedField<Option<String>>,
    /// Associated check suite, or explicit `null`.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub check_suite: SeedField<Option<CheckRunCheckSuiteSeed>>,
    /// GitHub App that produced this check, or explicit `null`.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub app: SeedField<Option<IntegrationSeed>>,
    /// Pull requests associated with this check.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub pull_requests: SeedField<Vec<types::PullRequestMinimal>>,
    /// Start timestamp.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub started_at: SeedField<Option<String>>,
    /// Completion timestamp.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub completed_at: SeedField<Option<String>>,
    /// Check output.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub output: SeedField<CheckRunOutputSeed>,
}

/// Sparse commit-status values accepted by [`crate::server::Commit`].
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct CommitStatusSeed {
    /// Status id.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub id: SeedField<u64>,
    /// Status state.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub state: SeedField<String>,
    /// Status context.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub context: SeedField<String>,
    /// Human-readable description.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub description: SeedField<Option<String>>,
    /// Target URL.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub target_url: SeedField<Option<String>>,
    /// Status creator.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub creator: SeedField<Option<UserSeed>>,
    /// Creation timestamp.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub created_at: SeedField<String>,
    /// Update timestamp.
    #[serde(default, skip_serializing_if = "SeedField::is_missing")]
    pub updated_at: SeedField<String>,
}

/// A default workflow run: a completed `CI` push run, created now.
pub(crate) fn workflow_run(id: u64, head_sha: String) -> WorkflowRunRecord {
    let now = crate::time::now_utc();
    WorkflowRunRecord {
        id,
        name: Some("CI".to_owned()),
        display_title: "CI".to_owned(),
        path: ".github/workflows/ci.yml".to_owned(),
        workflow_id: id,
        run_number: id,
        run_attempt: 1,
        event: "push".to_owned(),
        status: Some("completed".to_owned()),
        conclusion: None,
        head_branch: Some("main".to_owned()),
        head_sha,
        html_url: None,
        created_at: now,
        updated_at: now,
        run_started_at: Some(now),
    }
}

/// A default job: completed, named `Job`, created now.
pub(crate) fn job(id: u64) -> JobRecord {
    let now = crate::time::now_utc();
    JobRecord {
        id,
        name: "Job".to_owned(),
        status: types::JobStatus::Completed,
        conclusion: None,
        run_attempt: 1,
        labels: Vec::new(),
        runner_id: None,
        runner_name: None,
        runner_group_id: None,
        steps: Vec::new(),
        created_at: now,
        started_at: now,
        completed_at: Some(now),
    }
}

/// Builds a default check run for a commit: a completed run named `Check`, recorded now.
pub(crate) fn check_run(sha: &str, id: i64) -> CheckRunRecord {
    CheckRunRecord {
        id,
        name: "Check".to_owned(),
        head_sha: sha.to_owned(),
        suite_id: None,
        app: None,
        status: types::CheckRunStatus::Completed,
        conclusion: None,
        created_at: time::now_utc(),
        started_at: None,
        completed_at: None,
        details_url: None,
        external_id: None,
        output_title: None,
        output_summary: None,
        output_text: None,
        annotations: Vec::new(),
        pull_requests: Vec::new(),
        seeded: SeededCheckRun::default(),
    }
}

/// Builds a default commit status: a `success` in the `default` context, set now.
pub(crate) fn commit_status(id: i64) -> CommitStatusRecord {
    let now = time::now_utc();
    CommitStatusRecord {
        id,
        state: StatusState::Success,
        context: "default".to_owned(),
        description: None,
        target_url: None,
        creator: None,
        created_at: now,
        updated_at: now,
    }
}

/// Completes an issue directly from its typed sparse fixture fields.
///
/// Users are seeded by login alone. Labels the repository lacks are added to its label store;
/// a label it has is referenced, and a seed that contradicts it is rejected. The comment count
/// is derived from seeded comments, so only `comments: 0` is accepted.
pub(crate) fn seeded_issue(
    state: &mut State,
    request: &ApiRequest,
    owner: &str,
    repo_name: &str,
    default_author: Name,
    number: u64,
    seed: IssueSeed,
) -> io::Result<Issue> {
    model_id_or_input(number, "issue number")?;
    let now = crate::time::now_utc();
    let mut issue = Issue {
        number,
        node_id: format!("I_{owner}_{repo_name}_{number}"),
        title: format!("Issue #{number}"),
        body: None,
        author: default_author,
        state: IssueState::Open,
        state_reason: None,
        closed_at: None,
        closed_by: None,
        labels: Vec::new(),
        assignees: Vec::new(),
        milestone: None,
        locked: false,
        lock_reason: None,
        created_at: now,
        updated_at: now,
        comments: Vec::new(),
        events: Vec::new(),
        is_pinned: false,
        linked_branches: Vec::new(),
    };
    apply!(issue.title, seed.title);
    apply!(issue.body, seed.body);
    apply!(issue.locked, seed.locked);
    if let SeedField::Present(text) = seed.state {
        issue.state = IssueState::parse(&text).ok_or_else(|| {
            invalid_seed(format!(
                "invalid issue state {text:?}: expected \"open\" or \"closed\""
            ))
        })?;
    }
    if let SeedField::Present(count) = seed.comments
        && count != 0
    {
        return Err(invalid_seed(
            "an issue's comment count is derived from its comments; seed them with \
             Issue::add_comment",
        ));
    }
    if let SeedField::Present(labels) = seed.labels {
        for (index, label) in labels.into_iter().enumerate() {
            let name = seeded_issue_label(state, request, owner, repo_name, label, number, index)?;
            if !issue.has_label(name.as_str()) {
                issue.labels.push(name);
            }
        }
    }
    let assignees = match seed.assignees {
        SeedField::Present(users) => Some(
            users
                .into_iter()
                .map(|user| seeded_login(state, user, "issue assignee"))
                .collect::<io::Result<Vec<_>>>()?,
        ),
        SeedField::Missing => None,
    };
    let assignee = match seed.assignee {
        SeedField::Present(user) => Some(
            user.map(|user| seeded_login(state, user, "issue assignee"))
                .transpose()?,
        ),
        SeedField::Missing => None,
    };
    issue.assignees = match (assignee, assignees) {
        (None, None) => Vec::new(),
        (Some(assignee), None) => assignee.into_iter().collect(),
        (None, Some(assignees)) => assignees,
        (Some(assignee), Some(assignees)) => {
            if assignee.as_ref() != assignees.first() {
                return Err(invalid_seed(
                    "an issue's assignee is the first of its assignees; seed only assignees",
                ));
            }
            assignees
        }
    };
    if let SeedField::Present(user) = seed.user {
        issue.author = seeded_login(state, user, "issue author")?;
    }
    Ok(issue)
}

/// The login a user seed names, creating the user when it does not exist. The seed may set only
/// `login`: issues reference users, whose other fields come from the account.
pub(crate) fn seeded_login(state: &mut State, seed: UserSeed, role: &str) -> io::Result<Name> {
    let extra = seed.fields_besides_login();
    if !extra.is_empty() {
        return Err(invalid_seed(format!(
            "an {role} is seeded by login only; {} come from the account",
            extra.join(", ")
        )));
    }
    let login = match seed.login {
        SeedField::Present(login) => login,
        SeedField::Missing => "admin".to_owned(),
    };
    Ok(Name::new(state.add_user(&login).login.clone()))
}

/// Stores the label a seed names unless the repository has it, and returns its name. A seed that
/// names a stored label may repeat its fields but not change them.
fn seeded_issue_label(
    state: &mut State,
    request: &ApiRequest,
    owner: &str,
    repo_name: &str,
    seed: LabelSeed,
    number: u64,
    index: usize,
) -> io::Result<Name> {
    let label = label_model(request, owner, repo_name, seed.clone(), number, index)?;
    let repo = state
        .account(owner)
        .and_then(|account| account.repo(repo_name))
        .ok_or_else(|| invalid_seed(format!("no repository {owner}/{repo_name}")))?;
    if let Some(stored) = repo.labels.get(fold(&label.name).as_ref()) {
        let rendered = render::label(state, request, owner, repo_name, stored);
        let conflicts = seed.conflicts_with(&rendered);
        if !conflicts.is_empty() {
            return Err(invalid_seed(format!(
                "label {:?} is already stored with other {}",
                stored.name,
                conflicts.join(", ")
            )));
        }
    }
    let repo = state
        .account_mut(owner)
        .and_then(|account| account.repo_mut(repo_name))
        .expect("repository found above");
    Ok(crate::handlers::issues::ensure_label(repo, &label))
}

/// Completes a pull request directly from its typed sparse fixture fields.
pub(crate) fn seeded_pull_request(
    state: &mut State,
    request: &ApiRequest,
    repository_name: (&str, &str),
    default_user: types::SimpleUser,
    default_repository: types::Repository,
    number: u64,
    seed: PullRequestSeed,
) -> io::Result<SeededPullRequest> {
    let (owner, repo_name) = repository_name;
    let repository_id = state
        .account(owner)
        .and_then(|account| account.repo(repo_name))
        .map(|repo| repo.id)
        .unwrap_or(0);
    let repo_urls = request.urls().repo(owner, repo_name);
    let urls = repo_urls.pull(number);
    let signed_number = model_id_or_input(number, "pull request number")?;
    let id = match seed.id {
        SeedField::Present(id) => model_id_signed(id, "pull request")?,
        SeedField::Missing => signed_number,
    };
    let default_sha = default_sha();
    let user = default_user;
    let repository = default_repository;
    let now = crate::time::now_utc();
    let mut pull = types::PullRequest {
        active_lock_reason: None,
        additions: 0,
        assignee: None,
        assignees: Vec::new(),
        author_association: types::AuthorAssociation::Owner,
        auto_merge: None,
        base: types::PullRequestBase {
            label: format!("{owner}:main"),
            ref_: "main".to_owned(),
            repo: repository.clone(),
            sha: default_sha.clone(),
            user: user.clone(),
        },
        body: None,
        changed_files: 0,
        closed_at: None,
        comments: 0,
        comments_url: urls.issue.comments(),
        commits: 1,
        commits_url: urls.commits(),
        created_at: now,
        deletions: 0,
        diff_url: urls.diff(),
        draft: Some(false),
        head: types::PullRequestHead {
            label: format!("{owner}:feature"),
            ref_: "feature".to_owned(),
            repo: repository,
            sha: default_sha.clone(),
            user: user.clone(),
        },
        html_url: urls.html.clone(),
        id,
        issue_url: urls.issue.api.clone(),
        labels: Vec::new(),
        links: types::PullRequestLinks {
            comments: types::Link {
                href: urls.issue.comments(),
            },
            commits: types::Link {
                href: urls.commits(),
            },
            html: types::Link {
                href: urls.html.clone(),
            },
            issue: types::Link {
                href: urls.issue.api.clone(),
            },
            review_comment: types::Link {
                href: repo_urls.review_comment_template(),
            },
            review_comments: types::Link {
                href: urls.review_comments(),
            },
            self_: types::Link {
                href: urls.api.clone(),
            },
            statuses: types::Link {
                href: repo_urls.statuses(&default_sha),
            },
        },
        locked: false,
        maintainer_can_modify: true,
        merge_commit_sha: None,
        mergeable: Some(true),
        mergeable_state: "clean".to_owned(),
        merged: false,
        merged_at: None,
        merged_by: None,
        milestone: None,
        node_id: crate::ids::pull_request_node_id(repository_id, number),
        number: signed_number,
        patch_url: urls.patch(),
        rebaseable: None,
        requested_reviewers: Vec::new(),
        requested_teams: Vec::new(),
        review_comment_url: repo_urls.review_comment_template(),
        review_comments: 0,
        review_comments_url: urls.review_comments(),
        stack: None,
        state: types::PullRequestState::Open,
        statuses_url: repo_urls.statuses(&default_sha),
        title: format!("Pull request #{number}"),
        updated_at: now,
        url: urls.api.clone(),
        user,
    };
    apply!(pull.title, seed.title);
    apply!(pull.body, seed.body);
    if let SeedField::Present(number) = seed.number {
        pull.number = model_id_signed(number, "pull request number")?;
    }
    apply!(pull.html_url, seed.html_url);
    apply_optional!(pull.draft, seed.draft);
    apply!(pull.mergeable, seed.mergeable);
    apply!(pull.comments, seed.comments);
    apply!(pull.locked, seed.locked);
    apply!(pull.maintainer_can_modify, seed.maintainer_can_modify);
    apply!(pull.commits, seed.commits);
    apply!(pull.changed_files, seed.changed_files.clone());
    apply!(pull.additions, seed.additions.clone());
    apply!(pull.deletions, seed.deletions.clone());
    let seeded_state = match seed.state {
        SeedField::Present(value) => Some(parse_enum::<types::PullRequestState>(
            value,
            "pull request state",
        )?),
        SeedField::Missing => None,
    };
    if let SeedField::Present(value) = seed.created_at {
        pull.created_at = parse_timestamp(&value, "pull request created_at")?;
    }
    if let SeedField::Present(value) = seed.updated_at {
        pull.updated_at = parse_timestamp(&value, "pull request updated_at")?;
    }
    if let SeedField::Present(value) = seed.id {
        pull.id = model_id_signed(value, "pull request")?;
    }
    if let SeedField::Present(labels) = seed.labels {
        pull.labels = labels
            .into_iter()
            .enumerate()
            .map(|(index, label)| pull_label(request, owner, repo_name, label, number, index))
            .collect::<io::Result<Vec<_>>>()?;
    }
    if let SeedField::Present(reviewers) = seed.requested_reviewers {
        pull.requested_reviewers = reviewers
            .into_iter()
            .map(|seed| simple_user(state, request, seed))
            .collect::<io::Result<Vec<_>>>()?;
    }
    if let SeedField::Present(teams) = seed.requested_teams {
        pull.requested_teams = teams
            .into_iter()
            .map(|seed| requested_team(state, request, owner, &seed.slug))
            .collect::<io::Result<Vec<_>>>()?;
    }
    if let SeedField::Present(auto_merge_seed) = seed.auto_merge {
        pull.auto_merge = auto_merge_seed
            .map(|seed| auto_merge(state, request, seed))
            .transpose()?;
    }
    if let SeedField::Present(base) = seed.base {
        apply_branch(
            state,
            request,
            base,
            (
                &mut pull.base.label,
                &mut pull.base.ref_,
                &mut pull.base.sha,
                &mut pull.base.user,
                &mut pull.base.repo,
            ),
        )?;
    }
    if let SeedField::Present(head) = seed.head {
        apply_branch(
            state,
            request,
            head,
            (
                &mut pull.head.label,
                &mut pull.head.ref_,
                &mut pull.head.sha,
                &mut pull.head.user,
                &mut pull.head.repo,
            ),
        )?;
    }
    if let SeedField::Present(user_seed) = seed.user {
        pull.user = simple_user(state, request, user_seed)?;
    }
    let files = match seed.files {
        SeedField::Present(files) => {
            let files = files
                .into_iter()
                .map(|file| pull_request_file(&repo_urls, &pull.head.sha, file))
                .collect::<io::Result<Vec<_>>>()?;
            let count = i64::try_from(files.len())
                .map_err(|error| invalid_seed(format!("too many files: {error}")))?;
            let additions = files.iter().map(|file| file.additions).sum();
            let deletions = files.iter().map(|file| file.deletions).sum();
            for (name, field, derived) in [
                ("changed_files", seed.changed_files.clone(), count),
                ("additions", seed.additions.clone(), additions),
                ("deletions", seed.deletions.clone(), deletions),
            ] {
                if matches!(field, SeedField::Present(value) if value != derived) {
                    return Err(invalid_seed(format!(
                        "pull request {name} disagrees with its files ({derived})"
                    )));
                }
            }
            pull.changed_files = count;
            pull.additions = additions;
            pull.deletions = deletions;
            Some(files)
        }
        SeedField::Missing => None,
    };
    let merger = match seed.merged_by {
        SeedField::Present(user_seed) => simple_user(state, request, user_seed)?,
        SeedField::Missing => pull.user.clone(),
    };
    let events = closing_events(
        state,
        &mut pull,
        ClosingSeed {
            state: seeded_state,
            merged: seed.merged,
            merged_at: seed.merged_at,
            closed_at: seed.closed_at,
            merger,
        },
    )?;
    Ok(SeededPullRequest {
        data: pull,
        files,
        events,
    })
}

/// A seeded pull request before it joins the server state. Its REST model is still open and
/// unmerged; applying `events` closes or merges it.
pub(crate) struct SeededPullRequest {
    /// The REST model.
    pub data: types::PullRequest,
    /// Files the seed lists; without them `GET /pulls/{number}/files` reads them from git.
    pub files: Option<Vec<types::DiffEntry>>,
    /// `merged` and `closed` events the seed implies.
    pub events: Vec<crate::state::NewTimelineEvent>,
}

/// The seed fields that decide whether and when a pull request was merged or closed.
struct ClosingSeed {
    /// Seeded `state`.
    state: Option<types::PullRequestState>,
    /// Seeded `merged`.
    merged: SeedField<bool>,
    /// Seeded `merged_at`.
    merged_at: SeedField<String>,
    /// Seeded `closed_at`.
    closed_at: SeedField<String>,
    /// Who merged.
    merger: types::SimpleUser,
}

/// The `merged` and `closed` events a seed implies, refusing contradictory seeds.
///
/// A merged or closed seed without its timestamp is dated at the pull request's `updated_at`.
fn closing_events(
    state: &mut State,
    pull: &mut types::PullRequest,
    seed: ClosingSeed,
) -> io::Result<Vec<crate::state::NewTimelineEvent>> {
    let merged_at = match seed.merged_at {
        SeedField::Present(value) => Some(parse_timestamp(&value, "pull request merged_at")?),
        SeedField::Missing => None,
    };
    let closed_at = match seed.closed_at {
        SeedField::Present(value) => Some(parse_timestamp(&value, "pull request closed_at")?),
        SeedField::Missing => None,
    };
    let merged = match (seed.merged, merged_at) {
        (SeedField::Present(false), Some(_)) => {
            return Err(invalid_seed("merged_at needs merged to be true"));
        }
        (SeedField::Present(merged), _) => merged,
        (SeedField::Missing, merged_at) => merged_at.is_some(),
    };
    let closed = match seed.state {
        Some(types::PullRequestState::Open) if merged || closed_at.is_some() => {
            return Err(invalid_seed(
                "a merged or closed pull request cannot be open",
            ));
        }
        Some(state) => state == types::PullRequestState::Closed,
        None => merged || closed_at.is_some(),
    };
    if merged && pull.draft == Some(true) {
        return Err(invalid_seed("a draft pull request cannot be merged"));
    }
    pull.state = types::PullRequestState::Open;
    if !closed {
        return Ok(Vec::new());
    }
    let explicit = merged_at.is_some() || closed_at.is_some();
    let merged_at = merged.then(|| merged_at.unwrap_or(pull.updated_at));
    let closed_at = closed_at.or(merged_at).unwrap_or(pull.updated_at);
    if let Some(merged_at) = merged_at
        && closed_at != merged_at
    {
        return Err(invalid_seed(
            "a merged pull request closes when it merges: closed_at must equal merged_at",
        ));
    }
    if explicit && closed_at < pull.created_at {
        return Err(invalid_seed("closed_at is before created_at"));
    }
    // A pull request closed without merging is closed by its author.
    let actor = if merged {
        seed.merger
    } else {
        pull.user.clone()
    };
    let mut events = Vec::new();
    if merged {
        events.push(crate::state::NewTimelineEvent {
            id: state.next_id(),
            kind: crate::state::TimelineEventKind::Merged,
            actor: actor.clone(),
            created_at: closed_at,
            commit_id: Some(pull.head.sha.clone()),
            requested_reviewer: None,
            requested_team: None,
        });
    }
    events.push(crate::state::NewTimelineEvent {
        id: state.next_id(),
        kind: crate::state::TimelineEventKind::Closed,
        actor,
        created_at: closed_at,
        commit_id: None,
        requested_reviewer: None,
        requested_team: None,
    });
    Ok(events)
}

/// A seeded changed file as `GET /pulls/{number}/files` lists it.
fn pull_request_file(
    repo: &RepoUrls,
    head_sha: &str,
    seed: PullRequestFileSeed,
) -> io::Result<types::DiffEntry> {
    let additions = field_or(seed.additions, 0);
    let deletions = field_or(seed.deletions, 0);
    let status = parse_enum(
        field_or(seed.status, "modified".to_owned()),
        "pull request file status",
    )?;
    let previous_filename = match seed.previous_filename {
        SeedField::Present(name) => Some(name),
        SeedField::Missing => None,
    };
    let filename = seed.filename;
    Ok(types::DiffEntry {
        additions,
        blob_url: repo.blob_html(head_sha, &filename),
        changes: additions + deletions,
        contents_url: repo.contents(&filename, head_sha),
        deletions,
        patch: None,
        previous_filename,
        raw_url: repo.raw(head_sha, &filename),
        sha: None,
        status,
        filename,
    })
}

/// A team of `org` as review requests name it, created when it does not exist.
pub(crate) fn requested_team(
    state: &mut State,
    request: &ApiRequest,
    org: &str,
    slug: &str,
) -> io::Result<types::TeamSimple> {
    if state
        .account(org)
        .is_some_and(|account| account.kind != crate::state::AccountKind::Organization)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{org} is a user, so its pull requests cannot request teams"),
        ));
    }
    state.add_team(org, slug, &[]);
    let org = state.account(org).expect("team organization exists");
    let team = org.team(slug).expect("team exists");
    Ok(render::team_simple(request, org, team))
}

fn pull_label(
    request: &ApiRequest,
    owner: &str,
    repo: &str,
    seed: LabelSeed,
    number: u64,
    index: usize,
) -> io::Result<types::PullRequestLabelsItem> {
    let label = label_model(request, owner, repo, seed, number, index)?;
    Ok(types::PullRequestLabelsItem {
        archived_at: label.archived_at,
        archived_by: label.archived_by,
        color: label.color,
        default: label.default,
        description: label.description,
        id: label.id,
        name: label.name,
        node_id: label.node_id,
        url: label.url,
    })
}

fn label_model(
    request: &ApiRequest,
    owner: &str,
    repo: &str,
    seed: LabelSeed,
    number: u64,
    index: usize,
) -> io::Result<types::Label> {
    let index = u64::try_from(index)
        .map_err(|error| invalid_seed(format!("invalid label index: {error}")))?;
    let id = number
        .checked_mul(100)
        .and_then(|base| base.checked_add(index + 1))
        .ok_or_else(|| invalid_seed("label id overflow"))?;
    let mut label = types::Label {
        archived_at: None,
        archived_by: None,
        color: "ededed".to_owned(),
        default: false,
        description: Some(String::new()),
        id: model_id_or_input(id, "label")?,
        name: "label".to_owned(),
        node_id: format!("LA_{owner}_{repo}_{number}_{index}"),
        url: String::new(),
    };
    apply!(label.name, seed.name);
    if let SeedField::Present(id) = seed.id {
        label.id = model_id_signed(id, "label")?;
    }
    apply!(label.node_id, seed.node_id);
    apply!(label.description, seed.description);
    apply!(label.color, seed.color);
    apply!(label.default, seed.default);
    apply!(label.url, seed.url);
    if label.url.is_empty() {
        label.url = request.urls().repo(owner, repo).label(&label.name);
    }
    Ok(label)
}

fn auto_merge(
    state: &mut State,
    request: &ApiRequest,
    seed: AutoMergeSeed,
) -> io::Result<types::AutoMerge> {
    let enabled_by_seed = field_or(seed.enabled_by, UserSeed::default());
    let enabled_by = simple_user(state, request, enabled_by_seed)?;
    let mut auto = types::AutoMerge {
        commit_message: String::new(),
        commit_title: String::new(),
        enabled_by,
        merge_method: types::AutoMergeMergeMethod::Merge,
    };
    apply!(auto.commit_message, seed.commit_message);
    apply!(auto.commit_title, seed.commit_title);
    if let SeedField::Present(method) = seed.merge_method {
        auto.merge_method = parse_enum(method, "auto-merge method")?;
    }
    Ok(auto)
}

fn apply_branch(
    state: &mut State,
    request: &ApiRequest,
    seed: PullRequestBranchSeed,
    target: (
        &mut String,
        &mut String,
        &mut String,
        &mut types::SimpleUser,
        &mut types::Repository,
    ),
) -> io::Result<()> {
    let (label, branch_ref, sha, user, repository) = target;
    apply!(*label, seed.label);
    apply!(*branch_ref, seed.branch_ref);
    apply!(*sha, seed.sha);
    if let SeedField::Present(Some(user_seed)) = seed.user {
        *user = simple_user(state, request, user_seed)?;
    }
    if let SeedField::Present(repo_seed) = seed.repo
        && let SeedField::Present(full_name) = repo_seed.full_name
    {
        repository.full_name = full_name;
    }
    Ok(())
}

/// Completes an Actions run from typed sparse fixture fields.
pub(crate) fn seeded_workflow_run(seed: WorkflowRunSeed) -> io::Result<WorkflowRunRecord> {
    let id = field_or(seed.id, 1);
    model_id_or_input(id, "workflow run")?;
    let head_sha = field_or(seed.head_sha, default_sha());
    let mut run = workflow_run(id, head_sha);
    apply!(run.name, seed.name);
    apply!(run.head_branch, seed.head_branch);
    if let SeedField::Present(url) = seed.html_url {
        run.html_url = Some(url);
    }
    apply!(run.event, seed.event);
    apply!(run.status, seed.status);
    apply!(run.conclusion, seed.conclusion);
    apply!(run.display_title, seed.display_title);
    apply!(run.path, seed.path);
    if let SeedField::Present(value) = seed.workflow_id {
        model_id_or_input(value, "workflow")?;
        run.workflow_id = value;
    }
    if let SeedField::Present(value) = seed.run_number {
        model_id_or_input(value, "workflow run number")?;
        run.run_number = value;
    }
    apply_time(
        seed.created_at,
        &mut run.created_at,
        "workflow run created_at",
    )?;
    apply_time(
        seed.updated_at,
        &mut run.updated_at,
        "workflow run updated_at",
    )?;
    Ok(run)
}

/// Completes an Actions job from typed sparse fixture fields.
pub(crate) fn seeded_job(seed: JobSeed, index: usize) -> io::Result<JobRecord> {
    let id = match seed.id {
        SeedField::Present(id) => model_id_or_input(id, "workflow job")?,
        SeedField::Missing => indexed_model_id(index, "workflow job")?,
    };
    let mut job = job(node_id(id));
    apply!(job.name, seed.name);
    if let SeedField::Present(value) = seed.conclusion {
        job.conclusion = value
            .map(|value| parse_enum(value, "workflow job conclusion"))
            .transpose()?;
    }
    match seed.started_at {
        SeedField::Missing => {}
        SeedField::Present(Some(value)) => {
            job.started_at = parse_timestamp(&value, "workflow job started_at")?;
        }
        SeedField::Present(None) => {
            return Err(invalid_seed("workflow job started_at cannot be null"));
        }
    }
    if let SeedField::Present(value) = seed.completed_at {
        job.completed_at = value
            .map(|value| parse_timestamp(&value, "workflow job completed_at"))
            .transpose()?;
    }
    if let SeedField::Present(status) = seed.status {
        job.status = parse_enum(status, "workflow job status")?;
    }
    if let SeedField::Present(steps) = seed.steps {
        job.steps = steps
            .into_iter()
            .enumerate()
            .map(|(step_index, seed)| seeded_job_step(seed, step_index))
            .collect::<io::Result<Vec<_>>>()?;
    }
    Ok(job)
}

/// Completes an Actions job step from typed sparse fixture fields.
fn seeded_job_step(seed: JobStepSeed, index: usize) -> io::Result<types::JobStepsItem> {
    let number = match seed.number {
        SeedField::Present(number) => model_id_signed(number, "job step")?,
        SeedField::Missing => indexed_model_id(index, "job step")?,
    };
    let mut step = types::JobStepsItem {
        completed_at: None,
        conclusion: None,
        name: format!("Step {}", index + 1),
        number,
        started_at: None,
        status: types::JobStepsItemStatus::Completed,
    };
    apply!(step.name, seed.name);
    if let SeedField::Present(status) = seed.status {
        step.status = parse_enum(status, "workflow job step status")?;
    }
    if let SeedField::Present(conclusion) = seed.conclusion {
        step.conclusion = Some(parse_enum(conclusion, "workflow job step conclusion")?);
    }
    Ok(step)
}

/// Completes a check run from typed sparse fixture fields.
pub(crate) fn seeded_check_run(
    state: &mut State,
    request: &ApiRequest,
    sha: &str,
    seed: CheckRunSeed,
    fallback: usize,
) -> io::Result<CheckRunRecord> {
    let id = match seed.id {
        SeedField::Present(id) => model_id_or_input(id, "check run")?,
        SeedField::Missing => indexed_model_id(fallback, "check run")?,
    };
    let mut check = check_run(sha, id);
    if let SeedField::Present(node_id) = seed.node_id {
        check.seeded.node_id = Some(node_id);
    }
    apply!(check.name, seed.name);
    if let SeedField::Present(head_sha) = seed.head_sha {
        check.seeded.head_sha = Some(head_sha);
    }
    if let SeedField::Present(url) = seed.url {
        check.seeded.url = Some(url);
    }
    if let SeedField::Present(html_url) = seed.html_url {
        check.seeded.html_url = Some(html_url);
    }
    if let SeedField::Present(status) = seed.status {
        check.status = parse_enum(status, "check run status")?;
    }
    if let SeedField::Present(conclusion) = seed.conclusion {
        check.conclusion = conclusion
            .map(|value| parse_enum(value, "check run conclusion"))
            .transpose()?;
    }
    apply!(check.details_url, seed.details_url);
    apply!(check.external_id, seed.external_id);
    if let SeedField::Present(value) = seed.started_at {
        check.started_at = value
            .map(|value| parse_timestamp(&value, "check run started_at"))
            .transpose()?;
    }
    if let SeedField::Present(value) = seed.completed_at {
        check.completed_at = value
            .map(|value| parse_timestamp(&value, "check run completed_at"))
            .transpose()?;
    }
    if let SeedField::Present(pull_requests) = seed.pull_requests {
        check.pull_requests = pull_requests;
    }
    if let SeedField::Present(output) = seed.output {
        apply!(check.output_title, output.title);
        apply!(check.output_summary, output.summary);
        apply!(check.output_text, output.text);
        if let SeedField::Present(count) = output.annotations_count {
            check.seeded.annotations_count = Some(count);
        }
        if let SeedField::Present(url) = output.annotations_url {
            check.seeded.annotations_url = Some(url);
        }
    }
    if let SeedField::Present(suite) = seed.check_suite {
        check.suite_id = suite
            .map(|suite| model_id_signed(field_or(suite.id, 1), "check suite"))
            .transpose()?;
    }
    if let SeedField::Present(app) = seed.app {
        check.app = app
            .map(|app| {
                integration(state, request, app).map(|app| CheckRunApp::Seeded(Box::new(app)))
            })
            .transpose()?;
    }
    Ok(check)
}

/// Completes a commit status from typed sparse fixture fields.
///
/// # Errors
/// Returns an input error for an id out of range, a state other than `error`, `failure`,
/// `pending` or `success`, or a timestamp that is not RFC 3339.
pub(crate) fn seeded_commit_status(
    request: &ApiRequest,
    account: &Account,
    seed: CommitStatusSeed,
    fallback: usize,
) -> io::Result<CommitStatusRecord> {
    let id = match seed.id {
        SeedField::Present(id) => model_id_or_input(id, "commit status")?,
        SeedField::Missing => indexed_model_id(fallback, "commit status")?,
    };
    let mut status = commit_status(id);
    if let SeedField::Present(state) = seed.state {
        status.state = StatusState::parse(&state)
            .ok_or_else(|| invalid_seed(format!("invalid commit status state '{state}'")))?;
    }
    apply!(status.context, seed.context);
    apply!(status.description, seed.description);
    apply!(status.target_url, seed.target_url);
    apply_time(
        seed.created_at,
        &mut status.created_at,
        "commit status created_at",
    )?;
    apply_time(
        seed.updated_at,
        &mut status.updated_at,
        "commit status updated_at",
    )?;
    if let SeedField::Present(creator) = seed.creator {
        status.creator = creator
            .map(|seed| simple_user_from_seed(request, account, seed))
            .transpose()?;
    }
    Ok(status)
}

/// Builds a complete simple user and applies its typed sparse fixture fields.
///
/// # Errors
/// Returns an input error when a seed identifier is outside the generated model's range.
pub(crate) fn simple_user(
    state: &mut State,
    request: &ApiRequest,
    seed: UserSeed,
) -> io::Result<types::SimpleUser> {
    let login = match seed.login.clone() {
        SeedField::Present(login) => login,
        SeedField::Missing => "admin".to_owned(),
    };
    let account = state.add_user(&login);
    simple_user_from_seed(request, account, seed)
}

fn simple_user_from_seed(
    request: &ApiRequest,
    account: &Account,
    seed: UserSeed,
) -> io::Result<types::SimpleUser> {
    let mut user = render::simple_user(request, account);
    apply!(user.avatar_url, seed.avatar_url);
    apply!(user.email, seed.email);
    apply!(user.events_url, seed.events_url);
    apply!(user.followers_url, seed.followers_url);
    apply!(user.following_url, seed.following_url);
    apply!(user.gists_url, seed.gists_url);
    apply!(user.gravatar_id, seed.gravatar_id);
    apply!(user.url, seed.url);
    apply!(user.html_url, seed.html_url);
    if let SeedField::Present(id) = seed.id {
        user.id = model_id_signed(id, "user")?;
    }
    apply!(user.name, seed.name);
    apply!(user.login, seed.login);
    apply!(user.type_, seed.account_type);
    apply!(user.organizations_url, seed.organizations_url);
    apply!(user.received_events_url, seed.received_events_url);
    apply!(user.repos_url, seed.repos_url);
    apply!(user.site_admin, seed.site_admin);
    apply!(user.starred_at, seed.starred_at);
    apply!(user.starred_url, seed.starred_url);
    apply!(user.subscriptions_url, seed.subscriptions_url);
    apply!(user.user_view_type, seed.user_view_type);
    if let SeedField::Present(node_id) = seed.node_id {
        user.node_id = node_id;
    }
    Ok(user)
}

fn integration(
    state: &mut State,
    request: &ApiRequest,
    seed: IntegrationSeed,
) -> io::Result<types::NullableIntegration> {
    let default_owner = state.add_user("admin");
    let owner = render::simple_user(request, default_owner);
    let mut integration = types::NullableIntegration {
        client_id: None,
        created_at: crate::time::now_utc(),
        description: Some(String::new()),
        events: Vec::new(),
        external_url: String::new(),
        html_url: String::new(),
        id: 1,
        installations_count: None,
        name: "App".to_owned(),
        node_id: render::node_id("Integration", 1),
        owner: types::NullableIntegrationOwner::SimpleUser(owner),
        permissions: types::NullableIntegrationPermissions {
            checks: None,
            contents: None,
            deployments: None,
            issues: None,
            metadata: None,
            extra: std::collections::HashMap::new(),
        },
        slug: None,
        updated_at: crate::time::now_utc(),
    };
    if let SeedField::Present(id) = seed.id {
        integration.id = model_id_signed(id, "integration")?;
    }
    apply!(integration.node_id, seed.node_id);
    apply!(integration.slug, seed.slug);
    apply!(integration.name, seed.name);
    apply!(integration.description, seed.description);
    apply!(integration.external_url, seed.external_url);
    apply!(integration.html_url, seed.html_url);
    apply!(integration.events, seed.events);
    apply_time(
        seed.created_at,
        &mut integration.created_at,
        "integration created_at",
    )?;
    apply_time(
        seed.updated_at,
        &mut integration.updated_at,
        "integration updated_at",
    )?;
    if let SeedField::Present(owner_seed) = seed.owner {
        let login = field_or(owner_seed.login.clone(), "admin".to_owned());
        let account = state.add_user(&login);
        integration.owner = types::NullableIntegrationOwner::SimpleUser(simple_user_from_seed(
            request, account, owner_seed,
        )?);
    }
    if let SeedField::Present(permissions) = seed.permissions {
        apply_optional!(integration.permissions.checks, permissions.checks);
        apply_optional!(integration.permissions.contents, permissions.contents);
        apply_optional!(integration.permissions.deployments, permissions.deployments);
        apply_optional!(integration.permissions.issues, permissions.issues);
        apply_optional!(integration.permissions.metadata, permissions.metadata);
    }
    Ok(integration)
}

fn field_or<T>(field: SeedField<T>, default: T) -> T {
    match field {
        SeedField::Missing => default,
        SeedField::Present(value) => value,
    }
}

fn model_id_or_input(id: u64, kind: &str) -> io::Result<i64> {
    i64::try_from(id).map_err(|error| invalid_seed(format!("invalid {kind} id: {error}")))
}

pub(crate) fn model_id_signed(id: i64, kind: &str) -> io::Result<i64> {
    if id < 0 {
        Err(invalid_seed(format!(
            "invalid {kind} id: must be non-negative"
        )))
    } else {
        Ok(id)
    }
}

fn node_id(id: i64) -> u64 {
    u64::try_from(id).expect("seeded model ids are checked before node-id construction")
}

fn indexed_model_id(index: usize, kind: &str) -> io::Result<i64> {
    let id = index
        .checked_add(1)
        .ok_or_else(|| invalid_seed(format!("invalid {kind} index: overflow")))?;
    let id = u64::try_from(id)
        .map_err(|error| invalid_seed(format!("invalid {kind} index: {error}")))?;
    model_id_or_input(id, kind)
}

fn parse_enum<T>(value: String, kind: &str) -> io::Result<T>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    value
        .parse()
        .map_err(|error| invalid_seed(format!("invalid {kind} '{value}': {error}")))
}

pub(crate) fn parse_timestamp(value: &str, kind: &str) -> io::Result<DateTime<Utc>> {
    crate::time::parse(value)
        .map_err(|error| invalid_seed(format!("invalid {kind} timestamp '{value}': {error}")))
}

fn apply_time(seed: SeedField<String>, target: &mut DateTime<Utc>, kind: &str) -> io::Result<()> {
    if let SeedField::Present(value) = seed {
        *target = parse_timestamp(&value, kind)?;
    }
    Ok(())
}

fn invalid_seed(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

/// Returns the SHA used by a workflow run when a seed omits it.
fn default_sha() -> String {
    "0000000000000000000000000000000000000000".to_owned()
}
