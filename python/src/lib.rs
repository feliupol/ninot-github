//! Thin Python bindings for Ninot GitHub's Rust-backed fixture resources.

#![allow(clippy::useless_conversion)]

use std::collections::BTreeMap;

use ninot_github::faults::Rule;
use ninot_github::fixtures;
use ninot_github::server::{
    ActionsSecret as RustActionsSecret, Commit as RustCommit, Issue as RustIssue, Options,
    Project as RustProject, PullRequest as RustPullRequest, Repository as RustRepository,
    Review as RustReview, Server as RustServer, WorkflowRun as RustWorkflowRun,
};
use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use serde::de::DeserializeOwned;

/// A running GitHub HTTP and Git server.
#[pyclass]
pub struct Server {
    inner: Option<RustServer>,
}

/// A repository that owns its issue, pull-request, commit, workflow and project fixtures.
#[pyclass]
pub struct Repository {
    inner: RustRepository,
}

/// An issue that owns its comments.
#[pyclass]
pub struct Issue {
    inner: RustIssue,
}

/// A pull request that owns its reviews and comments.
#[pyclass]
pub struct PullRequest {
    inner: RustPullRequest,
}

/// A review that owns its pull request fixture.
#[pyclass]
pub struct Review {
    inner: RustReview,
}

/// A workflow run that owns its job logs.
#[pyclass]
pub struct WorkflowRun {
    inner: RustWorkflowRun,
}

/// Encrypted Actions secret fixture owned by a repository.
#[pyclass]
pub struct ActionsSecret {
    inner: RustActionsSecret,
}

/// A Git commit that owns its status and check-run fixtures.
#[pyclass]
pub struct Commit {
    inner: RustCommit,
}

/// A GitHub Projects v2 board that owns its fields and items.
#[pyclass]
pub struct Project {
    inner: RustProject,
}

#[pymethods]
impl Server {
    /// Start a local GitHub-compatible HTTP server.
    #[new]
    #[pyo3(signature = (tls = false))]
    fn new(tls: bool) -> PyResult<Self> {
        let options = Options {
            tls,
            ..Default::default()
        };
        let inner = RustServer::start_with(options)
            .map_err(|error| PyRuntimeError::new_err(error.to_string()))?;
        Ok(Self { inner: Some(inner) })
    }

    /// The API base URL.
    #[getter]
    fn url(&self) -> PyResult<&str> {
        self.inner
            .as_ref()
            .map(RustServer::url)
            .ok_or_else(|| PyRuntimeError::new_err("server was stopped"))
    }

    /// Token for the built-in administrator account.
    #[getter]
    fn admin_token(&self) -> PyResult<&str> {
        self.inner
            .as_ref()
            .map(RustServer::admin_token)
            .ok_or_else(|| PyRuntimeError::new_err("server was stopped"))
    }

    /// Path to the local certificate authority, or `None` when TLS is disabled.
    #[getter]
    fn ca_cert_path(&self) -> PyResult<Option<String>> {
        self.inner
            .as_ref()
            .map(|server| {
                server
                    .ca_cert_path()
                    .map(|path| path.to_string_lossy().to_string())
            })
            .ok_or_else(|| PyRuntimeError::new_err("server was stopped"))
    }

    /// Adds or updates a user account.
    #[pyo3(signature = (login, name = None))]
    fn add_user(&self, login: &str, name: Option<String>) -> PyResult<()> {
        let server = self.server()?;
        server.add_user(login);
        if name.is_some() {
            server.set_user_name(login, name);
        }
        Ok(())
    }

    /// Adds an organization whose members administer its repositories.
    fn add_org(&self, login: &str, members: Vec<String>) -> PyResult<()> {
        let member_refs = members.iter().map(String::as_str).collect::<Vec<_>>();
        self.server()?.add_org(login, &member_refs);
        Ok(())
    }

    /// Adds members to an organization's team, creating both if needed.
    fn add_team(&self, org: &str, slug: &str, members: Vec<String>) -> PyResult<()> {
        let member_refs = members.iter().map(String::as_str).collect::<Vec<_>>();
        self.server()?
            .add_team(org, slug, &member_refs)
            .map_err(|error| PyValueError::new_err(error.to_string()))
    }

