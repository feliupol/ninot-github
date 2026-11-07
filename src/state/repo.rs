//! Repositories: their lifecycle, redirects left by renames and transfers, and who may do
//! what with them.

use std::io;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use super::{Hook, Release, State};
use crate::api::{ApiResponse, Caller};
use crate::time::now;
use crate::types::{
    FullRepositoryMergeCommitMessage, FullRepositoryMergeCommitTitle,
    FullRepositoryPullRequestCreationPolicy, FullRepositorySquashMergeCommitMessage,
    FullRepositorySquashMergeCommitTitle,
};

/// A repository, owned by exactly one account.
pub struct Repo {
    pub id: u64,
    pub name: String,
    pub private: bool,
    pub description: Option<String>,
    /// Primary language reported by repository metadata.
    pub language: Option<String>,
    pub default_branch: String,
    pub created_at: String,
    pub updated_at: String,
    pub pushed_at: Option<String>,
    /// Bare git repository, named after the id so renames and transfers leave it in place.
    pub git_dir: Option<PathBuf>,
    /// Upstream repository anchoring this repository's fork network.
    pub fork_source: Option<(String, String)>,
    pub releases: Vec<Release>,
    /// Pull requests by number, with the fields the GitHub REST API serves.
    pub pull_requests: BTreeMap<u64, PullRequest>,
    /// Issues by number.
    pub issues: BTreeMap<u64, Issue>,
    /// Labels defined on the repository, keyed by lowercase name.
    ///
    /// Labels that only exist because an issue or pull request was seeded with them are not
    /// copied here. Reads merge this map with those copies.
    pub labels: BTreeMap<String, crate::types::Label>,
    /// Milestones by their repository-local number.
    pub milestones: BTreeMap<u64, Milestone>,
    /// Workflow runs, ordered as the Actions API lists them.
    pub workflow_runs: Vec<crate::types::WorkflowRun>,
    /// Jobs keyed by workflow run id.
    pub workflow_jobs: BTreeMap<u64, Vec<crate::types::Job>>,
    /// Plain-text workflow job logs, keyed by job id.
    pub workflow_job_logs: BTreeMap<u64, String>,
    /// Public key used by Actions secret clients, if configured by the fixture.
    pub actions_public_key: Option<crate::types::ActionsPublicKey>,
    /// Encrypted Actions secret payloads, keyed by their case-sensitive name.
    pub actions_secrets: BTreeMap<String, StoredActionsSecret>,
    /// Check runs keyed by commit SHA.
    pub check_runs: BTreeMap<String, Vec<crate::types::CheckRun>>,
    /// Check suites by id.
    pub check_suites: BTreeMap<i64, CheckSuiteRecord>,
    /// Annotations of a check run, in the order they were submitted.
    pub check_annotations: BTreeMap<i64, Vec<crate::types::CheckAnnotation>>,
    /// Whether a GitHub App creates a check suite on push. Absent means enabled.
    pub check_suite_preferences: BTreeMap<i64, bool>,
    /// Commit statuses keyed by commit SHA.
    pub commit_statuses: BTreeMap<String, Vec<crate::types::Status>>,
    pub hooks: Vec<Hook>,
    pub settings: Settings,
}

/// A check suite owned by one GitHub App and one commit.
#[derive(Clone)]
pub struct CheckSuiteRecord {
    /// Suite id.
    pub id: i64,
    /// GitHub App that owns the suite.
    pub app_id: i64,
    /// Commit the suite checks.
    pub head_sha: String,
    /// Branch name when the head is the tip of a branch.
    pub head_branch: Option<String>,
    /// Parent of `head_sha`, or forty zeros when the commit has none.
    pub before: String,
    /// `head_sha` as recorded when the suite was created.
    pub after: String,
    /// When the suite was created.
    pub created_at: DateTime<Utc>,
    /// When the suite or one of its runs last changed.
    pub updated_at: DateTime<Utc>,
    /// Rerequest forces queued with no conclusion until a run in the suite changes.
    pub rerequested: bool,
}

