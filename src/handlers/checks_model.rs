//! Check runs and suites shared by the Checks handlers: git heads, app identity, and rollup.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use ninot_common::git;

use super::repos::Found;
use crate::api::{ApiRequest, ApiResponse};
use crate::render;
use crate::state::{Account, CheckSuiteRecord, Repo, State};
use crate::types;

const ZERO_SHA: &str = "0000000000000000000000000000000000000000";
const MAX_SAME_NAME: usize = 1000;

struct Head {
    sha: String,
    branch: Option<String>,
    before: String,
    after: String,
}

/// Commit SHA `reference` names, or `reference` itself when git cannot resolve it.
pub(crate) fn commit_key(repo: &Repo, reference: &str) -> String {
    repo.git_dir
        .as_deref()
        .and_then(|git_dir| git::resolve_commit(git_dir, reference).ok().flatten())
        .unwrap_or_else(|| reference.to_owned())
}

/// The caller must be the GitHub App installed on this repository.
pub(crate) fn require_checks_app(request: &ApiRequest) -> Result<i64, ApiResponse> {
    let caller = request.require_caller()?;
    let wanted = format!(
        "installation:{}/{}",
        request.param("owner"),
        request.param("repo")
    );
    let installed = caller
        .scopes
        .iter()
        .any(|scope| scope.eq_ignore_ascii_case(&wanted));
    if !installed {
        return Err(checks_forbidden(request));
    }
    Ok(caller
        .scopes
        .iter()
        .find_map(|scope| {
            scope
                .strip_prefix("github-app:")
                .and_then(|id| id.parse().ok())
        })
        .unwrap_or(1))
}

/// 403 for a token that is not the GitHub App allowed to write this check.
pub(crate) fn checks_forbidden(request: &ApiRequest) -> ApiResponse {
    let integration = request.caller.as_ref().is_some_and(|caller| {
        caller
            .scopes
            .iter()
            .any(|scope| scope.starts_with("installation:"))
    });
    ApiResponse::error(
        403,
        if integration {
            "Resource not accessible by integration"
        } else {
            "Resource not accessible by personal access token"
        },
    )
}

/// GitHub App embedded on check runs and check suites.
pub(crate) fn integration(
    request: &ApiRequest,
    account: &Account,
    app_id: i64,
    now: DateTime<Utc>,
) -> types::NullableIntegration {
    let slug = if app_id == 1 {
        "ninot-test".to_owned()
    } else {
        format!("app-{app_id}")
    };
    let html = format!("{}/apps/{slug}", request.web_url);
    types::NullableIntegration {
        client_id: None,
        created_at: now,
        description: Some("Checks".to_owned()),
        events: Vec::new(),
        external_url: html.clone(),
        html_url: html,
        id: app_id,
        installations_count: None,
        name: slug.clone(),
        node_id: render::node_id("Integration", u64::try_from(app_id).unwrap_or(0)),
        owner: types::NullableIntegrationOwner::SimpleUser(render::simple_user(request, account)),
        permissions: types::NullableIntegrationPermissions {
            checks: Some("write".to_owned()),
            contents: None,
            deployments: None,
            issues: None,
            metadata: Some("read".to_owned()),
            extra: std::collections::HashMap::new(),
        },
        slug: Some(slug),
        updated_at: now,
    }
}

/// Finds the app's suite for `reference`, creating one when this app has none for that commit.
pub(crate) fn open_suite(
    state: &mut State,
    found: &Found,
    app_id: i64,
    reference: &str,
) -> (i64, bool) {
    let head = snapshot(repo(state, found), reference);
    if let Some(id) = repo(state, found)
        .check_suites
        .values()
        .find(|suite| suite.app_id == app_id && suite.head_sha == head.sha)
        .map(|suite| suite.id)
    {
        return (id, false);
    }
    let raw = state.next_id();
    let id = render::id(raw);
    let now = render::timestamp(&crate::time::now());
    repo_mut(state, found).check_suites.insert(
        id,
        CheckSuiteRecord {
            id,
            app_id,
            head_sha: head.sha,
            head_branch: head.branch,
            before: head.before,
            after: head.after,
            created_at: now,
            updated_at: now,
            rerequested: false,
        },
    );
    (id, true)
}

