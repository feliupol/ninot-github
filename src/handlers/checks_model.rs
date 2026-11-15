//! Check runs and suites shared by the Checks handlers: git heads, app identity, and rollup.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use ninot_common::git;

use super::repos::Found;
use crate::api::{ApiRequest, ApiResponse};
use crate::render;
use crate::state::{Account, CheckRunApp, CheckRunRecord, CheckSuiteRecord, Repo, State};
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
// Source: <https://docs.github.com/en/rest/checks/runs#create-a-check-run>
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
// Source: <https://docs.github.com/en/rest/using-the-rest-api/troubleshooting-the-rest-api#resource-not-accessible>
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
    let html = request.urls().app_html(&slug);
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
    let head = snapshot(found.repo(state), reference);
    if let Some(id) = found
        .repo(state)
        .check_suites
        .values()
        .find(|suite| suite.app_id == app_id && suite.head_sha == head.sha)
        .map(|suite| suite.id)
    {
        return (id, false);
    }
    let raw = state.next_id();
    let id = render::id(raw);
    let now = crate::time::now_utc();
    found.repo_mut(state).check_suites.insert(
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
        suite.updated_at = crate::time::now_utc();
    }
}

/// Resets a suite to queued with no conclusion. The check runs themselves stay as they are.
// Source: <https://docs.github.com/en/rest/checks/suites#rerequest-a-check-suite>
pub(crate) fn mark_rerequested(repo: &mut Repo, suite_id: i64) -> bool {
    let Some(suite) = repo.check_suites.get_mut(&suite_id) else {
        return false;
    };
    suite.rerequested = true;
    suite.updated_at = crate::time::now_utc();
    true
}

/// Drops the oldest runs once one suite holds 1000 runs of `name`, making room for one more.
// Source: <https://docs.github.com/en/rest/checks/runs#create-a-check-run>
pub(crate) fn cap_same_name(repo: &mut Repo, suite_id: i64, name: &str) {
    let mut ids: Vec<i64> = repo
        .check_runs
        .iter()
        .filter(|run| run.name == name && run.suite_id == Some(suite_id))
        .map(|run| run.id)
        .collect();
    if ids.len() < MAX_SAME_NAME {
        return;
    }
    ids.sort_unstable();
    let drop = &ids[..ids.len() + 1 - MAX_SAME_NAME];
    repo.check_runs
        .retain(|run| run.suite_id != Some(suite_id) || !drop.contains(&run.id));
}

/// Check runs that belong to `suite_id`, oldest id first.
pub(crate) fn suite_runs(repo: &Repo, suite_id: i64) -> Vec<&CheckRunRecord> {
    let mut runs: Vec<_> = repo
        .check_runs
        .iter()
        .filter(|run| run.suite_id == Some(suite_id))
        .collect();
    runs.sort_by_key(|run| run.id);
    runs
}

/// Check runs created for the commit `sha`, in the order they were added.
pub(crate) fn commit_runs<'a>(repo: &'a Repo, sha: &str) -> Vec<&'a CheckRunRecord> {
    repo.check_runs
        .iter()
        .filter(|run| run.head_sha == sha)
        .collect()
}

/// The check run with `id`. Fixtures may seed one id on several commits; the lowest commit wins.
pub(crate) fn find_run(repo: &Repo, id: i64) -> Option<&CheckRunRecord> {
    repo.check_runs
        .iter()
        .filter(|run| run.id == id)
        .min_by(|a, b| a.head_sha.cmp(&b.head_sha))
}

/// Applies `filter`, `check_name`, `status`, and `app_id`, then orders by id.
pub(crate) fn select_runs<'a>(
    mut runs: Vec<&'a CheckRunRecord>,
    request: &ApiRequest,
) -> Vec<&'a CheckRunRecord> {
    // Source: <https://docs.github.com/en/rest/checks/runs#list-check-runs-for-a-git-reference>
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
        runs.retain(|run| run.app.as_ref().is_some_and(|app| app.id() == app_id));
    }
    runs.sort_by_key(|run| run.id);
    runs
}