    /// Creates an empty repository and returns the handle that owns its fixtures.
    #[pyo3(signature = (owner, name, private = false, *, description = None, language = None, default_branch = "main"))]
    fn create_repository(
        &self,
        owner: &str,
        name: &str,
        private: bool,
        description: Option<String>,
        language: Option<String>,
        default_branch: &str,
    ) -> PyResult<Repository> {
        let inner = self
            .server()?
            .create_repository(owner, name, private, description, language, default_branch)
            .map_err(|error| PyRuntimeError::new_err(error.to_string()))?;
        Ok(Repository { inner })
    }

    /// Returns a handle for an existing repository, or `None` when it is absent.
    fn repository(&self, owner: &str, name: &str) -> PyResult<Option<Repository>> {
        Ok(self
            .server()?
            .repository(owner, name)
            .map(|inner| Repository { inner }))
    }

    /// Issues a token acting as an existing user.
    fn add_token(&self, login: &str, scopes: Vec<String>) -> PyResult<String> {
        let scope_refs = scopes.iter().map(String::as_str).collect::<Vec<_>>();
        Ok(self.server()?.add_token(login, &scope_refs))
    }

    /// Installs a typed fault rule in the HTTP path.
    fn add_fault(&self, rule: &Bound<'_, PyAny>) -> PyResult<()> {
        let rule: Rule = extract_seed(rule.py(), rule)?;
        self.server()?.add_fault(rule);
        Ok(())
    }

    /// Clears configured fault rules and their history.
    fn clear_faults(&self) -> PyResult<()> {
        self.server()?.clear_faults();
        Ok(())
    }

    /// Resets the random sequence used by probabilistic fault rules.
    fn seed_faults(&self, seed: u64) -> PyResult<()> {
        self.server()?.seed_faults(seed);
        Ok(())
    }

    /// Returns fault applications in request order as typed mapping-shaped records.
    fn applied_faults<'py>(&self, py: Python<'py>) -> PyResult<Vec<Bound<'py, PyAny>>> {
        let json = pyo3::types::PyModule::import_bound(py, "json")?;
        self.server()?
            .applied_faults()
            .into_iter()
            .map(|fault| {
                let encoded = serde_json::to_string(&fault)
                    .map_err(|error| PyRuntimeError::new_err(error.to_string()))?;
                json.call_method1("loads", (encoded,))
            })
            .collect()
    }

    /// Advances the rate-limit clock without waiting.
    fn advance_clock(&self, milliseconds: u64) -> PyResult<()> {
        self.server()?
            .advance_clock(std::time::Duration::from_millis(milliseconds));
        Ok(())
    }

    /// Starts recording HTTP requests.
    fn start_profile(&self) -> PyResult<()> {
        self.server()?.start_profile();
        Ok(())
    }

    /// Whether HTTP profiling is active.
    #[getter]
    fn is_recording(&self) -> PyResult<bool> {
        Ok(self.server()?.is_recording())
    }

    /// Stops profiling and returns request, route, byte-count and timing records.
    fn stop_profile<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let profile = self.server()?.stop_profile();
        let hits = profile
            .hits()
            .iter()
            .map(|hit| {
                serde_json::json!({
                    "method": hit.method, "path": hit.path, "query": hit.query,
                    "request": hit.request(), "label": hit.label, "status": hit.status,
                    "request_bytes": hit.request_bytes, "response_bytes": hit.response_bytes,
                    "duration_ms": hit.duration.as_millis()
                })
            })
            .collect::<Vec<_>>();
        let labels = profile
            .by_label()
            .iter()
            .map(|stats| {
                serde_json::json!({
                    "label": stats.label, "count": stats.count, "total_ms": stats.total.as_millis()
                })
            })
            .collect::<Vec<_>>();
        let repeats = profile
            .repeats()
            .iter()
            .map(|repeat| {
                serde_json::json!({
                    "request": repeat.request, "count": repeat.count
                })
            })
            .collect::<Vec<_>>();
        as_python_value(
            py,
            &serde_json::json!({
                "requests": hits.len(), "hits": hits, "labels": labels, "repeats": repeats
            }),
        )
    }

    /// Stops the server.
    fn stop(&mut self) {
        self.inner = None;
    }

    /// Enters a server context.
    fn __enter__(slf: PyRef<Self>) -> PyRef<Self> {
        slf
    }

    /// Stops the server when leaving its context.
    #[pyo3(signature = (_exc_type=None, _exc_val=None, _exc_tb=None))]
    fn __exit__(
        &mut self,
        _exc_type: Option<&Bound<PyAny>>,
        _exc_val: Option<&Bound<PyAny>>,
        _exc_tb: Option<&Bound<PyAny>>,
    ) {
        self.stop();
    }
}