/// Encrypted Actions secret data retained by the fake but never returned by REST reads.
#[derive(Clone)]
pub struct StoredActionsSecret {
    /// Secret name.
    pub name: String,
    /// Ciphertext submitted by the client.
    pub encrypted_value: String,
    /// Public-key identifier used to encrypt the ciphertext.
    pub key_id: String,
    /// First creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Most recent update timestamp.
    pub updated_at: DateTime<Utc>,
}

/// A pull request and its associated review/comment collections.
#[derive(Clone, Default)]
pub struct PullRequest {
    /// The REST pull request, absent for GraphQL-only seeded review state.
    pub data: Option<crate::types::PullRequest>,
    /// Reviews returned by `GET /pulls/{number}/reviews`.
    pub reviews: Vec<crate::types::PullRequestReview>,
    /// Issue comments returned by `GET /issues/{number}/comments`.
    pub issue_comments: Vec<crate::types::IssueComment>,
    /// Review comments returned by `GET /pulls/{number}/comments`.
    pub review_comments: Vec<crate::types::PullRequestReviewComment>,
    /// Review threads returned by GraphQL, including their persisted comment history.
    pub review_threads: Vec<ReviewThread>,
    /// Explicit review decision when seeded; otherwise derived from current reviews.
    pub review_decision: Option<String>,
    /// Timestamp used by the GraphQL `autoMergeRequest` projection.
    pub auto_merge_enabled_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Timeline events recording who changed the pull request and when, oldest first.
    /// Reviews, comments and commits are timeline events too, derived from their own lists.
    pub events: Vec<TimelineEvent>,
    /// When each commit first reached the head branch, by sha; orders `committed` events.
    pub pushed_at: BTreeMap<String, DateTime<Utc>>,
    /// Files returned by `GET /pulls/{number}/files`.
    pub files: Vec<crate::types::DiffEntry>,
}

/// What a stored timeline event records, named as the timeline's `event` field names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimelineEventKind {
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
    /// The pull request was closed.
    Closed,
    /// A closed pull request was reopened.
    Reopened,
}

impl TimelineEventKind {
    /// Every kind, for parsing.
    const ALL: [Self; 8] = [
        Self::ReadyForReview,
        Self::ConvertToDraft,
        Self::ReviewRequested,
        Self::ReviewRequestRemoved,
        Self::HeadRefForcePushed,
        Self::Merged,
        Self::Closed,
        Self::Reopened,
    ];

    /// The timeline's `event` value.
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
        }
    }

    /// The kind whose timeline `event` value is `name`.
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.name() == name)
    }
}

/// One stored timeline event of a pull request.
#[derive(Clone, Debug)]
pub struct TimelineEvent {
    /// Event id, unique across the server.
    pub id: u64,
    /// What happened.
    pub kind: TimelineEventKind,
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
}

