//! The HTTP server and its lifecycle.

use std::collections::BTreeSet;
use std::io::{self, Cursor, Write};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use axum::body::{Body as AxumBody, to_bytes};
use axum::extract::{ConnectInfo, Request, State as AxumState};
use axum::response::Response;
use headers::HeaderMapExt;
use headers::authorization::{Authorization, Basic, Bearer, Credentials};
use http::{HeaderMap, HeaderValue, StatusCode, request::Parts};
use ninot_common::git;
use ninot_common::profile::{Label, Profile, Profiler};
use ninot_common::tls::CA_FILE;
use ninot_common::token;

use crate::api::{ApiRequest, ApiResponse, Body, Caller};
use crate::faults::{Applied, Fault, RequestInfo, Rule};
use crate::handlers;
use crate::operation::{Method, OPERATIONS};
use crate::rate_limit::{Client, Limits, RateLimits, Refusal, Resource};
use crate::router::Router;
use crate::state::{Delivery, State, lock};
use crate::types;
use crate::validate;
use crate::webhooks;

/// Path prefix of a fake's tokens, which GitHub spells `ghp_`.
pub(crate) const TOKEN_PREFIX: &str = "ghp_";

/// Path prefix GitHub Enterprise clients, such as `gh`, put before every API path.
pub(crate) const ENTERPRISE_PREFIX: &str = "/api/v3";

/// Path prefix GitHub Enterprise uses for uploads, such as release assets.
pub(crate) const UPLOADS_PREFIX: &str = "/api/uploads";

/// Login of the user that owns the admin token.
pub(crate) const ADMIN_LOGIN: &str = "admin";

/// Scopes granted to the admin token.
pub(crate) const ADMIN_SCOPES: [&str; 5] = [
    "repo",
    "delete_repo",
    "admin:org",
    "admin:repo_hook",
    "workflow",
];

/// Routes and state shared by request handlers.
pub(crate) struct App {
    router: Router,
    pub state: Arc<Mutex<State>>,
    running: AtomicBool,
    profiler: Profiler,
    hits: Arc<Mutex<BTreeSet<Hit>>>,
    validate_responses: bool,
    /// `http` or `https`, for the URLs the fake hands out.
    scheme: &'static str,
}

/// Something the fake served, as listed by [`Server::hits`].
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Hit {
    /// A REST operation by `operationId`, with the response status.
    Rest {
        operation: &'static str,
        status: u16,
    },
    /// A GraphQL field, as `Type.field`, that resolved without error.
    GraphQl { field: String },
    /// A git smart HTTP service path, such as `git-upload-pack`, with the response status.
    Git { service: String, status: u16 },
    /// A request that matched no operation of the spec.
    Unrouted { method: String, path: String },
    /// A GraphQL request rejected before execution, such as by validation, with the
    /// `Type.field` the error names, if any.
    GraphQlError {
        field: Option<String>,
        message: String,
    },
    /// A request whose handler panicked, answered with a `500`.
    Panic { path: String },
}

impl std::fmt::Display for Hit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rest { operation, status } => write!(f, "rest {operation} {status}"),
            Self::GraphQl { field } => write!(f, "graphql {field}"),
            Self::Git { service, status } => write!(f, "git {service} {status}"),
            Self::Unrouted { method, path } => write!(f, "unrouted {method} {path}"),
            Self::GraphQlError {
                field: Some(field),
                message,
            } => write!(f, "graphql-error {field}: {message}"),
            Self::GraphQlError {
                field: None,
                message,
            } => write!(f, "graphql-error {message}"),
            Self::Panic { path } => write!(f, "panic {path}"),
        }
    }
}

/// How to start a fake.
#[derive(Clone, Debug)]
pub struct Options {
    pub address: SocketAddr,
    /// Checks every successful JSON response against the pinned spec and turns a mismatch into
    /// a `500` naming it. Checks every webhook payload too, and records a mismatch as the
    /// delivery's error instead of sending it.
    pub validate_responses: bool,
    /// Serves HTTPS with a certificate from a fresh local certificate authority, whose
    /// certificate [`Server::ca_cert_path`] points at.
    pub tls: bool,
    pub rate_limits: Limits,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            address: SocketAddr::from(([127, 0, 0, 1], 0)),
            validate_responses: true,
            tls: false,
            rate_limits: Limits::default(),
        }
    }
}

/// A running fake, stopped when dropped.
pub struct Server {
    admin_token: String,
    app: Arc<App>,
    harness: ninot_common::server::Server,
    data_dir: PathBuf,
}

/// A repository fixture handle. Repository-owned API state is seeded through this value.
#[derive(Clone)]
pub struct Repository {
    app: Arc<App>,
    owner: String,
    name: String,
    server_url: String,
}

/// An issue fixture handle, retaining its owning repository.
#[derive(Clone)]
pub struct Issue {
    repository: Repository,
    number: u64,
}

/// A pull-request fixture handle, retaining its owning repository.
#[derive(Clone)]
pub struct PullRequest {
    repository: Repository,
    number: u64,
}

/// A pull-request review owned by its pull request.
#[derive(Clone)]
pub struct Review {
    pull_request: PullRequest,
    id: i64,
}

/// An Actions workflow-run fixture handle.
#[derive(Clone)]
pub struct WorkflowRun {
    repository: Repository,
    id: u64,
}

/// An Actions workflow-job fixture handle.
#[derive(Clone)]
pub struct WorkflowJob {
    run_id: u64,
    id: u64,
}

/// An encrypted Actions secret fixture owned by a repository.
#[derive(Clone)]
pub struct ActionsSecret {
    repository: Repository,
    name: String,
}

/// A commit fixture handle, used to seed associated checks and statuses.
#[derive(Clone)]
pub struct Commit {
    repository: Repository,
    sha: String,
}

/// A Projects v2 board fixture handle.
#[derive(Clone)]
pub struct Project {
    app: Arc<App>,
    node_id: String,
}

/// A user fixture handle. User-owned API state is seeded through this value.
#[derive(Clone)]
pub struct User {
    app: Arc<App>,
    login: String,
}

/// An organization fixture handle. Organization-owned API state is seeded through this value.
#[derive(Clone)]
pub struct Org {
    app: Arc<App>,
    login: String,
}

/// A team fixture handle, retaining its owning organization.
#[derive(Clone)]
pub struct Team {
    org: Org,
    slug: String,
}

/// A self-hosted Actions runner fixture handle.
#[derive(Clone)]
pub struct Runner {
    org: Org,
    id: u64,
}

impl Server {
    /// Starts a fake on a free local port, validating its responses against the spec.
    pub fn start() -> io::Result<Self> {
        Self::start_with(Options::default())
    }

    pub fn start_with(options: Options) -> io::Result<Self> {
        let listener = ninot_common::server::Server::bind(options.address)?;
        let scheme = if options.tls { "https" } else { "http" };
        let admin_token = token::new_token(TOKEN_PREFIX);
        let data_dir_name = token::new_token(TOKEN_PREFIX);
        let data_dir = std::env::temp_dir().join(format!("ninot-github-{}", &data_dir_name[4..]));
        std::fs::create_dir_all(&data_dir)?;
        let tls = if options.tls {
            let tls =
                ninot_common::tls::generate("ninot-github local CA").map_err(io::Error::other)?;
            std::fs::write(data_dir.join(CA_FILE), &tls.ca_pem)?;
            Some(tls.config)
        } else {
            None
        };
        let mut state = State::default();
        state.git_root = Some(data_dir.clone());
        state.rate_limits = RateLimits::new(options.rate_limits);
        state.validate_payloads = options.validate_responses;
        state.add_user(ADMIN_LOGIN);
        state.tokens.insert(
            admin_token.clone(),
            Caller {
                login: ADMIN_LOGIN.to_owned(),
                scopes: ADMIN_SCOPES
                    .iter()
                    .map(|scope| (*scope).to_owned())
                    .collect(),
            },
        );
        let app = Arc::new(App {
            router: Router::new(OPERATIONS),
            state: Arc::new(Mutex::new(state)),
            running: AtomicBool::new(true),
            profiler: Profiler::new(),
            hits: Arc::default(),
            validate_responses: options.validate_responses,
            scheme,
        });
        let queue = webhooks::spawn_sender(Arc::downgrade(&app.state));
        lock(&app.state).webhook_queue = Some(queue);
        let router = axum::Router::new()
            .fallback(dispatch)
            .with_state(app.clone());
        let router = app.profiler.layer(router);
        let harness = match tls {
            None => ninot_common::server::Server::serve(listener, router)?,
            Some(config) => ninot_common::server::Server::serve_tls(listener, router, config)?,
        };
        Ok(Self {
            admin_token,
            app,
            harness,
            data_dir,
        })
    }

    /// Base URL, such as `http://127.0.0.1:1234`. The API is served here and under `/api/v3`.
    pub fn url(&self) -> &str {
        self.harness.url()
    }

    /// PEM certificate of the local certificate authority when serving HTTPS. Point clients at
    /// it, for example with `SSL_CERT_FILE` or git's `http.sslCAInfo`.
    pub fn ca_cert_path(&self) -> Option<PathBuf> {
        let path = self.data_dir.join(CA_FILE);
        path.exists().then_some(path)
    }

    /// Token of the `admin` user, which holds every scope.
    pub fn admin_token(&self) -> &str {
        &self.admin_token
    }

    /// Starts recording every request over HTTP, clearing earlier profile hits.
    pub fn start_profile(&self) {
        self.app.profiler.start();
    }

    /// Stops recording and returns all observed HTTP requests.
    pub fn stop_profile(&self) -> Profile {
        self.app.profiler.stop()
    }

    /// Whether this server is recording requests.
    pub fn is_recording(&self) -> bool {
        self.app.profiler.is_recording()
    }

    /// Adds a user and returns a handle for seeding its fixture state.
    pub fn add_user(&self, login: &str) -> User {
        lock(&self.app.state).add_user(login);
        User {
            app: self.app.clone(),
            login: login.to_owned(),
        }
    }

    /// Returns a handle for an existing user, or `None` when it is absent.
    pub fn user(&self, login: &str) -> Option<User> {
        let state = lock(&self.app.state);
        if state.account(login).is_some() {
            Some(User {
                app: self.app.clone(),
                login: login.to_owned(),
            })
        } else {
            None
        }
    }

