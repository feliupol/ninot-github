//! Server state shared by every request: accounts own repositories, tokens act as accounts.
//!
//! [`State`] and its account lookups live here; each kind of object has its own submodule.

mod account;
pub(crate) mod actions_service;
mod hook;
mod release;
mod repo;
mod runner;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, PoisonError};

pub use account::{Account, AccountKind, Team};
pub use hook::{Delivery, Hook};
pub use release::{Asset, Release};
pub use repo::{
    CheckSuiteRecord, Issue, Milestone, Permission, PullRequest, Repo, ReviewThread,
    ReviewThreadComment, Settings, StoredActionsSecret, TimelineEvent, TimelineEventKind,
};
pub(crate) use runner::RunnerTokenKind;
pub(crate) use runner::{CreateRunnerError, RunnerSpec, valid_label_name, valid_runner_name};
pub use runner::{Runner, RunnerGroup, RunnerLabel};

use crate::api::Caller;
pub use crate::time::now;

/// Locks `state`, recovering it when a thread panicked while holding the lock, so one failing
/// request leaves later requests and test seeding working.
pub fn lock(state: &Mutex<State>) -> MutexGuard<'_, State> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Everything the fake knows, guarded by one lock taken through [`lock`].
#[derive(Default)]
pub struct State {
    /// Access tokens by their secret value.
    pub tokens: HashMap<String, Caller>,
    /// Repository installations used by GitHub App API-token exchanges.
    pub installations: BTreeMap<u64, Installation>,
    /// Projects v2, keyed by their database id.
    pub projects: BTreeMap<u64, Project>,
    /// Gists created through the Gists API, returned by later list requests.
    pub gists: Vec<crate::types::GistSimple>,
    /// Next gist id, kept separately because GitHub gist ids are strings.
    pub last_gist_id: u64,
    /// One-time registration and removal tokens for self-hosted runners, by secret.
    runner_tokens: HashMap<String, runner::RunnerToken>,
    /// What the Actions runner's service endpoints serve: tokens, sessions and queued jobs.
    pub(crate) actions_service: actions_service::ActionsService,
    /// Users and organizations by lowercase login.
    pub(crate) accounts: BTreeMap<String, Account>,
    /// Old `owner/name` (lowercase) of renamed or transferred repositories, to the current one.
    redirects: HashMap<String, (String, String)>,
    last_id: u64,
    /// Directory holding one bare git repository per repository; without it, nothing is on disk.
    pub git_root: Option<PathBuf>,
    /// Every webhook delivery, oldest first.
    pub deliveries: Vec<Delivery>,
    /// Hands deliveries to the sender thread; without it, deliveries are recorded but not sent.
    pub webhook_queue: Option<std::sync::mpsc::Sender<u64>>,
    /// Checks every webhook payload against its octokit/webhooks schema before delivery.
    pub validate_payloads: bool,
    pub faults: crate::faults::Faults,
    pub(crate) rate_limits: crate::rate_limit::RateLimits,
}

/// A persisted GitHub Projects v2 board and its single-select fields/items.
#[derive(Clone, Default)]
pub struct Project {
    pub id: u64,
    pub owner: String,
    pub repository: String,
    pub number: u64,
    pub title: String,
    pub fields: BTreeMap<u64, ProjectField>,
    pub items: BTreeMap<u64, ProjectItem>,
}

#[derive(Clone, Default)]
pub struct ProjectField {
    pub id: u64,
    pub name: String,
    pub options: BTreeMap<u64, String>,
}

#[derive(Clone, Default)]
pub struct ProjectItem {
    pub id: u64,
    pub owner: String,
    pub repository: String,
    pub issue_number: u64,
    /// Project field database id to selected option database id.
    pub single_select_values: BTreeMap<u64, u64>,
}

/// A GitHub App installation scoped to one repository.
#[derive(Clone)]
pub struct Installation {
    pub id: u64,
    pub owner: String,
    pub repository: String,
    /// GitHub App that owns the installation. The default app is 1.
    pub app_id: u64,
    /// Actions secrets access granted to tokens minted for this installation.
    pub secrets_permission: Option<crate::types::AppPermissionsSecrets>,
}