impl Server {
    fn server(&self) -> PyResult<&RustServer> {
        self.inner
            .as_ref()
            .ok_or_else(|| PyRuntimeError::new_err("server was stopped"))
    }
}

#[pymethods]
impl Repository {
    /// Repository owner login.
    #[getter]
    fn owner(&self) -> &str {
        self.inner.owner()
    }

    /// Repository name.
    #[getter]
    fn name(&self) -> &str {
        self.inner.name()
    }

    /// Returns a handle for an existing issue, or `None` when it is absent.
    fn issue(&self, number: u64) -> PyResult<Option<Issue>> {
        Ok(self
            .inner
            .issue(number)
            .map_err(|error| PyRuntimeError::new_err(error.to_string()))?
            .map(|inner| Issue { inner }))
    }

    /// Returns a handle for an existing pull request, or `None` when it is absent.
    fn pull_request(&self, number: u64) -> PyResult<Option<PullRequest>> {
        Ok(self
            .inner
            .pull_request(number)
            .map_err(|error| PyRuntimeError::new_err(error.to_string()))?
            .map(|inner| PullRequest { inner }))
    }

    /// Commits bytes to a branch and returns the resulting Git commit handle.
    ///
    /// `author` and `committer` are `GitIdentitySeed` values; a missing committer is the author.
    /// Open pull requests whose head is `branch` move to the new commit.
    #[pyo3(signature = (branch, path, content, message, *, author=None, committer=None))]
    fn commit_file(
        &self,
        branch: &str,
        path: &str,
        content: &[u8],
        message: &str,
        author: Option<&Bound<'_, PyAny>>,
        committer: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Commit> {
        let author = author
            .map(|seed| extract_seed::<fixtures::GitIdentitySeed>(seed.py(), seed))
            .transpose()?;
        let committer = committer
            .map(|seed| extract_seed::<fixtures::GitIdentitySeed>(seed.py(), seed))
            .transpose()?;
        let inner = self
            .inner
            .commit_file_by(
                branch,
                std::path::Path::new(path),
                content,
                message,
                author.as_ref(),
                committer.as_ref(),
            )
            .map_err(|error| PyRuntimeError::new_err(error.to_string()))?;
        Ok(Commit { inner })
    }

    /// Copies an upstream repository's Git refs and objects into this fork.
    fn fork_from(&self, upstream: &Repository) -> PyResult<()> {
        self.inner
            .fork_from(&upstream.inner)
            .map_err(|error| PyRuntimeError::new_err(error.to_string()))
    }

    /// Returns the fixture handle for a commit SHA in this repository.
    fn commit(&self, sha: &str) -> PyResult<Commit> {
        let inner = self
            .inner
            .commit(sha)
            .map_err(|error| PyRuntimeError::new_err(error.to_string()))?;
        Ok(Commit { inner })
    }

    /// Seeds the GitHub App installation associated with this repository.
    #[pyo3(signature = (installation_id, secrets_permission = None))]
    fn create_installation(
        &self,
        installation_id: u64,
        secrets_permission: Option<&str>,
    ) -> PyResult<()> {
        let permission = match secrets_permission {
            None => None,
            Some("read") => Some(fixtures::ActionsSecretsPermission::Read),
            Some("write") => Some(fixtures::ActionsSecretsPermission::Write),
            Some(other) => {
                return Err(PyValueError::new_err(format!(
                    "unknown Actions secrets permission {other:?}; expected 'read' or 'write'"
                )));
            }
        };
        self.inner
            .create_installation_with_secrets_permission(installation_id, permission)
            .map_err(|error| PyRuntimeError::new_err(error.to_string()))
    }

    /// Seeds the public key used for repository Actions secrets.
    fn set_actions_public_key(&self, py: Python<'_>, seed: &Bound<'_, PyAny>) -> PyResult<()> {
        let seed: fixtures::ActionsPublicKeySeed = extract_seed(py, seed)?;
        self.inner
            .set_actions_public_key(seed)
            .map_err(|error| PyValueError::new_err(error.to_string()))
    }

    /// Seeds an existing encrypted secret using this repository's public key.
    fn create_actions_secret(
        &self,
        py: Python<'_>,
        seed: &Bound<'_, PyAny>,
    ) -> PyResult<ActionsSecret> {
        let seed: fixtures::ActionsSecretSeed = extract_seed(py, seed)?;
        let inner = self
            .inner
            .create_actions_secret(seed)
            .map_err(|error| PyValueError::new_err(error.to_string()))?;
        Ok(ActionsSecret { inner })
    }

    /// Returns a fixture handle for a stored secret without exposing it through REST reads.
    fn actions_secret(&self, name: &str) -> PyResult<Option<ActionsSecret>> {
        Ok(self
            .inner
            .actions_secret(name)
            .map_err(|error| PyValueError::new_err(error.to_string()))?
            .map(|inner| ActionsSecret { inner }))
    }

    /// Controls whether pull-request auto-merge is enabled for this repository.
    fn set_auto_merge_allowed(&self, allowed: bool) -> PyResult<()> {
        self.inner
            .set_auto_merge_allowed(allowed)
            .map_err(|error| PyRuntimeError::new_err(error.to_string()))
    }

    /// Creates an issue from a sparse typed seed.
    fn create_issue(
        &self,
        py: Python<'_>,
        number: u64,
        seed: &Bound<'_, PyAny>,
    ) -> PyResult<Issue> {
        let seed: fixtures::IssueSeed = extract_seed(py, seed)?;
        let inner = self
            .inner
            .create_issue(number, seed)
            .map_err(|error| PyValueError::new_err(error.to_string()))?;
        Ok(Issue { inner })
    }

    /// Creates a pull request from a sparse typed seed.
    fn create_pull_request(
        &self,
        py: Python<'_>,
        number: u64,
        seed: &Bound<'_, PyAny>,
    ) -> PyResult<PullRequest> {
        let seed: fixtures::PullRequestSeed = extract_seed(py, seed)?;
        let inner = self
            .inner
            .create_pull_request(number, seed)
            .map_err(|error| PyValueError::new_err(error.to_string()))?;
        Ok(PullRequest { inner })
    }

    /// Creates an Actions workflow run and its jobs from typed sparse seeds.
    fn create_workflow_run(
        &self,
        py: Python<'_>,
        seed: &Bound<'_, PyAny>,
        jobs: &Bound<'_, PyAny>,
    ) -> PyResult<WorkflowRun> {
        let seed: fixtures::WorkflowRunSeed = extract_seed(py, seed)?;
        let jobs: Vec<fixtures::JobSeed> = extract_seed(py, jobs)?;
        let inner = self
            .inner
            .create_workflow_run(seed, jobs)
            .map_err(|error| PyValueError::new_err(error.to_string()))?;
        Ok(WorkflowRun { inner })
    }

    /// Creates a Projects v2 board owned by this repository.
    fn create_project(&self, title: &str) -> PyResult<Project> {
        let inner = self
            .inner
            .create_project(title)
            .map_err(|error| PyRuntimeError::new_err(error.to_string()))?;
        Ok(Project { inner })
    }
}

#[pymethods]
impl Issue {
    /// Issue number.
    #[getter]
    fn number(&self) -> u64 {
        self.inner.number()
    }