    /// Adds an organization whose members administer its repositories, and returns a handle.
    pub fn add_org(&self, login: &str, members: &[&str]) -> Org {
        lock(&self.app.state).add_org(login, members);
        Org {
            app: self.app.clone(),
            login: login.to_owned(),
        }
    }

    /// Returns a handle for an existing organization, or `None` when it is absent.
    pub fn org(&self, login: &str) -> Option<Org> {
        let state = lock(&self.app.state);
        if state
            .account(login)
            .is_some_and(|account| account.kind == crate::state::AccountKind::Organization)
        {
            Some(Org {
                app: self.app.clone(),
                login: login.to_owned(),
            })
        } else {
            None
        }
    }

    /// Creates a repository and returns the handle that owns its fixture operations.
    pub fn create_repository(
        &self,
        owner: &str,
        name: &str,
        private: bool,
        description: Option<String>,
        language: Option<String>,
        default_branch: &str,
    ) -> io::Result<Repository> {
        let storage = {
            let mut state = lock(&self.app.state);
            let repo = state
                .create_repo(owner, name, private, description.clone())
                .map_err(|response| {
                    io::Error::other(format!("cannot add repository: {}", response.status))
                })?;
            repo.git_dir
                .clone()
                .map(|directory| (directory, repo.default_branch.clone()))
        };
        if let Some((directory, branch)) = storage {
            handlers::repos::init_storage(
                &self.app.state,
                owner,
                name,
                &directory,
                &branch,
                false,
            )?;
        }
        lock(&self.app.state).set_repo_metadata(
            owner,
            name,
            description,
            language,
            default_branch,
        )?;
        Ok(Repository {
            app: self.app.clone(),
            owner: owner.to_owned(),
            name: name.to_owned(),
            server_url: self.url().to_owned(),
        })
    }

    /// Returns a handle for an existing repository.
    pub fn repository(&self, owner: &str, name: &str) -> Option<Repository> {
        let (owner, name) = {
            let state = lock(&self.app.state);
            let account = state.account(owner)?;
            let repository = account.repo(name)?;
            (account.login.clone(), repository.name.clone())
        };
        Some(Repository {
            app: self.app.clone(),
            owner,
            name,
            server_url: self.url().to_owned(),
        })
    }

    /// Returns a handle for a runner in an organization.
    pub fn runner(&self, org: &str, id: u64) -> Option<Runner> {
        let state = lock(&self.app.state);
        if state.account(org).is_some_and(|account| {
            account.kind == crate::state::AccountKind::Organization
                && account.runners.contains_key(&id)
        }) {
            Some(Runner {
                org: Org {
                    app: self.app.clone(),
                    login: org.to_owned(),
                },
                id,
            })
        } else {
            None
        }
    }

    /// Installs a fault rule, evaluated after the rules already installed.
    pub fn add_fault(&self, rule: Rule) {
        lock(&self.app.state).faults.add(rule);
    }

    /// Removes every fault rule and forgets the applied faults.
    pub fn clear_faults(&self) {
        lock(&self.app.state).faults.clear();
    }

    /// Restarts the random sequence that decides which requests probabilistic rules hit.
    pub fn seed_faults(&self, seed: u64) {
        lock(&self.app.state).faults.seed(seed);
    }

    /// Every fault applied so far, oldest first.
    pub fn applied_faults(&self) -> Vec<Applied> {
        lock(&self.app.state).faults.applied.clone()
    }

    /// Moves the rate limits' clock forward, expiring windows without waiting.
    pub fn advance_clock(&self, by: std::time::Duration) {
        lock(&self.app.state).rate_limits.advance(by);
    }

    /// Every webhook delivery so far, oldest first.
    pub fn deliveries(&self) -> Vec<Delivery> {
        lock(&self.app.state).deliveries.clone()
    }

    /// Waits until at least `count` deliveries have finished and returns all deliveries, or
    /// panics after `timeout`.
    pub fn wait_for_deliveries(&self, count: usize, timeout: std::time::Duration) -> Vec<Delivery> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let deliveries = self.deliveries();
            let finished = deliveries
                .iter()
                .filter(|delivery| delivery.finished())
                .count();
            if finished >= count {
                return deliveries;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "only {finished} of {count} webhook deliveries finished within {timeout:?}"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    /// Everything served so far, deduplicated.
    pub fn hits(&self) -> Vec<Hit> {
        self.app.hits.lock().unwrap().iter().cloned().collect()
    }

    /// Blocks until the server stops.
    pub fn wait(mut self) {
        self.harness.wait();
    }
}

impl User {
    /// User login.
    pub fn login(&self) -> &str {
        &self.login
    }

    /// Issues a token acting as this user.
    pub fn add_token(&self, scopes: &[&str]) -> String {
        lock(&self.app.state).add_token(&self.login, scopes)
    }

    /// Sets the display name returned by public user endpoints.
    pub fn set_name(&self, name: Option<String>) {
        lock(&self.app.state).set_account_name(&self.login, name);
    }
}

impl Org {
    /// Organization login.
    pub fn login(&self) -> &str {
        &self.login
    }

    /// Adds `members` to the team `slug` of this organization, creating both if needed.
    ///
    /// # Errors
    /// When the organization key conflicts with a user login.
    pub fn add_team(&self, slug: &str, members: &[&str]) -> io::Result<Team> {
        let mut state = lock(&self.app.state);
        if state
            .account(&self.login)
            .is_some_and(|account| account.kind != crate::state::AccountKind::Organization)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{} is a user, not an organization", self.login),
            ));
        }
        state.add_team(&self.login, slug, members);
        Ok(Team {
            org: self.clone(),
            slug: slug.to_owned(),
        })
    }

    /// Registers a self-hosted runner as the Actions runner would, using a one-time registration
    /// token from `POST /orgs/{org}/actions/runners/registration-token`.
    ///
    /// The runner comes online in the organization's default group. Reusing `registration_token`
    /// fails, and a failed attempt leaves the token unused.
    pub fn register_runner(
        &self,
        registration_token: &str,
        name: &str,
        labels: &[&str],
        os: &str,
    ) -> Result<Runner, &'static str> {
        let id = lock(&self.app.state).register_runner(
            &self.login,
            registration_token,
            name,
            labels,
            os,
        )?;
        Ok(Runner {
            org: self.clone(),
            id,
        })
    }
}

impl Team {
    /// Team slug.
    pub fn slug(&self) -> &str {
        &self.slug
    }

    /// Owning organization.
    pub fn org(&self) -> &Org {
        &self.org
    }
}

impl Runner {
    /// Runner id.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Marks a runner offline, as if it had disconnected.
    pub fn set_offline(&self) -> Result<(), &'static str> {
        lock(&self.org.app.state).set_runner_offline(&self.org.login, self.id)
    }
}

impl Repository {
    fn ensure_running(&self) -> io::Result<()> {
        ensure_app_running(&self.app)
    }

    /// Repository owner login.
    pub fn owner(&self) -> &str {
        &self.owner
    }

    /// Repository name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns a handle for an existing pull request in this repository.
    pub fn pull_request(&self, number: u64) -> io::Result<Option<PullRequest>> {
        self.ensure_running()?;
        let state = lock(&self.app.state);
        let pull = state
            .account(&self.owner)
            .and_then(|account| account.repo(&self.name))
            .and_then(|repository| repository.pull_requests.get(&number));
        if pull.is_none() {
            return Ok(None);
        }
        Ok(Some(PullRequest {
            repository: self.clone(),
            number,
        }))
    }

    /// Returns a handle for an existing issue in this repository.
    pub fn issue(&self, number: u64) -> io::Result<Option<Issue>> {
        self.ensure_running()?;
        let state = lock(&self.app.state);
        let Some(repository) = state
            .account(&self.owner)
            .and_then(|account| account.repo(&self.name))
        else {
            return Ok(None);
        };
        Ok(repository.issues.contains_key(&number).then(|| Issue {
            repository: self.clone(),
            number,
        }))
    }

    fn api_request(&self) -> ApiRequest {
        ApiRequest::for_urls(&self.server_url, None)
    }

    /// Returns the fixture handle for a commit SHA in this repository.
    pub fn commit(&self, sha: &str) -> io::Result<Commit> {
        self.ensure_running()?;
        Ok(Commit {
            repository: self.clone(),
            sha: sha.to_owned(),
        })
    }

    /// Sets whether GraphQL reports auto-merge as available for this repository.
    pub fn set_auto_merge_allowed(&self, allowed: bool) -> io::Result<()> {
        self.ensure_running()?;
        let mut state = lock(&self.app.state);
        require_repository(&state, &self.owner, &self.name)?;
        state.set_auto_merge_allowed(&self.owner, &self.name, allowed);
        Ok(())
    }

    /// Commits fixture bytes to a real Git branch and returns its commit handle.
    pub fn commit_file(
        &self,
        branch: &str,
        path: &std::path::Path,
        content: &[u8],
        message: &str,
    ) -> io::Result<Commit> {
        self.commit_file_by(branch, path, content, message, None, None)
    }

    /// Commits fixture bytes to a real Git branch as `author` and `committer`, and returns its
    /// commit handle. A missing committer is the author; a missing author is the fake itself,
    /// dated 2000-01-01.
    ///
    /// Open pull requests whose head is `branch` move to the new commit, which reaches the
    /// branch at its committer date, or now when no identity is given.
    pub fn commit_file_by(
        &self,
        branch: &str,
        path: &std::path::Path,
        content: &[u8],
        message: &str,
        author: Option<&crate::fixtures::GitIdentitySeed>,
        committer: Option<&crate::fixtures::GitIdentitySeed>,
    ) -> io::Result<Commit> {
        self.ensure_running()?;
        let committer = committer.or(author);
        for identity in [author, committer].into_iter().flatten() {
            crate::fixtures::parse_timestamp(&identity.date, "commit date")?;
        }
        let request = self.api_request();
        let mut state = lock(&self.app.state);
        let (sha, owner) = {
            let account = state
                .account(&self.owner)
                .ok_or_else(|| not_found("repository does not exist"))?;
            let repo = account
                .repo(&self.name)
                .ok_or_else(|| not_found("repository does not exist"))?;
            let git_dir = repo
                .git_dir
                .as_ref()
                .ok_or_else(|| io::Error::other("repository has no Git storage"))?;
            let sha = match author {
                Some(author) => crate::fixture_git::commit(crate::fixture_git::Commit {
                    git_dir,
                    branch,
                    default_branch: &repo.default_branch,
                    path,
                    bytes: content,
                    message,
                    author: &git_identity(author),
                    committer: &git_identity(committer.unwrap_or(author)),
                })?,
                None => crate::fixture_git::commit_file(
                    git_dir,
                    branch,
                    &repo.default_branch,
                    path,
                    content,
                    message,
                )?,
            };
            (sha, crate::render::simple_user(&request, account))
        };
        let reached = match committer {
            Some(committer) => crate::fixtures::parse_timestamp(&committer.date, "commit date")?,
            None => crate::fixtures::current_timestamp(),
        };
        crate::handlers::timeline::branch_moved(
            &mut state,
            &self.owner,
            &self.name,
            branch,
            &sha,
            reached,
            &owner,
        )?;
        Ok(Commit {
            repository: self.clone(),
            sha,
        })
    }