impl State {
    /// Seeds an App installation for one existing repository.
    pub fn add_installation(
        &mut self,
        owner: &str,
        repository: &str,
        id: u64,
        secrets_permission: Option<crate::types::AppPermissionsSecrets>,
    ) {
        self.insert_installation(owner, repository, id, 1, secrets_permission);
    }

    /// Seeds an installation of `app_id` on one existing repository.
    pub fn add_app_installation(&mut self, owner: &str, repository: &str, id: u64, app_id: u64) {
        self.insert_installation(owner, repository, id, app_id, None);
    }

    fn insert_installation(
        &mut self,
        owner: &str,
        repository: &str,
        id: u64,
        app_id: u64,
        secrets_permission: Option<crate::types::AppPermissionsSecrets>,
    ) {
        assert!(
            self.account(owner)
                .and_then(|account| account.repo(repository))
                .is_some(),
            "no repository {owner}/{repository}"
        );
        self.installations.insert(
            id,
            Installation {
                id,
                owner: owner.to_owned(),
                repository: repository.to_owned(),
                app_id,
                secrets_permission,
            },
        );
    }

    /// A new id, unique across every object of the fake.
    pub fn next_id(&mut self) -> u64 {
        self.last_id += 1;
        self.last_id
    }

    /// Adds a user; an existing login is returned unchanged.
    pub fn add_user(&mut self, login: &str) -> &mut Account {
        self.add_account(login, AccountKind::User)
    }

    /// Sets the display name of an existing account.
    pub fn set_account_name(&mut self, login: &str, name: Option<String>) {
        if let Some(account) = self.account_mut(login) {
            account.name = name;
        }
    }

    /// Adds an organization whose members administer its repositories.
    pub fn add_org(&mut self, login: &str, members: &[&str]) -> &mut Account {
        let org = self.add_account(login, AccountKind::Organization);
        org.members
            .extend(members.iter().map(|member| member.to_lowercase()));
        org
    }

    /// Adds `members` to the team `slug` of the organization `org`, creating both if needed.
    ///
    /// # Panics
    /// When `org` is a user.
    pub fn add_team(&mut self, org: &str, slug: &str, members: &[&str]) -> &mut Team {
        let id = self.next_id();
        let org = self.add_account(org, AccountKind::Organization);
        assert_eq!(
            org.kind,
            AccountKind::Organization,
            "{} is not an organization",
            org.login
        );
        let team = org
            .teams
            .entry(slug.to_lowercase())
            .or_insert_with(|| Team {
                id,
                slug: slug.to_lowercase(),
                members: BTreeSet::new(),
            });
        team.members
            .extend(members.iter().map(|member| member.to_lowercase()));
        team
    }

    fn add_account(&mut self, login: &str, kind: AccountKind) -> &mut Account {
        let key = login.to_lowercase();
        if !self.accounts.contains_key(&key) {
            let id = self.next_id();
            self.accounts.insert(
                key.clone(),
                Account {
                    id,
                    login: login.to_owned(),
                    name: None,
                    kind,
                    created_at: now(),
                    members: BTreeSet::new(),
                    teams: BTreeMap::new(),
                    repos: BTreeMap::new(),
                    runners: BTreeMap::new(),
                    runner_groups: BTreeMap::new(),
                    runner_label_ids: BTreeMap::new(),
                },
            );
        }
        self.accounts.get_mut(&key).unwrap()
    }

    /// Issues a token acting as `login`, which must be an existing user.
    pub fn add_token(&mut self, login: &str, scopes: &[&str]) -> String {
        let account = self
            .account(login)
            .unwrap_or_else(|| panic!("no user {login}"));
        assert_eq!(account.kind, AccountKind::User, "{login} is not a user");
        let token = ninot_common::token::new_token(crate::server::TOKEN_PREFIX);
        self.tokens.insert(
            token.clone(),
            Caller {
                login: account.login.clone(),
                scopes: scopes.iter().map(|scope| (*scope).to_owned()).collect(),
            },
        );
        token
    }

    pub fn account(&self, login: &str) -> Option<&Account> {
        self.accounts.get(&login.to_lowercase())
    }