    /// Adds a comment to this issue.
    fn create_comment(&self, py: Python<'_>, seed: &Bound<'_, PyAny>) -> PyResult<()> {
        let seed = extract_seed::<fixtures::IssueCommentSeed>(py, seed)?;
        self.inner
            .create_comment(seed)
            .map_err(|error| PyValueError::new_err(error.to_string()))
    }
}

#[pymethods]
impl PullRequest {
    /// Pull-request number.
    #[getter]
    fn number(&self) -> u64 {
        self.inner.number()
    }

    /// Adds a review to this pull request.
    fn create_review(&self, py: Python<'_>, seed: &Bound<'_, PyAny>) -> PyResult<Review> {
        let seed = extract_seed::<fixtures::ReviewSeed>(py, seed)?;
        let inner = self
            .inner
            .create_review(seed)
            .map_err(|error| PyValueError::new_err(error.to_string()))?;
        Ok(Review { inner })
    }

    /// Adds an issue comment to this pull request.
    fn create_issue_comment(&self, py: Python<'_>, seed: &Bound<'_, PyAny>) -> PyResult<()> {
        let seed = extract_seed::<fixtures::IssueCommentSeed>(py, seed)?;
        self.inner
            .create_issue_comment(seed)
            .map_err(|error| PyValueError::new_err(error.to_string()))
    }