    /// Copies an upstream repository's refs and objects into this repository as a fork.
    pub fn fork_from(&self, upstream: &Repository) -> io::Result<()> {
        self.ensure_running()?;
        upstream.ensure_running()?;
        if !Arc::ptr_eq(&self.app, &upstream.app) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "fork source must belong to the same server",
            ));
        }
        let (target, source) = {
            let state = lock(&self.app.state);
            let target = state
                .account(&self.owner)
                .and_then(|account| account.repo(&self.name))
                .and_then(|repo| repo.git_dir.clone())
                .ok_or_else(|| io::Error::other("fork repository does not exist"))?;
            let source = state
                .account(&upstream.owner)
                .and_then(|account| account.repo(&upstream.name))
                .and_then(|repo| repo.git_dir.clone())
                .ok_or_else(|| io::Error::other("upstream repository does not exist"))?;
            (target, source)
        };
        crate::fixture_git::copy_refs(&target, &source)?;
        let mut state = lock(&self.app.state);
        let repo = state
            .account_mut(&self.owner)
            .and_then(|account| account.repo_mut(&self.name))
            .ok_or_else(|| io::Error::other("fork repository does not exist"))?;
        repo.fork_source = Some((upstream.owner.clone(), upstream.name.clone()));
        Ok(())
    }

    /// Seeds a GitHub App installation associated with this repository.
    pub fn create_installation(&self, installation_id: u64) -> io::Result<()> {
        self.create_installation_with_secrets_permission(installation_id, None)
    }

    /// Seeds an installation and its explicit repository Actions secrets permission.
    pub fn create_installation_with_secrets_permission(
        &self,
        installation_id: u64,
        secrets_permission: Option<crate::fixtures::ActionsSecretsPermission>,
    ) -> io::Result<()> {
        self.ensure_running()?;
        lock(&self.app.state).add_installation(
            &self.owner,
            &self.name,
            installation_id,
            secrets_permission.map(crate::fixtures::ActionsSecretsPermission::as_api),
        );
        Ok(())
    }

    /// Seeds the repository public key used to encrypt Actions secrets.
    pub fn set_actions_public_key(
        &self,
        seed: crate::fixtures::ActionsPublicKeySeed,
    ) -> io::Result<()> {
        self.ensure_running()?;
        let mut state = lock(&self.app.state);
        let repo = state
            .account_mut(&self.owner)
            .and_then(|account| account.repo_mut(&self.name))
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "repository does not exist"))?;
        repo.actions_public_key = Some(crate::types::ActionsPublicKey {
            key_id: seed.key_id,
            key: seed.key,
            id: None,
            url: None,
            title: None,
            created_at: None,
        });
        Ok(())
    }

    /// Seeds an already-encrypted Actions secret owned by this repository.
    pub fn create_actions_secret(
        &self,
        seed: crate::fixtures::ActionsSecretSeed,
    ) -> io::Result<ActionsSecret> {
        self.ensure_running()?;
        let mut state = lock(&self.app.state);
        let repo = state
            .account_mut(&self.owner)
            .and_then(|account| account.repo_mut(&self.name))
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "repository does not exist"))?;
        let public_key = repo.actions_public_key.as_ref().ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "Actions public key does not exist")
        })?;
        if seed.key_id != public_key.key_id {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Actions secret key id does not match the repository public key",
            ));
        }
        if repo.actions_secrets.contains_key(&seed.name) {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "Actions secret already exists",
            ));
        }
        let name = seed.name;
        let now = crate::fixtures::current_timestamp();
        repo.actions_secrets.insert(
            name.clone(),
            crate::state::StoredActionsSecret {
                name: name.clone(),
                encrypted_value: seed.encrypted_value,
                key_id: seed.key_id,
                created_at: now,
                updated_at: now,
            },
        );
        Ok(ActionsSecret {
            repository: self.clone(),
            name,
        })
    }

    /// Returns the repository-owned encrypted secret fixture, if present.
    pub fn actions_secret(&self, name: &str) -> io::Result<Option<ActionsSecret>> {
        self.ensure_running()?;
        let state = lock(&self.app.state);
        let repo = state
            .account(&self.owner)
            .and_then(|account| account.repo(&self.name))
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "repository does not exist"))?;
        Ok(repo
            .actions_secrets
            .contains_key(name)
            .then(|| ActionsSecret {
                repository: self.clone(),
                name: name.to_owned(),
            }))
    }

    /// Creates an issue with a sparse typed REST seed.
    pub fn create_issue(&self, number: u64, seed: crate::fixtures::IssueSeed) -> io::Result<Issue> {
        self.ensure_running()?;
        let request = self.api_request();
        let mut state = lock(&self.app.state);
        let default_user = {
            let account = state.account(&self.owner).ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "repository owner does not exist")
            })?;
            account.repo(&self.name).ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "repository does not exist")
            })?;
            crate::render::simple_user(&request, account)
        };
        let issue = crate::fixtures::seeded_issue(
            &mut state,
            &request,
            &self.owner,
            &self.name,
            default_user,
            number,
            seed,
        )?;
        state.add_issue(&self.owner, &self.name, number, issue);
        Ok(Issue {
            repository: self.clone(),
            number,
        })
    }

    /// Creates a pull request with a sparse typed REST seed.
    pub fn create_pull_request(
        &self,
        number: u64,
        seed: crate::fixtures::PullRequestSeed,
    ) -> io::Result<PullRequest> {
        self.ensure_running()?;
        let request = self.api_request();
        let mut state = lock(&self.app.state);
        let (default_user, default_repository) = {
            let account = state.account(&self.owner).ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "repository owner does not exist")
            })?;
            let repository = account.repo(&self.name).ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "repository does not exist")
            })?;
            (
                crate::render::simple_user(&request, account),
                crate::render::repository_for_pull(
                    &request,
                    account,
                    repository,
                    crate::state::Permission {
                        admin: true,
                        push: true,
                        pull: true,
                    },
                ),
            )
        };
        let pull = crate::fixtures::seeded_pull_request(
            &mut state,
            &request,
            (&self.owner, &self.name),
            default_user,
            default_repository,
            number,
            seed,
        )?;
        let exists = state
            .account(&self.owner)
            .and_then(|account| account.repo(&self.name))
            .and_then(|repo| repo.pull_requests.get(&number))
            .is_some_and(|pull| pull.data.is_some());
        if exists {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("pull request #{number} already exists"),
            ));
        }
        let entry = state.add_pull_request(&self.owner, &self.name, number, pull.data);
        entry.files = pull.files;
        for event in pull.events {
            entry
                .apply_event(event)
                .map_err(|message| io::Error::new(io::ErrorKind::InvalidInput, message))?;
        }
        Ok(PullRequest {
            repository: self.clone(),
            number,
        })
    }

    /// Queues a job of a new workflow run for a self-hosted runner carrying every one of
    /// `labels`. Each of `scripts` is a `run:` step. A runner registered to the repository's
    /// organization takes the job through the Actions service endpoints, runs the steps, and
    /// reports the outcome; until then the job is `queued`.
    pub fn queue_script_job(&self, labels: &[&str], scripts: &[&str]) -> io::Result<WorkflowJob> {
        self.ensure_running()?;
        let request = self.api_request();
        let labels: Vec<String> = labels.iter().map(|label| (*label).to_owned()).collect();
        let scripts: Vec<String> = scripts.iter().map(|script| (*script).to_owned()).collect();
        let (run_id, id) = lock(&self.app.state).queue_script_job(
            &request,
            &self.owner,
            &self.name,
            &labels,
            &scripts,
        )?;
        Ok(WorkflowJob { run_id, id })
    }

    /// Creates a workflow run and its jobs with sparse typed seeds.
    pub fn create_workflow_run(
        &self,
        seed: crate::fixtures::WorkflowRunSeed,
        jobs: Vec<crate::fixtures::JobSeed>,
    ) -> io::Result<WorkflowRun> {
        self.ensure_running()?;
        let request = self.api_request();
        let mut state = lock(&self.app.state);
        let run = {
            let account = state.account(&self.owner).ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "repository owner does not exist")
            })?;
            let repository = account.repo(&self.name).ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "repository does not exist")
            })?;
            crate::fixtures::seeded_workflow_run(&request, account, repository, seed)?
        };
        let run_id = u64::try_from(run.id).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("invalid workflow run id: {error}"),
            )
        })?;
        let jobs = jobs
            .into_iter()
            .enumerate()
            .map(|(index, seed)| crate::fixtures::seeded_job(&request, &run, seed, index))
            .collect::<io::Result<Vec<_>>>()?;
        state.add_workflow_run(&self.owner, &self.name, run, jobs);
        Ok(WorkflowRun {
            repository: self.clone(),
            id: run_id,
        })
    }

    /// Creates a Projects v2 board owned by this repository.
    pub fn create_project(&self, title: &str) -> io::Result<Project> {
        self.ensure_running()?;
        let mut state = lock(&self.app.state);
        require_repository(&state, &self.owner, &self.name)?;
        let id = state.add_project(&self.owner, &self.name, title).id;
        Ok(Project {
            app: self.app.clone(),
            node_id: crate::render::node_id("ProjectV2", id),
        })
    }
}

impl ActionsSecret {
    /// The secret's name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Ciphertext stored by the fixture; it is not returned by GitHub REST GET/list routes.
    pub fn encrypted_value(&self) -> io::Result<String> {
        self.field(|secret| secret.encrypted_value.clone())
    }

    /// Identifier of the public key used to encrypt this secret.
    pub fn key_id(&self) -> io::Result<String> {
        self.field(|secret| secret.key_id.clone())
    }