impl PullRequest {
    /// Applies `event` to the pull request's REST model and appends it to the timeline.
    ///
    /// # Errors
    ///
    /// Refuses, leaving the pull request unchanged, an event the current state does not allow:
    /// readying a non-draft, drafting a closed or draft pull request, closing or merging a closed
    /// one, merging a draft, reopening an open or merged one, a review request naming no reviewer, removing a
    /// request that was not made, or a force-push without its new head.
    pub fn apply_event(&mut self, event: TimelineEvent) -> Result<(), String> {
        let number = self.data.as_ref().map_or(0, |data| data.number);
        let data = self
            .data
            .as_mut()
            .ok_or_else(|| format!("pull request #{number} has no REST model"))?;
        let open = data.state == crate::types::PullRequestState::Open;
        let draft = data.draft.unwrap_or(false);
        match event.kind {
            TimelineEventKind::ReadyForReview if !draft => {
                return Err("only a draft pull request can be marked ready for review".to_owned());
            }
            TimelineEventKind::ReadyForReview => data.draft = Some(false),
            TimelineEventKind::ConvertToDraft if draft || !open => {
                return Err("only an open, non-draft pull request can become a draft".to_owned());
            }
            TimelineEventKind::ConvertToDraft => data.draft = Some(true),
            TimelineEventKind::ReviewRequested | TimelineEventKind::ReviewRequestRemoved => {
                let adding = event.kind == TimelineEventKind::ReviewRequested;
                if let Some(reviewer) = &event.requested_reviewer {
                    let position = data
                        .requested_reviewers
                        .iter()
                        .position(|user| user.login.eq_ignore_ascii_case(&reviewer.login));
                    match (adding, position) {
                        (true, None) => data.requested_reviewers.push(reviewer.clone()),
                        (false, Some(index)) => {
                            data.requested_reviewers.remove(index);
                        }
                        (true, Some(_)) => {}
                        (false, None) => {
                            return Err(format!("{} was not requested to review", reviewer.login));
                        }
                    }
                } else if let Some(team) = &event.requested_team {
                    let position = data
                        .requested_teams
                        .iter()
                        .position(|existing| existing.slug == team.slug);
                    match (adding, position) {
                        (true, None) => data.requested_teams.push(team.clone()),
                        (false, Some(index)) => {
                            data.requested_teams.remove(index);
                        }
                        (true, Some(_)) => {}
                        (false, None) => {
                            return Err(format!("team {} was not requested to review", team.slug));
                        }
                    }
                } else {
                    return Err("a review request names a reviewer or a team".to_owned());
                }
            }
            TimelineEventKind::HeadRefForcePushed => {
                let sha = event
                    .commit_id
                    .clone()
                    .ok_or_else(|| "a force-push records its new head".to_owned())?;
                data.statuses_url = data.statuses_url.replace(&data.head.sha, &sha);
                data.links.statuses.href = data.links.statuses.href.replace(&data.head.sha, &sha);
                data.head.sha = sha;
            }
            TimelineEventKind::Merged | TimelineEventKind::Closed if !open => {
                return Err("the pull request is already closed".to_owned());
            }
            TimelineEventKind::Merged if draft => {
                return Err("a draft pull request cannot be merged".to_owned());
            }
            TimelineEventKind::Merged => {
                data.merged = true;
                data.merged_at = Some(event.created_at);
                data.merged_by = Some(event.actor.clone().into());
                data.merge_commit_sha = event.commit_id.clone();
            }
            TimelineEventKind::Closed => {
                data.state = crate::types::PullRequestState::Closed;
                data.closed_at = Some(event.created_at);
            }
            TimelineEventKind::Reopened if open || data.merged => {
                return Err("only a closed, unmerged pull request can be reopened".to_owned());
            }
            TimelineEventKind::Reopened => {
                data.state = crate::types::PullRequestState::Open;
                data.closed_at = None;
            }
        }
        if data.updated_at < event.created_at {
            data.updated_at = event.created_at;
        }
        self.events.push(event);
        Ok(())
    }
}

/// Persisted review-thread state used by GraphQL reads and mutations.
#[derive(Clone, Debug)]
pub struct ReviewThread {
    /// Stable GraphQL node identifier.
    pub id: String,
    /// Whether the thread is resolved.
    pub is_resolved: bool,
    /// Reviewed file path.
    pub path: Option<String>,
    /// Reviewed file line.
    pub line: Option<i64>,
    /// Comments in chronological order.
    pub comments: Vec<ReviewThreadComment>,
}

/// A comment persisted inside a review thread.
#[derive(Clone, Debug)]
pub struct ReviewThreadComment {
    /// Stable GraphQL node identifier.
    pub id: String,
    /// Comment body.
    pub body: String,
    /// Last update timestamp.
    pub updated_at: String,
}