/// The REST model of a check run in `owner`'s repository `repo`.
pub(crate) fn render_run(
    request: &ApiRequest,
    owner: &Account,
    repo: &str,
    run: &CheckRunRecord,
) -> types::CheckRun {
    let urls = request.urls().repo(&owner.login, repo).check_run(run.id);
    let seeded = &run.seeded;
    let app = run.app.as_ref().map(|app| match app {
        CheckRunApp::Id(id) => integration(request, owner, *id, run.created_at),
        CheckRunApp::Seeded(app) => (**app).clone(),
    });
    let details_url = run.details_url.clone().or_else(|| match &run.app {
        Some(CheckRunApp::Id(_)) => app.as_ref().map(|app| app.html_url.clone()),
        _ => None,
    });
    types::CheckRun {
        app,
        check_suite: run.suite_id.map(|id| types::CheckRunCheckSuite { id }),
        completed_at: run.completed_at,
        conclusion: run.conclusion,
        deployment: None,
        details_url,
        external_id: run.external_id.clone(),
        head_sha: seeded
            .head_sha
            .clone()
            .unwrap_or_else(|| run.head_sha.clone()),
        html_url: seeded
            .html_url
            .clone()
            .unwrap_or_else(|| Some(urls.html.clone())),
        id: run.id,
        name: run.name.clone(),
        node_id: seeded
            .node_id
            .clone()
            .unwrap_or_else(|| render::node_id("CheckRun", u64::try_from(run.id).unwrap_or(0))),
        output: types::CheckRunOutput {
            annotations_count: seeded
                .annotations_count
                .unwrap_or_else(|| i64::try_from(run.annotations.len()).unwrap_or(i64::MAX)),
            annotations_url: seeded
                .annotations_url
                .clone()
                .unwrap_or_else(|| urls.annotations()),
            summary: run.output_summary.clone(),
            text: run.output_text.clone(),
            title: run.output_title.clone(),
        },
        pull_requests: run.pull_requests.clone(),
        started_at: run.started_at,
        status: run.status,
        url: seeded.url.clone().unwrap_or(urls.api),
    }
}

/// The REST models of `run`'s annotations, linking each to its file at the run's commit.
pub(crate) fn render_annotations(
    request: &ApiRequest,
    owner: &str,
    repo: &str,
    run: &CheckRunRecord,
) -> Vec<types::CheckAnnotation> {
    let urls = request.urls().repo(owner, repo);
    run.annotations
        .iter()
        .map(|item| types::CheckAnnotation {
            annotation_level: Some(item.level.clone()),
            blob_href: urls.blob_html(&run.head_sha, &item.path),
            end_column: item.end_column,
            end_line: item.end_line,
            message: Some(item.message.clone()),
            path: item.path.clone(),
            raw_details: item.raw_details.clone(),
            start_column: item.start_column,
            start_line: item.start_line,
            title: item.title.clone(),
        })
        .collect()
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
    let urls = request
        .urls()
        .repo(&found.owner, &found.name)
        .check_suite(suite_id);
    let now = suite.created_at;
    Ok(types::CheckSuite {
        after: Some(suite.after),
        app: Some(integration(request, account, suite.app_id, now)),
        before: Some(suite.before),
        check_runs_url: urls.check_runs(),
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
        url: Some(urls.api),
    })
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
    crate::time::parse(text).unwrap_or(fallback)
}

fn app_installed(state: &State, owner: &str, name: &str, app_id: i64) -> bool {
    state.installations.values().any(|installation| {
        installation.owner.eq_ignore_ascii_case(owner)
            && installation.repository.eq_ignore_ascii_case(name)
            && i64::try_from(installation.app_id).ok() == Some(app_id)
    })
}

fn latest_of(runs: Vec<&CheckRunRecord>) -> Vec<&CheckRunRecord> {
    let mut best: BTreeMap<&str, &CheckRunRecord> = BTreeMap::new();
    for run in runs {
        let replace = best
            .get(run.name.as_str())
            .is_none_or(|current| recency(run) > recency(current));
        if replace {
            best.insert(&run.name, run);
        }
    }
    best.into_values().collect()
}

fn recency(run: &CheckRunRecord) -> (DateTime<Utc>, i64) {
    (
        run.completed_at
            .or(run.started_at)
            .unwrap_or(DateTime::UNIX_EPOCH),
        run.id,
    )
}

fn rollup(
    latest: &[&CheckRunRecord],
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

fn rollup_conclusion(runs: &[&CheckRunRecord]) -> types::CheckSuiteConclusion {
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