    fn field<T>(
        &self,
        project: impl FnOnce(&crate::state::StoredActionsSecret) -> T,
    ) -> io::Result<T> {
        self.repository.ensure_running()?;
        let state = lock(&self.repository.app.state);
        state
            .account(&self.repository.owner)
            .and_then(|account| account.repo(&self.repository.name))
            .and_then(|repo| repo.actions_secrets.get(&self.name))
            .map(project)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Actions secret does not exist"))
    }
}

impl PullRequest {
    /// Pull request number.
    pub fn number(&self) -> u64 {
        self.number
    }

    /// Adds a review to this pull request.
    pub fn create_review(&self, seed: crate::fixtures::ReviewSeed) -> io::Result<Review> {
        self.repository.ensure_running()?;
        let api = format!(
            "{}/repos/{}/{}",
            self.repository.server_url, self.repository.owner, self.repository.name
        );
        let web = format!(
            "{}/{}/{}",
            self.repository.server_url, self.repository.owner, self.repository.name
        );
        let mut state = lock(&self.repository.app.state);
        require_pull_request(&state, &self.repository, self.number)?;
        let user = seeded_nullable_user(&mut state, &self.repository, seed.user)?;
        let default_id = {
            let pull = state
                .account(&self.repository.owner)
                .and_then(|account| account.repo(&self.repository.name))
                .and_then(|repo| repo.pull_requests.get(&self.number))
                .expect("pull request checked above");
            let mut candidate = self
                .number
                .checked_mul(100)
                .and_then(|id| id.checked_add(1))
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "review id overflow"))?;
            while pull
                .reviews
                .iter()
                .any(|review| i64::try_from(candidate).ok() == Some(review.id))
            {
                candidate = candidate.checked_add(1).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "review id overflow")
                })?;
            }
            candidate
        };
        let id = match &seed.id {
            crate::fixtures::SeedField::Missing => i64::try_from(default_id)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?,
            crate::fixtures::SeedField::Present(id) => {
                crate::fixtures::model_id_signed(*id, "pull request review")?
            }
        };
        let global_id = u64::try_from(id)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        let created = crate::state::now();
        let mut review = types::PullRequestReview {
            author_association: types::AuthorAssociation::None,
            body: String::new(),
            body_html: None,
            body_text: None,
            commit_id: None,
            html_url: format!("{web}/pull/{}#pullrequestreview-{id}", self.number),
            id,
            links: types::PullRequestReviewLinks {
                html: types::PullRequestReviewLinksHtml {
                    href: format!("{web}/pull/{}#pullrequestreview-{id}", self.number),
                },
                pull_request: types::PullRequestReviewLinksPullRequest {
                    href: format!("{api}/pulls/{}", self.number),
                },
            },
            node_id: crate::render::node_id("PullRequestReview", global_id),
            pull_request_url: format!("{api}/pulls/{}", self.number),
            state: "COMMENTED".to_owned(),
            submitted_at: Some(crate::fixtures::parse_timestamp(
                &created,
                "review submitted_at",
            )?),
            user,
        };
        if state
            .account(&self.repository.owner)
            .and_then(|account| account.repo(&self.repository.name))
            .and_then(|repo| repo.pull_requests.get(&self.number))
            .is_some_and(|pull| pull.reviews.iter().any(|existing| existing.id == review.id))
        {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("review {} already exists", review.id),
            ));
        }
        seed.body.apply(&mut review.body);
        seed.state.apply(&mut review.state);
        seed.commit_id.apply(&mut review.commit_id);
        if let crate::fixtures::SeedField::Present(value) = seed.submitted_at {
            review.submitted_at = value
                .map(|value| crate::fixtures::parse_timestamp(&value, "review submitted_at"))
                .transpose()?;
        }
        let id = review.id;
        state.add_review(
            &self.repository.owner,
            &self.repository.name,
            self.number,
            review,
        );
        Ok(Review {
            pull_request: self.clone(),
            id,
        })
    }

    /// Adds an issue comment to this pull request.
    pub fn create_issue_comment(&self, seed: crate::fixtures::IssueCommentSeed) -> io::Result<()> {
        create_issue_comment(&self.repository, self.number, seed)
    }

    /// Adds a review comment to this pull request.
    pub fn create_review_comment(
        &self,
        seed: crate::fixtures::ReviewCommentSeed,
    ) -> io::Result<()> {
        create_review_comment(&self.repository, self.number, seed)
    }

    /// Adds a persisted GraphQL review thread and returns its node id.
    pub fn create_review_thread(
        &self,
        seed: crate::fixtures::ReviewThreadSeed,
    ) -> io::Result<String> {
        self.repository.ensure_running()?;
        let supplied_id = match &seed.id {
            crate::fixtures::SeedField::Present(id) => Some(id.clone()),
            crate::fixtures::SeedField::Missing => None,
        };
        let mut state = lock(&self.repository.app.state);
        require_pull_request(&state, &self.repository, self.number)?;
        let thread_index = state
            .account(&self.repository.owner)
            .and_then(|account| account.repo(&self.repository.name))
            .and_then(|repo| repo.pull_requests.get(&self.number))
            .map(|pull| pull.review_threads.len())
            .ok_or_else(|| not_found("pull request does not exist"))?;
        let thread = crate::state::ReviewThread {
            id: supplied_id.unwrap_or_else(|| {
                crate::render::node_id("PullRequestReviewThread", state.next_id())
            }),
            is_resolved: match seed.is_resolved {
                crate::fixtures::SeedField::Missing => false,
                crate::fixtures::SeedField::Present(value) => value,
            },
            path: match seed.path {
                crate::fixtures::SeedField::Missing => None,
                crate::fixtures::SeedField::Present(value) => value,
            },
            line: match seed.line {
                crate::fixtures::SeedField::Missing => None,
                crate::fixtures::SeedField::Present(value) => value,
            },
            comments: match seed.comments {
                crate::fixtures::SeedField::Missing => Vec::new(),
                crate::fixtures::SeedField::Present(comments) => comments
                    .into_iter()
                    .enumerate()
                    .map(|(index, comment)| -> io::Result<_> {
                        let id = thread_comment_id(self.number, thread_index, index)?;
                        Ok(crate::state::ReviewThreadComment {
                            id,
                            body: match comment.body {
                                crate::fixtures::SeedField::Missing => String::new(),
                                crate::fixtures::SeedField::Present(value) => value,
                            },
                            updated_at: match comment.updated_at {
                                crate::fixtures::SeedField::Missing => String::new(),
                                crate::fixtures::SeedField::Present(value) => value,
                            },
                        })
                    })
                    .collect::<io::Result<Vec<_>>>()?,
            },
        };
        let thread_id = thread.id.clone();
        state.add_review_thread(
            &self.repository.owner,
            &self.repository.name,
            self.number,
            thread,
        );
        Ok(thread_id)
    }

    /// Sets the explicit GraphQL review decision for this pull request.
    pub fn set_review_decision(&self, decision: Option<String>) -> io::Result<()> {
        self.repository.ensure_running()?;
        let mut state = lock(&self.repository.app.state);
        require_pull_request(&state, &self.repository, self.number)?;
        state.set_review_decision(
            &self.repository.owner,
            &self.repository.name,
            self.number,
            decision,
        );
        Ok(())
    }

    /// Records a timeline event, applying it to the pull request as the action would: readying
    /// or drafting it, requesting or withdrawing a review, merging, closing or reopening it.
    ///
    /// # Errors
    ///
    /// Refuses an unknown event, a force-push (use [`Self::push_head`]), an event dated before
    /// the pull request's creation or its latest event, and one its current state does not
    /// allow, such as readying a pull request that is not a draft.
    pub fn create_timeline_event(
        &self,
        seed: crate::fixtures::TimelineEventSeed,
    ) -> io::Result<()> {
        use crate::fixtures::SeedField;
        use crate::state::TimelineEventKind;
        self.repository.ensure_running()?;
        let invalid = |message: String| io::Error::new(io::ErrorKind::InvalidInput, message);
        let kind = TimelineEventKind::parse(&seed.event)
            .filter(|kind| *kind != TimelineEventKind::HeadRefForcePushed)
            .ok_or_else(|| {
                invalid(format!(
                    "cannot seed timeline event '{}'; force-pushes come from push_head",
                    seed.event
                ))
            })?;
        let created_at = crate::fixtures::parse_timestamp(&seed.created_at, "event created_at")?;
        let request = self.repository.api_request();
        let mut state = lock(&self.repository.app.state);
        require_pull_request(&state, &self.repository, self.number)?;
        let (author, head_sha, earliest) = {
            let pull = state
                .account(&self.repository.owner)
                .and_then(|account| account.repo(&self.repository.name))
                .and_then(|repo| repo.pull_requests.get(&self.number))
                .expect("pull request checked above");
            let data = pull
                .data
                .as_ref()
                .ok_or_else(|| invalid("the pull request has no REST model".to_owned()))?;
            let latest = pull.events.iter().map(|event| event.created_at).max();
            (
                data.user.clone(),
                data.head.sha.clone(),
                latest.map_or(data.created_at, |latest| latest.max(data.created_at)),
            )
        };
        if created_at < earliest {
            return Err(invalid(format!(
                "event {} at {} is earlier than the pull request's creation or latest event ({})",
                seed.event,
                seed.created_at,
                earliest.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
            )));
        }
        let actor = match seed.actor {
            SeedField::Present(user) => crate::fixtures::simple_user(&mut state, &request, user)?,
            SeedField::Missing => author,
        };
        let requested_reviewer = match seed.requested_reviewer {
            SeedField::Present(user) => {
                Some(crate::fixtures::simple_user(&mut state, &request, user)?)
            }
            SeedField::Missing => None,
        };
        let requested_team = match seed.requested_team {
            SeedField::Present(team) => Some(crate::fixtures::requested_team(
                &mut state,
                &request,
                &self.repository.owner,
                &team.slug,
            )?),
            SeedField::Missing => None,
        };
        let is_request = matches!(
            kind,
            TimelineEventKind::ReviewRequested | TimelineEventKind::ReviewRequestRemoved
        );
        if is_request != (requested_reviewer.is_some() || requested_team.is_some())
            || (requested_reviewer.is_some() && requested_team.is_some())
        {
            return Err(invalid(
                "review-request events name exactly one requested_reviewer or requested_team; \
                 other events name neither"
                    .to_owned(),
            ));
        }
        let commit_id = match (kind, seed.commit_id) {
            (TimelineEventKind::Merged, SeedField::Present(sha)) => Some(sha),
            (TimelineEventKind::Merged, SeedField::Missing) => Some(head_sha),
            (_, SeedField::Present(_)) => {
                return Err(invalid("only a merged event names a commit_id".to_owned()));
            }
            (_, SeedField::Missing) => None,
        };
        let id = state.next_id();
        let pull = state
            .account_mut(&self.repository.owner)
            .and_then(|account| account.repo_mut(&self.repository.name))
            .and_then(|repo| repo.pull_requests.get_mut(&self.number))
            .expect("pull request checked above");
        pull.apply_event(crate::state::TimelineEvent {
            id,
            kind,
            actor,
            created_at,
            commit_id,
            requested_reviewer,
            requested_team,
        })
        .map_err(invalid)
    }

    /// Moves the pull request's head branch to `sha`, a commit already in the repository, as a
    /// push at `pushed_at` (now by default) by `actor` (the author by default) would.
    ///
    /// Commits new to the branch are listed in the timeline at `pushed_at`. When the old head
    /// is not an ancestor of `sha`, a `head_ref_force_pushed` event is recorded.
    ///
    /// # Errors
    ///
    /// Fails for a closed pull request, a commit the repository does not have, a malformed
    /// timestamp, and git failures.
    pub fn push_head(
        &self,
        sha: &str,
        pushed_at: Option<&str>,
        actor: Option<crate::fixtures::UserSeed>,
    ) -> io::Result<()> {
        self.repository.ensure_running()?;
        let at = match pushed_at {
            Some(value) => crate::fixtures::parse_timestamp(value, "pushed_at")?,
            None => crate::fixtures::current_timestamp(),
        };
        let request = self.repository.api_request();
        let mut state = lock(&self.repository.app.state);
        require_pull_request(&state, &self.repository, self.number)?;
        let (git_dir, branch, author, open) = {
            let repo = state
                .account(&self.repository.owner)
                .and_then(|account| account.repo(&self.repository.name))
                .expect("pull request checked above");
            let data = repo
                .pull_requests
                .get(&self.number)
                .and_then(|pull| pull.data.as_ref())
                .ok_or_else(|| not_found("the pull request has no REST model"))?;
            (
                repo.git_dir
                    .clone()
                    .ok_or_else(|| io::Error::other("repository has no Git storage"))?,
                data.head.ref_.clone(),
                data.user.clone(),
                data.state == types::PullRequestState::Open,
            )
        };
        if !open {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a push does not move the head of a closed pull request",
            ));
        }
        let sha = ninot_common::git::resolve_commit(&git_dir, sha)?
            .ok_or_else(|| not_found(format!("commit {sha} is not in the repository")))?;
        let actor = match actor {
            Some(user) => crate::fixtures::simple_user(&mut state, &request, user)?,
            None => author,
        };
        let event_id = state.next_id();
        ninot_common::git::create_ref(
            &git_dir,
            &format!("refs/heads/{branch}"),
            &sha,
            &crate::handlers::BOT,
        )?;
        let pull = state
            .account_mut(&self.repository.owner)
            .and_then(|account| account.repo_mut(&self.repository.name))
            .and_then(|repo| repo.pull_requests.get_mut(&self.number))
            .expect("pull request checked above");
        crate::handlers::timeline::record_head_update(&git_dir, pull, &sha, at, actor, event_id)
    }
}