    pub fn account_mut(&mut self, login: &str) -> Option<&mut Account> {
        self.accounts.get_mut(&login.to_lowercase())
    }

    pub fn accounts(&self) -> impl Iterator<Item = &Account> {
        self.accounts.values()
    }

    /// Seed one pull request as the GitHub REST API will return it.
    pub fn add_pull_request(
        &mut self,
        owner: &str,
        repo: &str,
        number: u64,
        data: crate::types::PullRequest,
    ) -> &mut PullRequest {
        let pull = self.pull_request_entry(owner, repo, number);
        if pull.data.is_none() {
            pull.data = Some(data);
        }
        pull
    }

    fn pull_request_entry(&mut self, owner: &str, repo: &str, number: u64) -> &mut PullRequest {
        self.account_mut(owner)
            .and_then(|account| account.repo_mut(repo))
            .unwrap_or_else(|| panic!("no repository {owner}/{repo}"))
            .pull_requests
            .entry(number)
            .or_default()
    }

    /// Sets whether GraphQL reports auto-merge as available for a repository.
    pub fn set_auto_merge_allowed(&mut self, owner: &str, repo: &str, allowed: bool) {
        let repository = self
            .account_mut(owner)
            .and_then(|account| account.repo_mut(repo))
            .unwrap_or_else(|| panic!("no repository {owner}/{repo}"));
        repository.settings.allow_auto_merge = allowed;
    }

    /// Append a review to a seeded pull request.
    pub fn add_review(
        &mut self,
        owner: &str,
        repo: &str,
        number: u64,
        review: crate::types::PullRequestReview,
    ) {
        let pull = self.pull_request_entry(owner, repo, number);
        if let (Some(data), Some(submitted_at)) = (&mut pull.data, review.submitted_at) {
            data.updated_at = data.updated_at.max(submitted_at);
        }
        pull.reviews.push(review);
    }

    /// Sets an explicit GraphQL review decision for a pull request.
    pub fn set_review_decision(
        &mut self,
        owner: &str,
        repo: &str,
        number: u64,
        decision: Option<String>,
    ) {
        self.pull_request_entry(owner, repo, number).review_decision = decision;
    }

    /// Adds a persisted review thread to a pull request.
    pub fn add_review_thread(
        &mut self,
        owner: &str,
        repo: &str,
        number: u64,
        thread: ReviewThread,
    ) {
        self.pull_request_entry(owner, repo, number)
            .review_threads
            .push(thread);
    }

    /// Adds a Projects v2 board with a stable database id.
    pub fn add_project(&mut self, owner: &str, repository: &str, title: &str) -> &mut Project {
        let id = self.next_id();
        self.projects.entry(id).or_insert_with(|| Project {
            id,
            owner: owner.to_owned(),
            repository: repository.to_owned(),
            number: id,
            title: title.to_owned(),
            ..Project::default()
        })
    }

    /// Adds a single-select field and its option ids to a Projects v2 board.
    pub fn add_project_field(
        &mut self,
        project_id: u64,
        name: &str,
        options: impl IntoIterator<Item = String>,
    ) -> u64 {
        let id = self.next_id();
        let option_map = options
            .into_iter()
            .map(|name| (self.next_id(), name))
            .collect();
        let project = self
            .projects
            .get_mut(&project_id)
            .unwrap_or_else(|| panic!("no project {project_id}"));
        project.fields.insert(
            id,
            ProjectField {
                id,
                name: name.to_owned(),
                options: option_map,
            },
        );
        id
    }

    /// Adds an issue-backed item to a Projects v2 board.
    pub fn add_project_item(
        &mut self,
        project_id: u64,
        owner: &str,
        repository: &str,
        issue_number: u64,
        single_select_values: BTreeMap<String, String>,
    ) -> u64 {
        let id = self.next_id();
        let project = self
            .projects
            .get_mut(&project_id)
            .unwrap_or_else(|| panic!("no project {project_id}"));
        let values = single_select_values
            .into_iter()
            .filter_map(|(field_name, option_name)| {
                let field = project
                    .fields
                    .values()
                    .find(|field| field.name == field_name)?;
                let option = field
                    .options
                    .iter()
                    .find_map(|(id, name)| (name == &option_name).then_some(*id))?;
                Some((field.id, option))
            })
            .collect();
        project.items.insert(
            id,
            ProjectItem {
                id,
                owner: owner.to_owned(),
                repository: repository.to_owned(),
                issue_number,
                single_select_values: values,
            },
        );
        id
    }