    /// Adds a review comment to this pull request.
    fn create_review_comment(&self, py: Python<'_>, seed: &Bound<'_, PyAny>) -> PyResult<()> {
        let seed = extract_seed::<fixtures::ReviewCommentSeed>(py, seed)?;
        self.inner
            .create_review_comment(seed)
            .map_err(|error| PyValueError::new_err(error.to_string()))
    }

    /// Adds a GraphQL review thread and returns its node id.
    fn create_review_thread(&self, py: Python<'_>, seed: &Bound<'_, PyAny>) -> PyResult<String> {
        let seed = extract_seed::<fixtures::ReviewThreadSeed>(py, seed)?;
        self.inner
            .create_review_thread(seed)
            .map_err(|error| PyValueError::new_err(error.to_string()))
    }

    /// Sets the explicit GraphQL review decision for this pull request.
    #[pyo3(signature = (decision=None))]
    fn set_review_decision(&self, decision: Option<String>) -> PyResult<()> {
        self.inner
            .set_review_decision(decision)
            .map_err(|error| PyRuntimeError::new_err(error.to_string()))
    }

    /// Records a timeline event and applies it to the pull request.
    fn create_timeline_event(&self, py: Python<'_>, seed: &Bound<'_, PyAny>) -> PyResult<()> {
        let seed = extract_seed::<fixtures::TimelineEventSeed>(py, seed)?;
        self.inner
            .create_timeline_event(seed)
            .map_err(|error| PyValueError::new_err(error.to_string()))
    }

    /// Moves the head branch to a commit, recording a force-push when history was rewritten.
    #[pyo3(signature = (sha, pushed_at=None, actor=None))]
    fn push_head(
        &self,
        py: Python<'_>,
        sha: &str,
        pushed_at: Option<&str>,
        actor: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<()> {
        let actor = actor
            .map(|seed| extract_seed::<fixtures::UserSeed>(py, seed))
            .transpose()?;
        self.inner
            .push_head(sha, pushed_at, actor)
            .map_err(|error| PyValueError::new_err(error.to_string()))
    }
}

#[pymethods]
impl Review {
    /// Numeric review id.
    #[getter]
    fn id(&self) -> PyResult<i64> {
        self.inner
            .id()
            .map_err(|error| PyRuntimeError::new_err(error.to_string()))
    }

    /// GitHub global node id for this review.
    #[getter]
    fn node_id(&self) -> PyResult<String> {
        self.inner
            .node_id()
            .map_err(|error| PyRuntimeError::new_err(error.to_string()))
    }
}

#[pymethods]
impl WorkflowRun {
    /// Workflow run id.
    #[getter]
    fn id(&self) -> u64 {
        self.inner.id()
    }