impl Review {
    /// Numeric review id.
    pub fn id(&self) -> io::Result<i64> {
        self.pull_request.repository.ensure_running()?;
        let state = lock(&self.pull_request.repository.app.state);
        state
            .account(&self.pull_request.repository.owner)
            .and_then(|account| account.repo(&self.pull_request.repository.name))
            .and_then(|repo| repo.pull_requests.get(&self.pull_request.number))
            .and_then(|pull| pull.reviews.iter().find(|review| review.id == self.id))
            .map(|review| review.id)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "pull request review does not exist",
                )
            })
    }

    /// GitHub global node id for this review.
    pub fn node_id(&self) -> io::Result<String> {
        self.pull_request.repository.ensure_running()?;
        let state = lock(&self.pull_request.repository.app.state);
        state
            .account(&self.pull_request.repository.owner)
            .and_then(|account| account.repo(&self.pull_request.repository.name))
            .and_then(|repo| repo.pull_requests.get(&self.pull_request.number))
            .and_then(|pull| pull.reviews.iter().find(|review| review.id == self.id))
            .map(|review| review.node_id.clone())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "pull request review does not exist",
                )
            })
    }
}

impl Issue {
    /// Issue number.
    pub fn number(&self) -> u64 {
        self.number
    }

    /// Adds a comment to this issue.
    pub fn create_comment(&self, seed: crate::fixtures::IssueCommentSeed) -> io::Result<()> {
        self.repository.ensure_running()?;
        create_issue_comment(&self.repository, self.number, seed)
    }
}

impl WorkflowJob {
    /// Workflow job identifier.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Identifier of the workflow run the job belongs to.
    pub fn run_id(&self) -> u64 {
        self.run_id
    }
}

impl WorkflowRun {
    /// Workflow run identifier.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Seeds a plain-text log for a job in this run.
    pub fn set_job_log(&self, job_id: u64, text: String) -> io::Result<()> {
        self.repository.ensure_running()?;
        let mut state = lock(&self.repository.app.state);
        require_repository(&state, &self.repository.owner, &self.repository.name)?;
        let repo = state
            .account_mut(&self.repository.owner)
            .and_then(|account| account.repo_mut(&self.repository.name))
            .ok_or_else(|| not_found("repository does not exist"))?;
        repo.workflow_job_logs.insert(job_id, text);
        Ok(())
    }
}

impl Commit {
    /// Commit SHA.
    pub fn sha(&self) -> &str {
        &self.sha
    }

    /// Adds a check run associated with this commit.
    pub fn create_check_run(&self, seed: crate::fixtures::CheckRunSeed) -> io::Result<()> {
        self.repository.ensure_running()?;
        let request = self.repository.api_request();
        let mut state = lock(&self.repository.app.state);
        require_repository(&state, &self.repository.owner, &self.repository.name)?;
        let fallback = state
            .account(&self.repository.owner)
            .and_then(|account| account.repo(&self.repository.name))
            .and_then(|repo| repo.check_runs.get(&self.sha))
            .map_or(0, Vec::len);
        let check = crate::fixtures::seeded_check_run(
            &mut state,
            &request,
            &self.repository.owner,
            &self.repository.name,
            &self.sha,
            seed,
            fallback,
        )?;
        state.add_check_run(
            &self.repository.owner,
            &self.repository.name,
            &self.sha,
            check,
        );
        Ok(())
    }

    /// Adds a commit status associated with this commit.
    pub fn create_status(&self, seed: crate::fixtures::CommitStatusSeed) -> io::Result<()> {
        self.repository.ensure_running()?;
        let request = self.repository.api_request();
        let mut state = lock(&self.repository.app.state);
        require_repository(&state, &self.repository.owner, &self.repository.name)?;
        let fallback = state
            .account(&self.repository.owner)
            .and_then(|account| account.repo(&self.repository.name))
            .and_then(|repo| repo.commit_statuses.get(&self.sha))
            .map_or(0, Vec::len);
        let status = {
            let account = state.account(&self.repository.owner).ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "repository owner does not exist")
            })?;
            let repository = account.repo(&self.repository.name).ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "repository does not exist")
            })?;
            crate::fixtures::seeded_commit_status(
                &request, account, repository, &self.sha, seed, fallback,
            )?
        };
        state.add_commit_status(
            &self.repository.owner,
            &self.repository.name,
            &self.sha,
            status,
        );
        Ok(())
    }
}

impl Project {
    /// Projects v2 GraphQL node id.
    pub fn node_id(&self) -> &str {
        &self.node_id
    }

    /// Adds a single-select field and its options.
    pub fn create_field(&self, name: &str, options: Vec<String>) -> io::Result<()> {
        ensure_app_running(&self.app)?;
        let id =
            crate::graphql::database_id(&self.node_id).expect("project id is a GraphQL node id");
        lock(&self.app.state).add_project_field(id, name, options);
        Ok(())
    }

    /// Adds an issue-backed project item.
    pub fn create_issue_item(
        &self,
        issue: &Issue,
        field_values: std::collections::BTreeMap<String, String>,
    ) -> io::Result<String> {
        ensure_app_running(&self.app)?;
        issue.repository.ensure_running()?;
        if !Arc::ptr_eq(&self.app, &issue.repository.app) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "project and issue must belong to the same server",
            ));
        }
        let state = lock(&self.app.state);
        require_issue(&state, &issue.repository, issue.number)?;
        drop(state);
        let id =
            crate::graphql::database_id(&self.node_id).expect("project id is a GraphQL node id");
        let item = lock(&self.app.state).add_project_item(
            id,
            &issue.repository.owner,
            &issue.repository.name,
            issue.number,
            field_values,
        );
        Ok(crate::render::node_id("ProjectV2Item", item))
    }
}

/// A seeded commit identity as git records it.
fn git_identity(seed: &crate::fixtures::GitIdentitySeed) -> crate::fixture_git::Identity<'_> {
    crate::fixture_git::Identity {
        name: &seed.name,
        email: &seed.email,
        date: &seed.date,
    }
}

fn seeded_nullable_user(
    state: &mut State,
    repository: &Repository,
    seed: crate::fixtures::SeedField<Option<crate::fixtures::UserSeed>>,
) -> io::Result<Option<types::NullableSimpleUser>> {
    match seed {
        crate::fixtures::SeedField::Present(None) => Ok(None),
        crate::fixtures::SeedField::Present(Some(seed)) => {
            let request = repository.api_request();
            crate::fixtures::simple_user(state, &request, seed).map(|user| Some(user.into()))
        }
        crate::fixtures::SeedField::Missing => {
            let request = repository.api_request();
            crate::fixtures::simple_user(state, &request, crate::fixtures::UserSeed::default())
                .map(|user| Some(user.into()))
        }
    }
}

fn require_repository(state: &State, owner: &str, name: &str) -> io::Result<()> {
    state
        .account(owner)
        .and_then(|account| account.repo(name))
        .map(|_| ())
        .ok_or_else(|| not_found(format!("repository {owner}/{name} does not exist")))
}