/// An issue and its comments, as GitHub REST reads them.
#[derive(Clone)]
pub struct Issue {
    /// The GitHub REST issue model.
    pub data: crate::types::Issue,
    /// Comments returned by the issue comments endpoint.
    pub comments: Vec<crate::types::IssueComment>,
}

/// A repository milestone.
///
/// `open_issues` and `closed_issues` are not stored. They are the number of open and closed
/// issues and pull requests whose milestone number is [`Self::number`], computed when a
/// milestone is rendered.
#[derive(Clone)]
pub struct Milestone {
    /// Stable id, unique across the server.
    pub id: u64,
    /// Number of the milestone within its repository, starting at 1.
    pub number: u64,
    /// Title. Unique among the repository's milestones, ignoring case.
    pub title: String,
    /// Description, when one has been set.
    pub description: Option<String>,
    /// `open` or `closed`.
    pub state: crate::types::NullableMilestoneState,
    /// User who created the milestone.
    pub creator: crate::types::NullableSimpleUser,
    /// When the milestone was created.
    pub created_at: DateTime<Utc>,
    /// When the milestone was last updated.
    pub updated_at: DateTime<Utc>,
    /// When the milestone was closed. Absent while it is open.
    pub closed_at: Option<DateTime<Utc>>,
    /// Due date, when one has been set.
    pub due_on: Option<DateTime<Utc>>,
}

impl Repo {
    /// Returns whether this repository is the root or a fork in the given fork network.
    pub(crate) fn belongs_to_fork_network(&self, owner: &str, root: &(String, String)) -> bool {
        (owner.eq_ignore_ascii_case(&root.0) && self.name.eq_ignore_ascii_case(&root.1))
            || self.fork_source.as_ref().is_some_and(|source| {
                source.0.eq_ignore_ascii_case(&root.0) && source.1.eq_ignore_ascii_case(&root.1)
            })
    }

    /// Points `default_branch` and the bare repository's `HEAD` at `branch`, which need not
    /// exist yet; the two always agree.
    ///
    /// # Errors
    ///
    /// Fails when `HEAD` cannot be rewritten, leaving `default_branch` unchanged.
    pub fn set_default_branch(&mut self, branch: &str) -> io::Result<()> {
        if let Some(git_dir) = &self.git_dir {
            ninot_common::git::set_head(git_dir, branch)?;
        }
        self.default_branch = branch.to_owned();
        Ok(())
    }
}

/// Settings `PATCH /repos/{owner}/{repo}` may change, named and rendered as in REST.
#[derive(Clone, Serialize, Deserialize)]
pub struct Settings {
    pub homepage: Option<String>,
    pub has_issues: bool,
    pub has_projects: bool,
    pub has_wiki: bool,
    pub has_pull_requests: bool,
    pub pull_request_creation_policy: FullRepositoryPullRequestCreationPolicy,
    pub is_template: bool,
    pub allow_squash_merge: bool,
    pub allow_merge_commit: bool,
    pub allow_rebase_merge: bool,
    pub allow_auto_merge: bool,
    pub delete_branch_on_merge: bool,
    pub allow_update_branch: bool,
    pub use_squash_pr_title_as_default: bool,
    pub squash_merge_commit_title: FullRepositorySquashMergeCommitTitle,
    pub squash_merge_commit_message: FullRepositorySquashMergeCommitMessage,
    pub merge_commit_title: FullRepositoryMergeCommitTitle,
    pub merge_commit_message: FullRepositoryMergeCommitMessage,
    pub archived: bool,
    pub allow_forking: bool,
    pub web_commit_signoff_required: bool,
}