    /// Sets the plain-text log served for a job in this run.
    fn set_job_log(&self, job_id: u64, text: String) -> PyResult<()> {
        self.inner
            .set_job_log(job_id, text)
            .map_err(|error| PyRuntimeError::new_err(error.to_string()))
    }
}

#[pymethods]
impl ActionsSecret {
    /// Secret name.
    #[getter]
    fn name(&self) -> &str {
        self.inner.name()
    }

    /// Encrypted value retained for fixture assertions.
    #[getter]
    fn encrypted_value(&self) -> PyResult<String> {
        self.inner
            .encrypted_value()
            .map_err(|error| PyRuntimeError::new_err(error.to_string()))
    }

    /// Public-key identifier used for encryption.
    #[getter]
    fn key_id(&self) -> PyResult<String> {
        self.inner
            .key_id()
            .map_err(|error| PyRuntimeError::new_err(error.to_string()))
    }
}

#[pymethods]
impl Commit {
    /// Commit SHA.
    #[getter]
    fn sha(&self) -> &str {
        self.inner.sha()
    }

    /// Adds a check run associated with this commit.
    fn create_check_run(&self, py: Python<'_>, seed: &Bound<'_, PyAny>) -> PyResult<()> {
        let seed = extract_seed::<fixtures::CheckRunSeed>(py, seed)?;
        self.inner
            .create_check_run(seed)
            .map_err(|error| PyValueError::new_err(error.to_string()))
    }

    /// Adds a commit status associated with this commit.
    fn create_status(&self, py: Python<'_>, seed: &Bound<'_, PyAny>) -> PyResult<()> {
        let seed = extract_seed::<fixtures::CommitStatusSeed>(py, seed)?;
        self.inner
            .create_status(seed)
            .map_err(|error| PyValueError::new_err(error.to_string()))
    }
}

#[pymethods]
impl Project {
    /// Projects v2 GraphQL node id.
    #[getter]
    fn node_id(&self) -> &str {
        self.inner.node_id()
    }

    /// Adds a single-select field with its available option names.
    fn create_field(&self, name: &str, options: Vec<String>) -> PyResult<()> {
        self.inner
            .create_field(name, options)
            .map_err(|error| PyRuntimeError::new_err(error.to_string()))
    }

    /// Adds an issue-backed item, with field names mapped to selected option names.
    fn create_issue_item(
        &self,
        issue: &Issue,
        field_values: BTreeMap<String, String>,
    ) -> PyResult<String> {
        self.inner
            .create_issue_item(&issue.inner, field_values)
            .map_err(|error| PyRuntimeError::new_err(error.to_string()))
    }
}

/// Python module for Ninot GitHub.
#[pymodule]
fn _native(module: &Bound<PyModule>) -> PyResult<()> {
    module.add_class::<Server>()?;
    module.add_class::<Repository>()?;
    module.add_class::<Issue>()?;
    module.add_class::<PullRequest>()?;
    module.add_class::<Review>()?;
    module.add_class::<WorkflowRun>()?;
    module.add_class::<ActionsSecret>()?;
    module.add_class::<Commit>()?;
    module.add_class::<Project>()?;
    Ok(())
}

/// Converts a Python TypedDict into its concrete Rust seed model.
fn extract_seed<T: DeserializeOwned>(py: Python<'_>, value: &Bound<'_, PyAny>) -> PyResult<T> {
    let encoded = pyo3::types::PyModule::import_bound(py, "json")?
        .call_method1("dumps", (value,))?
        .extract::<String>()?;
    serde_json::from_str(&encoded)
        .map_err(|error| PyValueError::new_err(format!("invalid typed fixture seed: {error}")))
}

fn as_python_value<'py>(py: Python<'py>, value: &serde_json::Value) -> PyResult<Bound<'py, PyAny>> {
    let body = serde_json::to_string(value)
        .map_err(|error| PyRuntimeError::new_err(format!("cannot serialize profile: {error}")))?;
    pyo3::types::PyModule::import_bound(py, "json")?.call_method1("loads", (body,))
}