    /// Append an issue or review comment to a seeded pull request.
    pub fn add_issue_comment(
        &mut self,
        owner: &str,
        repo: &str,
        number: u64,
        comment: crate::types::IssueComment,
    ) {
        let repository = self
            .account_mut(owner)
            .and_then(|account| account.repo_mut(repo))
            .unwrap_or_else(|| panic!("no repository {owner}/{repo}"));
        if let Some(issue) = repository.issues.get_mut(&number) {
            issue.comments.push(comment);
            issue.data.comments = issue.comments.len() as i64;
        } else {
            let pull = repository
                .pull_requests
                .get_mut(&number)
                .unwrap_or_else(|| panic!("no issue or pull request {owner}/{repo}#{number}"));
            let created_at = comment.created_at;
            pull.issue_comments.push(comment);
            if let Some(data) = &mut pull.data {
                data.comments = pull.issue_comments.len() as i64;
                data.updated_at = data.updated_at.max(created_at);
            }
        }
    }

    /// Append a review comment to a pull request.
    pub fn add_review_comment(
        &mut self,
        owner: &str,
        repo: &str,
        number: u64,
        comment: crate::types::PullRequestReviewComment,
    ) {
        self.pull_request_entry(owner, repo, number)
            .review_comments
            .push(comment);
    }

    /// Sets repository metadata directly for fixture setup.
    pub fn set_repo_metadata(
        &mut self,
        owner: &str,
        name: &str,
        description: Option<String>,
        language: Option<String>,
        default_branch: &str,
    ) -> std::io::Result<()> {
        let repo = self
            .account_mut(owner)
            .and_then(|account| account.repo_mut(name))
            .ok_or_else(|| std::io::Error::other(format!("no repository {owner}/{name}")))?;
        repo.description = description;
        repo.language = language;
        repo.set_default_branch(default_branch)
    }

    /// Adds a workflow run and the jobs it describes to a repository.
    pub fn add_workflow_run(
        &mut self,
        owner: &str,
        name: &str,
        run: crate::types::WorkflowRun,
        jobs: Vec<crate::types::Job>,
    ) {
        let repo = self
            .account_mut(owner)
            .and_then(|account| account.repo_mut(name))
            .unwrap_or_else(|| panic!("no repository {owner}/{name}"));
        let id = u64::try_from(run.id).expect("workflow run id is non-negative");
        repo.workflow_runs.push(run);
        repo.workflow_jobs.insert(id, jobs);
    }

    /// Adds a check run to a commit SHA.
    pub fn add_check_run(
        &mut self,
        owner: &str,
        name: &str,
        sha: &str,
        check: crate::types::CheckRun,
    ) {
        let repo = self
            .account_mut(owner)
            .and_then(|account| account.repo_mut(name))
            .unwrap_or_else(|| panic!("no repository {owner}/{name}"));
        repo.check_runs
            .entry(sha.to_owned())
            .or_default()
            .push(check);
    }

    /// Adds a commit status to a commit SHA.
    pub fn add_commit_status(
        &mut self,
        owner: &str,
        name: &str,
        sha: &str,
        status: crate::types::Status,
    ) {
        let repo = self
            .account_mut(owner)
            .and_then(|account| account.repo_mut(name))
            .unwrap_or_else(|| panic!("no repository {owner}/{name}"));
        repo.commit_statuses
            .entry(sha.to_owned())
            .or_default()
            .push(status);
    }

    /// Adds an issue to an existing repository.
    pub fn add_issue(&mut self, owner: &str, name: &str, number: u64, data: crate::types::Issue) {
        let repo = self
            .account_mut(owner)
            .and_then(|account| account.repo_mut(name))
            .unwrap_or_else(|| panic!("no repository {owner}/{name}"));
        repo.issues.insert(
            number,
            Issue {
                data,
                comments: Vec::new(),
            },
        );
    }
}