/// A run in the suite changed, so the suite is no longer in the rerequested state.
pub(crate) fn touch_suite(repo: &mut Repo, suite_id: i64) {
    if let Some(suite) = repo.check_suites.get_mut(&suite_id) {
        suite.rerequested = false;
        suite.updated_at = render::timestamp(&crate::time::now());
    }
}

/// Resets a suite to queued with no conclusion. The check runs themselves stay as they are.
pub(crate) fn mark_rerequested(repo: &mut Repo, suite_id: i64) -> bool {
    let Some(suite) = repo.check_suites.get_mut(&suite_id) else {
        return false;
    };
    suite.rerequested = true;
    suite.updated_at = render::timestamp(&crate::time::now());
    true
}

/// Drops the oldest runs once one suite holds 1000 runs of `name`, making room for one more.
pub(crate) fn cap_same_name(repo: &mut Repo, suite_id: i64, name: &str) {
    let mut ids = Vec::new();
    for run in repo.check_runs.values().flatten() {
        if run.name == name
            && run
                .check_suite
                .as_ref()
                .is_some_and(|suite| suite.id == suite_id)
        {
            ids.push(run.id);
        }
    }
    if ids.len() < MAX_SAME_NAME {
        return;
    }
    ids.sort_unstable();
    let drop_count = ids.len() + 1 - MAX_SAME_NAME;
    let drop = ids[..drop_count].to_vec();
    for runs in repo.check_runs.values_mut() {
        runs.retain(|run| !drop.contains(&run.id));
    }
    for id in drop {
        repo.check_annotations.remove(&id);
    }
}

/// Check runs that belong to `suite_id`, oldest id first.
pub(crate) fn suite_runs(repo: &Repo, suite_id: i64) -> Vec<types::CheckRun> {
    let mut runs = repo
        .check_runs
        .values()
        .flatten()
        .filter(|run| {
            run.check_suite
                .as_ref()
                .is_some_and(|suite| suite.id == suite_id)
        })
        .cloned()
        .collect::<Vec<_>>();
    runs.sort_by_key(|run| run.id);
    runs
}

/// Applies `filter`, `check_name`, `status`, and `app_id`, then orders by id.
pub(crate) fn select_runs(
    mut runs: Vec<types::CheckRun>,
    request: &ApiRequest,
) -> Vec<types::CheckRun> {
    if request.query("filter") != Some("all") {
        runs = latest_of(runs);
    }
    if let Some(name) = request.query("check_name") {
        runs.retain(|run| run.name == name);
    }
    if let Some(status) = request.query("status") {
        runs.retain(|run| run.status.to_string() == status);
    }
    if let Some(app) = request.query("app_id") {
        let Ok(app_id) = app.parse::<i64>() else {
            return Vec::new();
        };
        runs.retain(|run| run.app.as_ref().is_some_and(|app| app.id == app_id));
    }
    runs.sort_by_key(|run| run.id);
    runs
}