fn require_pull_request(state: &State, repository: &Repository, number: u64) -> io::Result<()> {
    state
        .account(&repository.owner)
        .and_then(|account| account.repo(&repository.name))
        .and_then(|repo| repo.pull_requests.get(&number))
        .map(|_| ())
        .ok_or_else(|| {
            not_found(format!(
                "pull request {}/{}#{number} does not exist",
                repository.owner, repository.name
            ))
        })
}

fn require_issue(state: &State, repository: &Repository, number: u64) -> io::Result<()> {
    state
        .account(&repository.owner)
        .and_then(|account| account.repo(&repository.name))
        .and_then(|repo| repo.issues.get(&number))
        .map(|_| ())
        .ok_or_else(|| {
            not_found(format!(
                "issue {}/{}#{number} does not exist",
                repository.owner, repository.name
            ))
        })
}

fn not_found(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, message.into())
}

fn create_issue_comment(
    repository: &Repository,
    number: u64,
    seed: crate::fixtures::IssueCommentSeed,
) -> io::Result<()> {
    repository.ensure_running()?;
    let api = format!(
        "{}/repos/{}/{}",
        repository.server_url, repository.owner, repository.name
    );
    let web = format!(
        "{}/{}/{}",
        repository.server_url, repository.owner, repository.name
    );
    let mut state = lock(&repository.app.state);
    require_repository(&state, &repository.owner, &repository.name)?;
    let repo = state
        .account(&repository.owner)
        .and_then(|account| account.repo(&repository.name))
        .ok_or_else(|| not_found("repository does not exist"))?;
    if !repo.issues.contains_key(&number) && !repo.pull_requests.contains_key(&number) {
        return Err(not_found(format!(
            "issue or pull request {}/{}#{number} does not exist",
            repository.owner, repository.name
        )));
    }
    let user = seeded_nullable_user(&mut state, repository, seed.user)?;
    let global_id = state.next_id();
    let id = crate::render::id(global_id);
    let now = crate::state::now();
    let timestamp = crate::fixtures::parse_timestamp(&now, "issue comment created_at")?;
    let mut comment = types::IssueComment {
        author_association: Some(types::AuthorAssociation::None),
        body: Some(String::new()),
        body_html: None,
        body_text: None,
        created_at: timestamp,
        html_url: format!("{web}/pull/{number}#issuecomment-{id}"),
        id,
        issue_url: format!("{api}/issues/{number}"),
        minimized: None,
        node_id: crate::render::node_id("IssueComment", global_id),
        performed_via_github_app: None,
        pin: None,
        reactions: None,
        updated_at: crate::fixtures::parse_timestamp(&now, "issue comment updated_at")?,
        url: format!("{api}/issues/comments/{id}"),
        user,
    };
    seed.body
        .apply(comment.body.get_or_insert_with(String::new));
    if let crate::fixtures::SeedField::Present(value) = seed.created_at {
        comment.created_at = crate::fixtures::parse_timestamp(&value, "issue comment created_at")?;
    }
    if let crate::fixtures::SeedField::Present(value) = seed.updated_at {
        comment.updated_at = crate::fixtures::parse_timestamp(&value, "issue comment updated_at")?;
    }
    state.add_issue_comment(&repository.owner, &repository.name, number, comment);
    Ok(())
}

fn create_review_comment(
    repository: &Repository,
    number: u64,
    seed: crate::fixtures::ReviewCommentSeed,
) -> io::Result<()> {
    repository.ensure_running()?;
    let api = format!(
        "{}/repos/{}/{}",
        repository.server_url, repository.owner, repository.name
    );
    let web = format!(
        "{}/{}/{}",
        repository.server_url, repository.owner, repository.name
    );
    let mut state = lock(&repository.app.state);
    require_pull_request(&state, repository, number)?;
    let user = seeded_nullable_user(&mut state, repository, seed.user.clone())?;
    let global_id = state.next_id();
    let id = crate::render::id(global_id);
    let review_id = checked_fixture_id(number, 100, "pull request review")?;
    let now = crate::state::now();
    let timestamp = crate::fixtures::parse_timestamp(&now, "review comment created_at")?;
    let mut comment = types::PullRequestReviewComment {
        author_association: types::AuthorAssociation::None,
        body: String::new(),
        body_html: None,
        body_text: None,
        commit_id: "0000000000000000000000000000000000000000".to_owned(),
        created_at: timestamp,
        diff_hunk: "@@ -1 +1 @@".to_owned(),
        html_url: format!("{web}/pull/{number}#discussion-diff-{id}"),
        id,
        in_reply_to_id: None,
        line: None,
        links: types::PullRequestReviewCommentLinks {
            html: types::PullRequestReviewCommentLinksHtml {
                href: format!("{web}/pull/{number}#discussion-diff-{id}"),
            },
            pull_request: types::PullRequestReviewCommentLinksPullRequest {
                href: format!("{api}/pulls/{number}"),
            },
            self_: types::PullRequestReviewCommentLinksSelf {
                href: format!("{api}/pulls/comments/{id}"),
            },
        },
        node_id: crate::render::node_id("PullRequestReviewComment", global_id),
        original_commit_id: "0000000000000000000000000000000000000000".to_owned(),
        original_line: None,
        original_position: Some(1),
        original_start_line: None,
        path: "README.md".to_owned(),
        position: Some(1),
        pull_request_review_id: Some(review_id),
        pull_request_url: format!("{api}/pulls/{number}"),
        reactions: None,
        side: types::PullRequestReviewCommentSide::Right,
        start_line: None,
        start_side: None,
        subject_type: None,
        updated_at: crate::fixtures::parse_timestamp(&now, "review comment updated_at")?,
        url: format!("{api}/pulls/comments/{id}"),
        user,
    };
    seed.path.apply(&mut comment.path);
    seed.line.apply(&mut comment.line);
    seed.position.apply(&mut comment.position);
    seed.body.apply(&mut comment.body);
    if let crate::fixtures::SeedField::Present(id) = seed.pull_request_review_id {
        comment.pull_request_review_id = Some(id);
    }
    if let crate::fixtures::SeedField::Present(id) = seed.in_reply_to_id {
        comment.in_reply_to_id = Some(id);
    }
    if let crate::fixtures::SeedField::Present(value) = seed.created_at {
        comment.created_at = crate::fixtures::parse_timestamp(&value, "review comment created_at")?;
    }
    if let crate::fixtures::SeedField::Present(value) = seed.updated_at {
        comment.updated_at = crate::fixtures::parse_timestamp(&value, "review comment updated_at")?;
    }
    state.add_review_comment(&repository.owner, &repository.name, number, comment);
    Ok(())
}

fn checked_fixture_id(number: u64, multiplier: u64, kind: &str) -> io::Result<i64> {
    let id = number
        .checked_mul(multiplier)
        .and_then(|value| value.checked_add(1))
        .ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, format!("{kind} id overflow"))
        })?;
    i64::try_from(id).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid {kind} id: {error}"),
        )
    })
}

fn thread_comment_id(number: u64, thread_index: usize, comment_index: usize) -> io::Result<String> {
    let thread_offset = u64::try_from(thread_index)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?
        .checked_mul(100)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "thread index overflow"))?;
    let comment_offset = u64::try_from(comment_index)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?
        .checked_add(1)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "comment index overflow"))?;
    let id = number
        .checked_mul(1000)
        .and_then(|id| id.checked_add(thread_offset))
        .and_then(|id| id.checked_add(comment_offset))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "thread comment id overflow"))?;
    Ok(crate::render::node_id("PullRequestReviewComment", id))
}

fn ensure_app_running(app: &App) -> io::Result<()> {
    if app.running.load(Ordering::Acquire) {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::BrokenPipe,
            "server was stopped",
        ))
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.app.running.store(false, Ordering::Release);
        self.harness.stop();
        let _ = std::fs::remove_dir_all(&self.data_dir);
    }
}

async fn dispatch(AxumState(app): AxumState<Arc<App>>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let body = match to_bytes(body, usize::MAX).await {
        Ok(body) => body,
        Err(_) => return into_response(ApiResponse::error(400, "cannot read request body"), None),
    };
    let path = parts.uri.path().to_owned();
    let handler_app = app.clone();
    let response = tokio::task::spawn_blocking(move || handler_app.handle(parts, body))
        .await
        .unwrap_or_else(|_| {
            app.record(Hit::Panic { path });
            into_response(ApiResponse::error(500, "handler panicked"), None)
        });
    let fault = response.extensions().get::<Fault>().cloned();
    match fault {
        Some(fault) => ninot_common::faults::apply(&fault, response).await,
        None => response,
    }
}

/// Body of a `403` secondary rate limit, as GitHub words it.
const SECONDARY_RATE_LIMIT: &str = "You have exceeded a secondary rate limit. Please wait a few minutes before you try again. If you reach out to GitHub Support for help, please include the request ID.";

impl App {
    /// Applies rate limits and injected faults around [`App::serve`].
    fn handle(&self, parts: Parts, body: bytes::Bytes) -> Response {
        let full_path = parts.uri.path();
        let path = full_path
            .strip_prefix(ENTERPRISE_PREFIX)
            .or_else(|| full_path.strip_prefix(UPLOADS_PREFIX))
            .filter(|rest| rest.is_empty() || rest.starts_with('/'))
            .map_or(full_path, |rest| if rest.is_empty() { "/" } else { rest })
            .to_owned();
        let git_path = git::parse_path(full_path);
        let git_service = git_path.as_ref().map(|git_path| {
            if git::is_push(git_path.service_path, parts.uri.query().unwrap_or("")) {
                "receive-pack"
            } else {
                "upload-pack"
            }
        });
        let operation = match git_path {
            Some(_) => None,
            None => method(&parts.method)
                .and_then(|method| self.router.find(method, &path))
                .map(|found| found.info.id),
        };
        let graphql = parts.method == http::Method::POST
            && (full_path == "/api/graphql" || path == "/graphql");
        let client = self.client(&parts);
        let resource = match operation {
            Some("rate-limit/get") => None,
            Some(id) if id.starts_with("search/") => Some(Resource::Search),
            Some(_) => Some(Resource::Core),
            // GraphQL answers anonymous callers with a 401 before running anything.
            None if graphql && matches!(client, Client::Token(_)) => Some(Resource::Graphql),
            None => None,
        };
        let creates_content =
            !graphql && !matches!(parts.method, http::Method::GET | http::Method::HEAD);
        let admitted = match resource {
            None => None,
            Some(resource) => {
                let admitted =
                    lock(&self.state)
                        .rate_limits
                        .admit(&client, resource, creates_content);
                match admitted {
                    Ok(window) => Some((resource, window)),
                    Err(refusal) => return self.refuse(&client, resource, refusal),
                }
            }
        };
        let mut response = self.apply_faults(parts, body, &path, operation, git_service, &client);
        let mut state = lock(&self.state);
        let headers = match admitted {
            Some((resource, window)) => {
                state.rate_limits.release(&client);
                window.headers(resource)
            }
            None if operation == Some("rate-limit/get") => state
                .rate_limits
                .window(&client, Resource::Core)
                .headers(Resource::Core),
            None => Vec::new(),
        };
        for (name, value) in headers {
            response.headers_mut().insert(
                http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                HeaderValue::from_str(&value).unwrap(),
            );
        }
        if let Some(operation) = operation {
            response
                .extensions_mut()
                .insert(Label(operation.to_owned()));
        }
        response
    }