impl Default for Settings {
    /// What GitHub gives a new repository.
    fn default() -> Self {
        Self {
            homepage: None,
            has_issues: true,
            has_projects: true,
            has_wiki: true,
            has_pull_requests: true,
            pull_request_creation_policy: FullRepositoryPullRequestCreationPolicy::All,
            is_template: false,
            allow_squash_merge: true,
            allow_merge_commit: true,
            allow_rebase_merge: true,
            allow_auto_merge: false,
            delete_branch_on_merge: false,
            allow_update_branch: false,
            use_squash_pr_title_as_default: false,
            squash_merge_commit_title: FullRepositorySquashMergeCommitTitle::CommitOrPrTitle,
            squash_merge_commit_message: FullRepositorySquashMergeCommitMessage::CommitMessages,
            merge_commit_title: FullRepositoryMergeCommitTitle::MergeMessage,
            merge_commit_message: FullRepositoryMergeCommitMessage::PrTitle,
            archived: false,
            allow_forking: true,
            web_commit_signoff_required: false,
        }
    }
}

/// What a caller may do with a repository.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Permission {
    pub admin: bool,
    pub push: bool,
    pub pull: bool,
}

impl Permission {
    pub(crate) const ADMIN: Self = Self {
        admin: true,
        push: true,
        pull: true,
    };
    const READ: Self = Self {
        admin: false,
        push: false,
        pull: true,
    };
}

impl State {
    /// Records a repository, reserving its name; fails when the owner already has one with
    /// that name. Its `git_dir` does not exist yet: the caller creates it with the lock
    /// released and deletes the repository when that fails.
    pub fn create_repo(
        &mut self,
        owner: &str,
        name: &str,
        private: bool,
        description: Option<String>,
    ) -> Result<&mut Repo, ApiResponse> {
        let id = self.next_id();
        let git_dir = self
            .git_root
            .as_ref()
            .map(|root| root.join(format!("{id}.git")));
        let account = self.account_mut(owner).ok_or_else(ApiResponse::not_found)?;
        let key = name.to_lowercase();
        if account.repos.contains_key(&key) {
            return Err(ApiResponse::validation_failed(
                "Repository",
                "name",
                "custom",
            ));
        }
        let created = now();
        account.repos.insert(
            key.clone(),
            Repo {
                id,
                name: name.to_owned(),
                private,
                description,
                language: None,
                default_branch: "main".to_owned(),
                created_at: created.clone(),
                updated_at: created,
                pushed_at: None,
                git_dir,
                fork_source: None,
                releases: Vec::new(),
                pull_requests: BTreeMap::new(),
                issues: BTreeMap::new(),
                labels: BTreeMap::new(),
                milestones: BTreeMap::new(),
                workflow_runs: Vec::new(),
                workflow_jobs: BTreeMap::new(),
                workflow_job_logs: BTreeMap::new(),
                actions_public_key: None,
                actions_secrets: BTreeMap::new(),
                check_runs: BTreeMap::new(),
                check_suites: BTreeMap::new(),
                check_annotations: BTreeMap::new(),
                check_suite_preferences: BTreeMap::new(),
                commit_statuses: BTreeMap::new(),
                hooks: Vec::new(),
                settings: Settings::default(),
            },
        );
        self.redirects
            .remove(&format!("{}/{key}", owner.to_lowercase()));
        Ok(self
            .account_mut(owner)
            .unwrap()
            .repos
            .get_mut(&key)
            .unwrap())
    }

    /// Removes a repository and any redirects pointing at it. Its `git_dir` stays on disk for
    /// the caller to remove with the lock released.
    pub fn delete_repo(&mut self, owner: &str, name: &str) -> Option<Repo> {
        let repo = self
            .account_mut(owner)?
            .repos
            .remove(&name.to_lowercase())?;
        let (owner, name) = (owner.to_lowercase(), name.to_lowercase());
        self.redirects.retain(|_, (to_owner, to_name)| {
            to_owner.to_lowercase() != owner || to_name.to_lowercase() != name
        });
        Some(repo)
    }