/// Renders one stored suite, including the status rolled up from its latest runs.
pub(crate) fn render_suite(
    state: &State,
    request: &ApiRequest,
    found: &Found,
    suite_id: i64,
) -> Result<types::CheckSuite, ApiResponse> {
    let account = state
        .account(&found.owner)
        .expect("repository owner exists");
    let repo = account.repo(&found.name).expect("repository exists");
    let suite = repo
        .check_suites
        .get(&suite_id)
        .cloned()
        .ok_or_else(ApiResponse::not_found)?;
    let runs = suite_runs(repo, suite_id);
    let latest = latest_of(runs.clone());
    let (status, conclusion) = rollup(&latest, suite.rerequested);
    let installed = app_installed(state, &found.owner, &found.name, suite.app_id);
    let api = format!("{}/repos/{}/{}", request.api_url, found.owner, found.name);
    let now = suite.created_at;
    Ok(types::CheckSuite {
        after: Some(suite.after),
        app: Some(integration(request, account, suite.app_id, now)),
        before: Some(suite.before),
        check_runs_url: format!("{api}/check-suites/{suite_id}/check-runs"),
        conclusion,
        created_at: Some(suite.created_at),
        head_branch: suite.head_branch,
        head_commit: simple_commit(repo, &suite.head_sha, now),
        head_sha: suite.head_sha,
        id: suite.id,
        latest_check_runs_count: i64::try_from(latest.len()).unwrap_or(i64::MAX),
        node_id: render::node_id("CheckSuite", u64::try_from(suite.id).unwrap_or(0)),
        pull_requests: Some(Vec::new()),
        repository: render::minimal_repository(request, account, repo, found.permission),
        rerequestable: Some(installed),
        runs_rerequestable: Some(installed && !runs.is_empty()),
        status: Some(status),
        updated_at: Some(suite.updated_at),
        url: Some(format!("{api}/check-suites/{suite_id}")),
    })
}

fn repo<'a>(state: &'a State, found: &Found) -> &'a Repo {
    state
        .account(&found.owner)
        .and_then(|account| account.repo(&found.name))
        .expect("repository exists")
}

fn repo_mut<'a>(state: &'a mut State, found: &Found) -> &'a mut Repo {
    state
        .account_mut(&found.owner)
        .and_then(|account| account.repo_mut(&found.name))
        .expect("repository exists")
}

fn snapshot(repo: &Repo, reference: &str) -> Head {
    let sha = commit_key(repo, reference);
    let branch = branch_of(repo, reference, &sha);
    let before = first_parent(repo, &sha).unwrap_or_else(|| ZERO_SHA.to_owned());
    Head {
        after: sha.clone(),
        sha,
        branch,
        before,
    }
}

fn branch_of(repo: &Repo, reference: &str, sha: &str) -> Option<String> {
    let short = reference.strip_prefix("refs/heads/").unwrap_or(reference);
    if short != sha && !looks_like_sha(short) && resolves_to(repo, short, sha) {
        return Some(short.to_owned());
    }
    let git_dir = repo.git_dir.as_deref()?;
    let heads = git::refs(git_dir, "refs/heads/").ok()?;
    let default = format!("refs/heads/{}", repo.default_branch);
    if heads
        .iter()
        .any(|head| head.name == default && head.sha == sha)
    {
        return Some(repo.default_branch.clone());
    }
    heads
        .into_iter()
        .find(|head| head.sha == sha)
        .map(|head| head.name.trim_start_matches("refs/heads/").to_owned())
}

fn looks_like_sha(value: &str) -> bool {
    value.len() >= 7 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn resolves_to(repo: &Repo, reference: &str, sha: &str) -> bool {
    repo.git_dir
        .as_deref()
        .and_then(|git_dir| git::resolve_commit(git_dir, reference).ok().flatten())
        .is_some_and(|resolved| resolved == sha)
}

fn first_parent(repo: &Repo, sha: &str) -> Option<String> {
    let git_dir = repo.git_dir.as_deref()?;
    git::commit(git_dir, sha).ok()?.parents.into_iter().next()
}

fn simple_commit(repo: &Repo, sha: &str, fallback: DateTime<Utc>) -> types::SimpleCommit {
    if let Some(git_dir) = repo.git_dir.as_deref()
        && let Ok(commit) = git::commit(git_dir, sha)
    {
        return types::SimpleCommit {
            author: git_author(&commit.author),
            committer: git_committer(&commit.committer),
            id: commit.sha,
            message: commit.message,
            timestamp: git_time(&commit.author.date, fallback),
            tree_id: commit.tree,
        };
    }
    types::SimpleCommit {
        author: None,
        committer: None,
        id: sha.to_owned(),
        message: String::new(),
        timestamp: fallback,
        tree_id: sha.to_owned(),
    }
}

fn git_author(signature: &git::Signature) -> Option<types::SimpleCommitAuthor> {
    dotted_email(&signature.email).then(|| types::SimpleCommitAuthor {
        email: signature.email.clone(),
        name: signature.name.clone(),
    })
}

fn git_committer(signature: &git::Signature) -> Option<types::SimpleCommitCommitter> {
    dotted_email(&signature.email).then(|| types::SimpleCommitCommitter {
        email: signature.email.clone(),
        name: signature.name.clone(),
    })
}

fn dotted_email(email: &str) -> bool {
    let Some((local, domain)) = email.rsplit_once('@') else {
        return false;
    };
    !local.is_empty() && domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.')
}

fn git_time(text: &str, fallback: DateTime<Utc>) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(text)
        .map(|time| time.with_timezone(&Utc))
        .unwrap_or(fallback)
}

