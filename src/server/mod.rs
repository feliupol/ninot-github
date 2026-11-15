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

mod app;
mod auth;
mod seed;

use app::dispatch;
pub(crate) use auth::token_from_authorization;
pub use seed::{
    ActionsSecret, Commit, Issue, Org, Project, PullRequest, Repository, Review, Runner, Team,
    User, WorkflowJob, WorkflowRun,
};

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
    /// Which addresses a webhook delivery may contact ([`crate::TargetPolicy`]). The default,
    /// [`crate::TargetPolicy::Any`], allows every address, including loopback, so local
    /// receivers keep working.
    pub webhook_targets: webhooks::TargetPolicy,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            address: SocketAddr::from(([127, 0, 0, 1], 0)),
            validate_responses: true,
            tls: false,
            rate_limits: Limits::default(),
            webhook_targets: webhooks::TargetPolicy::Any,
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
        create_private_dir(&data_dir)?;
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
        state.webhook_targets = options.webhook_targets;
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

impl Drop for Server {
    fn drop(&mut self) {
        self.app.running.store(false, Ordering::Release);
        self.harness.stop();
        let _ = std::fs::remove_dir_all(&self.data_dir);
    }
}

/// Creates `path` (and missing parents) readable only by its owner.
fn create_private_dir(path: &std::path::Path) -> io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn private_dirs_are_owner_only() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = std::env::temp_dir().join(format!("ninot-github-mode-{}", std::process::id()));
        create_private_dir(&dir).unwrap();
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        std::fs::remove_dir(&dir).unwrap();
        assert_eq!(mode, 0o700);
    }

    #[cfg(unix)]
    #[test]
    fn the_data_directory_is_owner_only() {
        use std::os::unix::fs::PermissionsExt as _;
        let server = Server::start().unwrap();
        let mode = std::fs::metadata(&server.data_dir)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o700);
    }

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