    /// Moves a repository to `new_owner` as `new_name`, leaving a redirect behind.
    pub fn move_repo(
        &mut self,
        owner: &str,
        name: &str,
        new_owner: &str,
        new_name: &str,
    ) -> Result<(), ApiResponse> {
        let target = self
            .account(new_owner)
            .ok_or_else(|| ApiResponse::validation_failed("Repository", "new_owner", "invalid"))?;
        let same = owner.eq_ignore_ascii_case(new_owner) && name.eq_ignore_ascii_case(new_name);
        if !same && target.repo(new_name).is_some() {
            return Err(ApiResponse::validation_failed(
                "Repository",
                "name",
                "custom",
            ));
        }
        let mut repo = self
            .account_mut(owner)
            .and_then(|account| account.repos.remove(&name.to_lowercase()))
            .ok_or_else(ApiResponse::not_found)?;
        repo.name = new_name.to_owned();
        repo.updated_at = now();
        let target = self.account_mut(new_owner).unwrap();
        let new_owner = target.login.clone();
        target.repos.insert(new_name.to_lowercase(), repo);
        let old = format!("{}/{}", owner.to_lowercase(), name.to_lowercase());
        for (to_owner, to_name) in self.redirects.values_mut() {
            if format!("{}/{}", to_owner.to_lowercase(), to_name.to_lowercase()) == old {
                *to_owner = new_owner.clone();
                *to_name = new_name.to_owned();
            }
        }
        if !same {
            self.redirects.insert(old, (new_owner, new_name.to_owned()));
        }
        Ok(())
    }

    /// Where a renamed or transferred repository lives now.
    pub fn redirect(&self, owner: &str, name: &str) -> Option<&(String, String)> {
        self.redirects
            .get(&format!("{}/{}", owner.to_lowercase(), name.to_lowercase()))
    }

    /// What `caller` may do with `owner/name`; `None` when the repository is invisible to them.
    pub fn permission(
        &self,
        caller: Option<&Caller>,
        owner: &str,
        name: &str,
    ) -> Option<Permission> {
        let account = self.account(owner)?;
        let repo = account.repo(name)?;
        if let Some(caller) = caller {
            if let Some(installation) = caller
                .scopes
                .iter()
                .find(|scope| scope.starts_with("installation:"))
            {
                let requested = format!("installation:{owner}/{name}");
                if installation.eq_ignore_ascii_case(&requested) {
                    return Some(Permission::ADMIN);
                }
                return (!repo.private).then_some(Permission::READ);
            }
            if account.is_administered_by(&caller.login) {
                return Some(Permission::ADMIN);
            }
        }
        (!repo.private).then_some(Permission::READ)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transfer_leaves_a_redirect_that_follows_later_moves() {
        let mut state = State::default();
        state.add_user("alice");
        state.add_org("acme", &["alice"]);
        state.create_repo("alice", "tool", false, None).unwrap();
        state.move_repo("alice", "tool", "acme", "tool").unwrap();
        state.move_repo("acme", "tool", "acme", "tool2").unwrap();
        assert_eq!(
            state.redirect("alice", "tool"),
            Some(&("acme".to_owned(), "tool2".to_owned()))
        );
        assert_eq!(
            state.redirect("ACME", "Tool"),
            Some(&("acme".to_owned(), "tool2".to_owned()))
        );
        state.create_repo("alice", "tool", false, None).unwrap();
        assert_eq!(state.redirect("alice", "tool"), None);
    }

    #[test]
    fn private_repos_are_invisible_to_outsiders() {
        let mut state = State::default();
        state.add_user("alice");
        state.add_user("bob");
        state.create_repo("alice", "secret", true, None).unwrap();
        let bob = Caller {
            login: "bob".to_owned(),
            scopes: Vec::new(),
        };
        let alice = Caller {
            login: "Alice".to_owned(),
            scopes: Vec::new(),
        };
        assert_eq!(state.permission(Some(&bob), "alice", "secret"), None);
        assert_eq!(state.permission(None, "alice", "secret"), None);
        assert!(
            state
                .permission(Some(&alice), "alice", "secret")
                .unwrap()
                .admin
        );
    }
}