fn app_installed(state: &State, owner: &str, name: &str, app_id: i64) -> bool {
    state.installations.values().any(|installation| {
        installation.owner.eq_ignore_ascii_case(owner)
            && installation.repository.eq_ignore_ascii_case(name)
            && i64::try_from(installation.app_id).ok() == Some(app_id)
    })
}

fn latest_of(runs: Vec<types::CheckRun>) -> Vec<types::CheckRun> {
    let mut best: BTreeMap<String, types::CheckRun> = BTreeMap::new();
    for run in runs {
        let replace = best
            .get(&run.name)
            .is_none_or(|current| recency(&run) > recency(current));
        if replace {
            best.insert(run.name.clone(), run);
        }
    }
    best.into_values().collect()
}

fn recency(run: &types::CheckRun) -> (DateTime<Utc>, i64) {
    (
        run.completed_at
            .or(run.started_at)
            .unwrap_or(DateTime::UNIX_EPOCH),
        run.id,
    )
}

fn rollup(
    latest: &[types::CheckRun],
    rerequested: bool,
) -> (types::CheckSuiteStatus, Option<types::CheckSuiteConclusion>) {
    if rerequested
        || latest.is_empty()
        || latest
            .iter()
            .all(|run| run.status == types::CheckRunStatus::Queued)
    {
        return (types::CheckSuiteStatus::Queued, None);
    }
    if latest
        .iter()
        .all(|run| run.status == types::CheckRunStatus::Completed)
    {
        return (
            types::CheckSuiteStatus::Completed,
            Some(rollup_conclusion(latest)),
        );
    }
    (types::CheckSuiteStatus::InProgress, None)
}

fn rollup_conclusion(runs: &[types::CheckRun]) -> types::CheckSuiteConclusion {
    let conclusions: Vec<_> = runs.iter().filter_map(|run| run.conclusion).collect();
    for kind in [
        types::CheckRunConclusion::ActionRequired,
        types::CheckRunConclusion::Cancelled,
        types::CheckRunConclusion::TimedOut,
        types::CheckRunConclusion::Failure,
    ] {
        if conclusions.contains(&kind) {
            return map_conclusion(kind);
        }
    }
    if conclusions.contains(&types::CheckRunConclusion::Success) {
        return types::CheckSuiteConclusion::Success;
    }
    if !conclusions.is_empty()
        && conclusions
            .iter()
            .all(|conclusion| *conclusion == types::CheckRunConclusion::Skipped)
    {
        return types::CheckSuiteConclusion::Skipped;
    }
    types::CheckSuiteConclusion::Neutral
}

fn map_conclusion(value: types::CheckRunConclusion) -> types::CheckSuiteConclusion {
    match value {
        types::CheckRunConclusion::Success => types::CheckSuiteConclusion::Success,
        types::CheckRunConclusion::Failure => types::CheckSuiteConclusion::Failure,
        types::CheckRunConclusion::Neutral => types::CheckSuiteConclusion::Neutral,
        types::CheckRunConclusion::Cancelled => types::CheckSuiteConclusion::Cancelled,
        types::CheckRunConclusion::Skipped => types::CheckSuiteConclusion::Skipped,
        types::CheckRunConclusion::TimedOut => types::CheckSuiteConclusion::TimedOut,
        types::CheckRunConclusion::ActionRequired => types::CheckSuiteConclusion::ActionRequired,
    }
}