    /// Answers a request refused by the rate limits.
    fn refuse(&self, client: &Client, resource: Resource, refusal: Refusal) -> Response {
        let (mut response, window) = match refusal {
            Refusal::Primary(window) => {
                let state = lock(&self.state);
                let who = match client {
                    Client::Ip(ip) => ip.to_string(),
                    Client::Token(token) => {
                        let login = &state.tokens[token].login;
                        let id = state.account(login).map_or(0, |account| account.id);
                        format!("user ID {id}")
                    }
                };
                let message = format!("API rate limit exceeded for {who}.");
                (ApiResponse::error(403, &message), window)
            }
            Refusal::Secondary {
                window,
                retry_after,
            } => (
                ApiResponse::error(403, SECONDARY_RATE_LIMIT)
                    .with_header("retry-after", retry_after.to_string()),
                window,
            ),
        };
        response.headers.extend(window.headers(resource));
        into_response(response, None)
    }

    /// Who rate limits count a request against: its valid token, or else its peer address.
    fn client(&self, parts: &Parts) -> Client {
        let token = token_from_authorization(&parts.headers)
            .filter(|token| lock(&self.state).tokens.contains_key(token));
        match token {
            Some(token) => Client::Token(token),
            None => {
                let ConnectInfo(address) = parts
                    .extensions
                    .get::<ConnectInfo<SocketAddr>>()
                    .expect("connections carry their peer address");
                Client::Ip(address.ip())
            }
        }
    }

    /// Applies injected faults around [`App::serve`].
    fn apply_faults(
        &self,
        parts: Parts,
        body: bytes::Bytes,
        path: &str,
        operation: Option<&str>,
        git_service: Option<&str>,
        client: &Client,
    ) -> Response {
        let info = RequestInfo {
            method: parts.method.as_str(),
            path,
            operation,
            git_service,
        };
        let decision = lock(&self.state)
            .faults
            .decide(info.method, info.path, |target| target.matches(&info));
        if !decision.latency.is_zero() {
            std::thread::sleep(decision.latency);
        }
        let terminal = match decision.terminal {
            None => return self.serve(parts, body, client),
            Some(Fault::Status(status)) => {
                let message = if status >= 500 {
                    "Server Error"
                } else {
                    "Injected fault"
                };
                return into_response(ApiResponse::error(status, message), None);
            }
            Some(Fault::SecondaryRateLimit) => {
                return into_response(
                    ApiResponse::error(403, SECONDARY_RATE_LIMIT)
                        .with_header("retry-after", "60".to_owned()),
                    None,
                );
            }
            Some(terminal) => terminal,
        };
        let mut response = match terminal {
            Fault::Drop => Response::new(AxumBody::empty()),
            _ => self.serve(parts, body, client),
        };
        response.extensions_mut().insert(terminal);
        response
    }

    fn serve(&self, parts: Parts, body: bytes::Bytes, client: &Client) -> Response {
        let host = parts
            .headers
            .get(http::header::HOST)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("localhost")
            .to_owned();
        let web_url = format!("{}://{host}", self.scheme);
        let full_path = parts.uri.path();
        if parts.method == http::Method::GET
            && let Some(rest) = full_path.strip_prefix("/_ninot/actions/logs/")
        {
            let parts: Vec<&str> = rest.split('/').collect();
            if let [owner, repo, id] = parts.as_slice()
                && let Ok(id) = id.parse::<u64>()
            {
                let text = lock(&self.state)
                    .account(owner)
                    .and_then(|account| account.repo(repo))
                    .and_then(|repo| repo.workflow_job_logs.get(&id))
                    .cloned();
                if let Some(text) = text {
                    return into_response(
                        ApiResponse::bytes(200, "text/plain; charset=utf-8", text.into_bytes()),
                        None,
                    );
                }
            }
            return into_response(ApiResponse::not_found(), None);
        }
        if parts.method == http::Method::GET
            && let Some(rest) = full_path.strip_prefix("/_ninot/actions/log-archives/")
        {
            let segments: Vec<&str> = rest.split('/').collect();
            if let [owner, repo_name, run_id] = segments.as_slice()
                && let Ok(run_id) = run_id.parse::<u64>()
            {
                let caller = match self.authenticate(&parts.headers) {
                    Ok(caller) => caller,
                    Err(response) => return into_response(response, None),
                };
                let archive_data = {
                    let state = lock(&self.state);
                    if state
                        .permission(caller.as_ref(), owner, repo_name)
                        .is_none()
                    {
                        return into_response(ApiResponse::not_found(), caller.as_ref());
                    }
                    let Some(repository) = state
                        .account(owner)
                        .and_then(|account| account.repo(repo_name))
                    else {
                        return into_response(ApiResponse::not_found(), caller.as_ref());
                    };
                    let run_exists = repository
                        .workflow_runs
                        .iter()
                        .any(|run| u64::try_from(run.id).ok() == Some(run_id));
                    let Some(jobs) = repository.workflow_jobs.get(&run_id) else {
                        return into_response(ApiResponse::not_found(), caller.as_ref());
                    };
                    if !run_exists {
                        return into_response(ApiResponse::not_found(), caller.as_ref());
                    }
                    jobs.iter()
                        .filter_map(|job| {
                            let id = u64::try_from(job.id).ok()?;
                            repository
                                .workflow_job_logs
                                .get(&id)
                                .map(|text| (id, text.clone()))
                        })
                        .collect::<Vec<_>>()
                };
                let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
                let options = zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Deflated);
                for (job_id, log) in archive_data {
                    let path = format!("run-{run_id}/job-{job_id}.txt");
                    if archive.start_file(path, options).is_err()
                        || archive.write_all(log.as_bytes()).is_err()
                    {
                        return into_response(
                            ApiResponse::error(500, "could not build workflow log archive"),
                            None,
                        );
                    }
                }
                let bytes = match archive.finish() {
                    Ok(bytes) => bytes.into_inner(),
                    Err(_) => {
                        return into_response(
                            ApiResponse::error(500, "could not build workflow log archive"),
                            None,
                        );
                    }
                };
                return into_response(
                    ApiResponse::bytes(200, "application/zip", bytes),
                    caller.as_ref(),
                );
            }
            return into_response(ApiResponse::not_found(), None);
        }
        let (api_url, path) = match full_path
            .strip_prefix(ENTERPRISE_PREFIX)
            .or_else(|| full_path.strip_prefix(UPLOADS_PREFIX))
        {
            Some(rest) if rest.is_empty() || rest.starts_with('/') => (
                format!("{web_url}{ENTERPRISE_PREFIX}"),
                if rest.is_empty() { "/" } else { rest },
            ),
            _ => (web_url.clone(), full_path),
        };
        if let Some(response) = handlers::actions_service::serve(
            &self.state,
            &parts.method,
            &parts.headers,
            path,
            parts.uri.query(),
            &body,
            &web_url,
        ) {
            if response.status == 404 {
                self.record(Hit::Unrouted {
                    method: parts.method.to_string(),
                    path: path.to_owned(),
                });
            }
            return into_response(response, None);
        }
        if let Some(response) = handlers::runner_service::serve(
            &self.state,
            &parts.method,
            &parts.headers,
            path,
            parts.uri.query(),
            &body,
            &web_url,
        ) {
            return into_response(response, None);
        }
        if let Some(git_path) = git::parse_path(full_path) {
            let response = self.serve_git(&parts, &body, &git_path);
            self.record(Hit::Git {
                service: git_path.service_path.to_owned(),
                status: response.status().as_u16(),
            });
            return response;
        }
        let caller = match self.authenticate(&parts.headers) {
            Ok(caller) => caller,
            Err(response) => return into_response(response, None),
        };
        if parts.method == http::Method::POST && (full_path == "/api/graphql" || path == "/graphql")
        {
            let response =
                crate::graphql::execute(&self.state, &self.hits, caller.clone(), &web_url, &body);
            return into_response(response, caller.as_ref());
        }
        if parts.method == http::Method::GET
            && let Some(response) =
                handlers::releases::download(&self.state, caller.as_ref(), full_path)
        {
            return into_response(response, caller.as_ref());
        }
        let Some(method) = method(&parts.method) else {
            return into_response(ApiResponse::not_found(), caller.as_ref());
        };
        let Some(found) = self.router.find(method, path) else {
            self.record(Hit::Unrouted {
                method: parts.method.to_string(),
                path: path.to_owned(),
            });
            return into_response(ApiResponse::not_found(), caller.as_ref());
        };
        let operation = found.info.id;
        let served = |response: Response| {
            self.record(Hit::Rest {
                operation,
                status: response.status().as_u16(),
            });
            response
        };
        let Some(handler) = handlers::handler(found.info) else {
            let response = ApiResponse::error(
                501,
                &format!("Not implemented by ninot-github: {}", found.info.id),
            );
            return served(into_response(response, caller.as_ref()));
        };
        if let Err(refusal) = validate::check_request(found.info, &body) {
            return served(into_response(refusal, caller.as_ref()));
        }
        let request = ApiRequest {
            path: path.to_owned(),
            params: found.params,
            query: parts
                .uri
                .query()
                .map(|query| {
                    url::form_urlencoded::parse(query.as_bytes())
                        .into_owned()
                        .collect()
                })
                .unwrap_or_default(),
            headers: parts.headers,
            body,
            caller,
            client: Some(client.clone()),
            api_url,
            web_url,
        };
        let response = handler(&self.state, &request).unwrap_or_else(|error| error);
        if self.validate_responses
            && let Body::Json(body) = &response.body
            && let Err(mismatch) = validate::check(found.info, response.status, body)
        {
            eprintln!("ninot-github: {mismatch}");
            let message = format!(
                "ninot-github returned a response that does not match the spec: {mismatch}"
            );
            return served(into_response(
                ApiResponse::error(500, &message),
                request.caller.as_ref(),
            ));
        }
        served(into_response(response, request.caller.as_ref()))
    }

    fn record(&self, hit: Hit) {
        self.hits.lock().unwrap().insert(hit);
    }

    /// Serves git's smart HTTP protocol, enforcing the repository's visibility and permissions.
    fn serve_git(&self, parts: &Parts, body: &bytes::Bytes, path: &git::GitPath) -> Response {
        let query = parts.uri.query().unwrap_or("");
        let push = git::is_push(path.service_path, query);
        let caller = self.authenticate(&parts.headers).ok().flatten();
        let resolved = {
            let state = lock(&self.state);
            let target = match state.permission(caller.as_ref(), path.owner, path.repo) {
                Some(_) => Some((path.owner.to_owned(), path.repo.to_owned())),
                None => state.redirect(path.owner, path.repo).cloned(),
            };
            target.and_then(|(owner, name)| {
                let permission = state.permission(caller.as_ref(), &owner, &name)?;
                let git_dir = state.account(&owner)?.repo(&name)?.git_dir.clone()?;
                Some((owner, name, permission, git_dir))
            })
        };
        let denied = |status: u16, text: &str| {
            if caller.is_none() {
                git_text(401, "Authentication required\n")
                    .with_header("www-authenticate", "Basic realm=\"GitHub\"".to_owned())
                    .into_axum()
            } else {
                git_text(status, text).into_axum()
            }
        };
        let (owner, name, git_dir) = match resolved {
            Some((owner, name, permission, git_dir)) if !push || permission.push => {
                (owner, name, git_dir)
            }
            Some(_) => return denied(403, "Permission denied\n"),
            None => return denied(404, "Repository not found.\n"),
        };
        let header = |name: &str| {
            parts
                .headers
                .get(name)
                .and_then(|value| value.to_str().ok())
        };
        let request = git::CgiRequest {
            method: parts.method.as_str(),
            service_path: path.service_path,
            query,
            content_type: header("content-type"),
            content_encoding: header("content-encoding"),
            git_protocol: header("git-protocol"),
            remote_user: caller.as_ref().map(|caller| caller.login.as_str()),
            export_all: true,
            body,
        };
        let receive = parts.method == http::Method::POST && path.service_path == "git-receive-pack";
        let before = receive.then(|| ref_snapshot(&git_dir));
        let response = match git::http_backend(&git_dir, &request) {
            Ok(response) => response,
            Err(error) => return git_text(500, &format!("{error}\n")).into_axum(),
        };
        if let Some(before) = before.filter(|_| response.status == 200) {
            let after = ref_snapshot(&git_dir);
            let host = parts
                .headers
                .get(http::header::HOST)
                .and_then(|value| value.to_str().ok())
                .unwrap_or("localhost");
            let urls = ApiRequest::for_urls(&format!("{}://{host}", self.scheme), caller.clone());
            let mut state = lock(&self.state);
            let pusher = caller
                .as_ref()
                .and_then(|caller| state.account(&caller.login))
                .map(|account| crate::render::simple_user(&urls, account));
            if let Some(repo) = state
                .account_mut(&owner)
                .and_then(|account| account.repo_mut(&name))
            {
                repo.pushed_at = Some(crate::state::now());
                if let Some(branch) = first_default_branch(&repo.default_branch, &before, &after)
                    && let Err(error) = repo.set_default_branch(&branch)
                {
                    eprintln!("ninot-github: cannot set the default branch to {branch}: {error}");
                }
            }
            if let Some(pusher) = pusher {
                let at = crate::render::timestamp(&crate::state::now());
                for (name_of_ref, sha) in &after {
                    let Some(branch) = name_of_ref.strip_prefix("refs/heads/") else {
                        continue;
                    };
                    if before.get(name_of_ref) == Some(sha) {
                        continue;
                    }
                    if let Err(error) = crate::handlers::timeline::branch_moved(
                        &mut state, &owner, &name, branch, sha, at, &pusher,
                    ) {
                        eprintln!("ninot-github: cannot move pull requests on {branch}: {error}");
                    }
                }
            }
            drop(state);
            webhooks::enqueue_push(&self.state, &urls, &owner, &name, &git_dir, &before, &after);
        }
        let mut reply = ApiResponse::bytes(response.status, "", response.body);
        reply.headers = response.headers;
        into_response(reply, None)
    }

    /// Resolves the `Authorization` header; an unknown token is a 401.
    fn authenticate(&self, headers: &HeaderMap) -> Result<Option<Caller>, ApiResponse> {
        if !headers.contains_key(http::header::AUTHORIZATION) {
            return Ok(None);
        }
        let state = lock(&self.state);
        let token = token_from_authorization(headers).unwrap_or_default();
        if let Some(caller) = state.tokens.get(&token) {
            return Ok(Some(caller.clone()));
        }
        let is_jwt = headers
            .get(http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .is_some_and(|value| {
                let mut parts = value.split('.');
                parts.next().is_some_and(|part| !part.is_empty())
                    && parts.next().is_some_and(|part| !part.is_empty())
                    && parts.next().is_some_and(|part| !part.is_empty())
                    && parts.next().is_none()
            });
        if is_jwt {
            return Ok(Some(Caller {
                login: "github-app-jwt".to_owned(),
                scopes: vec!["app".to_owned()],
            }));
        }
        Err(ApiResponse::error(401, "Bad credentials"))
    }
}

/// Extracts the token from `token X`, `Bearer X` or Basic auth with the token as password.
pub(crate) fn token_from_authorization(headers: &HeaderMap) -> Option<String> {
    if let Some(token) = headers.typed_get::<Authorization<Token>>() {
        return Some(token.0.0);
    }
    if let Some(token) = headers.typed_get::<Authorization<Bearer>>() {
        return Some(token.0.token().to_owned());
    }
    Some(
        headers
            .typed_get::<Authorization<Basic>>()?
            .0
            .password()
            .to_owned(),
    )
}

/// GitHub's own `token X` authorization scheme, which `headers` has no type for.
struct Token(String);

impl Credentials for Token {
    const SCHEME: &'static str = "token";

    fn decode(value: &HeaderValue) -> Option<Self> {
        let token = std::str::from_utf8(value.as_bytes().get(Self::SCHEME.len() + 1..)?).ok()?;
        Some(Token(token.trim().to_owned()))
    }

    fn encode(&self) -> HeaderValue {
        let value = format!("{} {}", Self::SCHEME, self.0);
        // Tokens are printable ASCII; one the fake cannot encode as a header value it does
        // not send.
        HeaderValue::from_str(&value).unwrap_or_else(|_| HeaderValue::from_static(Self::SCHEME))
    }
}

fn method(method: &http::Method) -> Option<Method> {
    Some(match *method {
        http::Method::GET => Method::Get,
        http::Method::POST => Method::Post,
        http::Method::PUT => Method::Put,
        http::Method::PATCH => Method::Patch,
        http::Method::DELETE => Method::Delete,
        _ => return None,
    })
}

/// Every ref of a bare repository and the sha it points at.
fn ref_snapshot(git_dir: &std::path::Path) -> std::collections::BTreeMap<String, String> {
    git::refs(git_dir, "refs/")
        .unwrap_or_default()
        .into_iter()
        .map(|found| (found.name, found.sha))
        .collect()
}

/// The default branch a push into a repository without branches establishes: the current
/// default, then `main`, then `master`, then the first pushed branch by name. `None` when the
/// repository already had branches or the push created none.
fn first_default_branch(
    current: &str,
    before: &std::collections::BTreeMap<String, String>,
    after: &std::collections::BTreeMap<String, String>,
) -> Option<String> {
    if before.keys().any(|name| name.starts_with("refs/heads/")) {
        return None;
    }
    let pushed: Vec<&str> = after
        .keys()
        .filter_map(|name| name.strip_prefix("refs/heads/"))
        .collect();
    [current, "main", "master"]
        .into_iter()
        .find(|branch| pushed.contains(branch))
        .or_else(|| pushed.first().copied())
        .map(str::to_owned)
}

fn git_text(status: u16, text: &str) -> ApiResponse {
    ApiResponse::bytes(status, "text/plain", text.as_bytes().to_vec())
}

impl ApiResponse {
    fn into_axum(self) -> Response {
        into_response(self, None)
    }
}

fn into_response(response: ApiResponse, caller: Option<&Caller>) -> Response {
    let mut builder = Response::builder()
        .status(StatusCode::from_u16(response.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR));
    let headers = builder.headers_mut().unwrap();
    if let Some(caller) = caller {
        headers.insert(
            "x-oauth-scopes",
            HeaderValue::from_str(&caller.scopes.join(", ")).unwrap(),
        );
    }
    let body = match response.body {
        Body::Empty => AxumBody::empty(),
        Body::Json(value) => {
            headers.insert(
                http::header::CONTENT_TYPE,
                HeaderValue::from_static("application/json; charset=utf-8"),
            );
            AxumBody::from(serde_json::to_vec(&value).unwrap())
        }
        Body::Bytes(bytes) => AxumBody::from(bytes),
    };
    for (name, value) in response.headers {
        if let (Ok(name), Ok(value)) = (
            http::HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_str(&value),
        ) {
            headers.insert(name, value);
        }
    }
    builder.body(body).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_succeed_after_a_panic_while_holding_the_state() {
        let server = Server::start().unwrap();
        let state = server.app.state.clone();
        std::thread::spawn(move || {
            let _held = state.lock();
            panic!("panic while holding the state");
        })
        .join()
        .unwrap_err();
        assert!(server.app.state.is_poisoned());
        assert!(lock(&server.app.state).account(ADMIN_LOGIN).is_some());
        let token = server.add_user("alice").add_token(&[]);
        let response = reqwest::blocking::Client::new()
            .get(format!("{}/user", server.url()))
            .header("Authorization", format!("token {token}"))
            .send()
            .unwrap();
        assert_eq!(response.status(), 200);
        let body: serde_json::Value = response.json().unwrap();
        assert_eq!(body["login"], "alice");
    }
}
