//! Workflow files on the default branch, and the runs they dispatch.

use std::collections::BTreeSet;
use std::sync::Mutex;

use super::Handler;
use super::actions_workflow_files::workflow_files;
use super::repos::{Found, find_repo};
use crate::api::{ApiRequest, ApiResponse, ApiResult};
use crate::operation::ops;
use crate::render;
use crate::state::{State, WorkflowRecord, WorkflowRunRecord, lock};
use crate::types;

/// `repo` or `workflow` scope, and push permission on the repository.
pub(crate) fn require_actions_write(
    state: &State,
    request: &ApiRequest,
) -> Result<Found, ApiResponse> {
    request.require_scope(&["repo", "workflow"])?;
    let found = find_repo(state, request)?;
    if !found.permission.push {
        return Err(ApiResponse::error(
            403,
            "Resource not accessible by integration",
        ));
    }
    Ok(found)
}

/// Branch, event, status (including conclusions), and head SHA filters of the runs API.
pub(crate) fn run_visible(run: &WorkflowRunRecord, request: &ApiRequest) -> bool {
    request
        .query("branch")
        .is_none_or(|value| run.head_branch.as_deref() == Some(value))
        && request
            .query("event")
            .is_none_or(|value| run.event == value)
        && request.query("status").is_none_or(|value| {
            run.status.as_deref() == Some(value) || run.conclusion.as_deref() == Some(value)
        })
        && request
            .query("head_sha")
            .is_none_or(|value| run.head_sha == value)
}

/// Refreshes workflows from `.github/workflows` and from runs that have no record yet.
pub(crate) fn sync_workflows(state: &mut State, owner: &str, repo_name: &str) {
    let Some((git_dir, branch, existing, runs)) = (|| {
        let repo = state.account(owner)?.repo(repo_name)?;
        Some((
            repo.git_dir.clone(),
            repo.default_branch.clone(),
            repo.workflows
                .iter()
                .map(|workflow| (workflow.id, workflow.path.clone(), workflow.state.clone()))
                .collect::<Vec<_>>(),
            repo.workflow_runs
                .iter()
                .map(|run| (run.workflow_id, run.path.clone()))
                .collect::<Vec<_>>(),
        ))
    })() else {
        return;
    };
    let scanned = workflow_files(git_dir.as_deref(), &branch);
    let now = crate::time::now_utc();
    let mut used: BTreeSet<u64> = existing
        .iter()
        .map(|(id, _, _)| *id)
        .chain(runs.iter().map(|(id, _)| *id))
        .collect();
    let mut fresh = Vec::new();
    if let Some(files) = &scanned {
        let needed = files
            .iter()
            .filter(|file| {
                !existing
                    .iter()
                    .any(|(_, path, state)| path == &file.path && state != "deleted")
                    && !runs.iter().any(|(id, path)| {
                        path == &file.path && !existing.iter().any(|(have, _, _)| have == id)
                    })
            })
            .count();
        while fresh.len() < needed {
            let id = state.next_id();
            if used.insert(id) {
                fresh.push(id);
            }
        }
    }
    let Some(repo) = state
        .account_mut(owner)
        .and_then(|account| account.repo_mut(repo_name))
    else {
        return;
    };
    let mut fresh = fresh.into_iter();
    if let Some(files) = &scanned {
        let paths: BTreeSet<&str> = files.iter().map(|file| file.path.as_str()).collect();
        for workflow in &mut repo.workflows {
            if workflow.on_branch
                && workflow.state != "deleted"
                && !paths.contains(workflow.path.as_str())
            {
                workflow.state = "deleted".to_owned();
                workflow.updated_at = now;
            }
        }
        for file in files {
            if let Some(workflow) = repo
                .workflows
                .iter_mut()
                .find(|workflow| workflow.path == file.path && workflow.state != "deleted")
            {
                workflow.name = file.name.clone();
                workflow.content = file.content.clone();
                workflow.on_branch = true;
                workflow.updated_at = now;
                continue;
            }
            let adopted = repo.workflow_runs.iter().find_map(|run| {
                (run.path == file.path
                    && !repo
                        .workflows
                        .iter()
                        .any(|workflow| workflow.id == run.workflow_id))
                .then_some(run.workflow_id)
            });
            let id = adopted.unwrap_or_else(|| fresh.next().expect("workflow id"));
            repo.workflows.push(WorkflowRecord {
                id,
                name: file.name.clone(),
                path: file.path.clone(),
                state: "active".to_owned(),
                on_branch: true,
                content: file.content.clone(),
                created_at: now,
                updated_at: now,
            });
        }
    }
    let pending: Vec<_> = repo
        .workflow_runs
        .iter()
        .map(|run| {
            (
                run.workflow_id,
                run.name.clone().unwrap_or_else(|| run.path.clone()),
                run.path.clone(),
                run.created_at,
            )
        })
        .collect();
    for (id, name, path, created_at) in pending {
        if repo.workflows.iter().any(|workflow| workflow.id == id) {
            continue;
        }
        repo.workflows.push(WorkflowRecord {
            id,
            name,
            path,
            state: "active".to_owned(),
            on_branch: false,
            content: String::new(),
            created_at,
            updated_at: now,
        });
    }
    repo.workflows
        .sort_by(|left, right| left.path.cmp(&right.path).then(left.id.cmp(&right.id)));
}

fn find_workflow<'a>(repo: &'a crate::state::Repo, selector: &str) -> Option<&'a WorkflowRecord> {
    let workflow = if let Ok(id) = selector.parse::<u64>() {
        repo.workflows.iter().find(|workflow| workflow.id == id)
    } else {
        let path = if selector.contains('/') {
            selector.to_owned()
        } else {
            format!(".github/workflows/{selector}")
        };
        repo.workflows.iter().find(|workflow| workflow.path == path)
    };
    workflow.filter(|workflow| workflow.state != "deleted")
}

fn workflow_state(state: &str) -> types::WorkflowState {
    state
        .parse()
        .unwrap_or_else(|_| panic!("workflow state {state}"))
}

fn render_workflow(
    request: &ApiRequest,
    owner: &str,
    repo: &crate::state::Repo,
    workflow: &WorkflowRecord,
) -> types::Workflow {
    let urls = request.urls().repo(owner, &repo.name);
    types::Workflow {
        badge_url: urls.workflow_badge(workflow.id),
        created_at: workflow.created_at,
        deleted_at: None,
        html_url: urls.blob_html(&repo.default_branch, &workflow.path),
        id: render::id(workflow.id),
        name: workflow.name.clone(),
        node_id: render::node_id("Workflow", workflow.id),
        path: workflow.path.clone(),
        state: workflow_state(&workflow.state),
        updated_at: workflow.updated_at,
        url: urls.workflow(workflow.id),
    }
}

fn runs_page(
    request: &ApiRequest,
    state: &State,
    found: &Found,
    workflow_id: Option<u64>,
) -> ApiResult {
    let owner = state.account(&found.owner).expect("repository exists");
    let repo = owner.repo(&found.name).expect("repository exists");
    let mut runs = repo
        .workflow_runs
        .iter()
        .filter(|run| workflow_id.is_none_or(|id| run.workflow_id == id))
        .filter(|run| run_visible(run, request))
        .collect::<Vec<_>>();
    runs.sort_by_key(|run| std::cmp::Reverse((run.created_at, run.id)));
    let total_count = runs.len() as i64;
    let (runs, link) = crate::api::page(request, &request.path, runs);
    let workflow_runs = runs
        .into_iter()
        .map(|run| render::workflow_run(request, owner, repo, run))
        .collect();
    let response = types::ActionsListWorkflowRunsResponse {
        total_count,
        workflow_runs,
    };
    let response = ApiResponse::conditional_typed(request, &response);
    Ok(match link {
        Some(link) if response.status == 200 => response.with_header("link", link),
        _ => response,
    })
}

impl Handler for ops::ActionsListRepoWorkflows {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let mut state = lock(state);
        let found = find_repo(&state, request)?;
        sync_workflows(&mut state, &found.owner, &found.name);
        let repo = found.repo(&state);
        let workflows = repo
            .workflows
            .iter()
            .filter(|workflow| workflow.state != "deleted")
            .map(|workflow| render_workflow(request, &found.owner, repo, workflow))
            .collect::<Vec<_>>();
        Ok(crate::api::paginate_with(
            request,
            &request.path,
            workflows,
            |total_count, workflows| types::ActionsListRepoWorkflowsResponse {
                total_count,
                workflows,
            },
        ))
    }
}

impl Handler for ops::ActionsGetWorkflow {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let mut state = lock(state);
        let found = find_repo(&state, request)?;
        sync_workflows(&mut state, &found.owner, &found.name);
        let selector = request.param("workflow_id").to_owned();
        let repo = found.repo(&state);
        let workflow = find_workflow(repo, &selector).ok_or_else(ApiResponse::not_found)?;
        Ok(ApiResponse::conditional_typed(
            request,
            &types::ActionsGetWorkflowResponse(render_workflow(
                request,
                &found.owner,
                repo,
                workflow,
            )),
        ))
    }
}

fn set_workflow_state(state: &Mutex<State>, request: &ApiRequest, next: &str) -> ApiResult {
    let mut state = lock(state);
    let found = require_actions_write(&state, request)?;
    sync_workflows(&mut state, &found.owner, &found.name);
    let selector = request.param("workflow_id").to_owned();
    let repo = found.repo_mut(&mut state);
    let workflow = repo
        .workflows
        .iter_mut()
        .find(|workflow| workflow.state != "deleted" && selector_matches(workflow, &selector))
        .ok_or_else(ApiResponse::not_found)?;
    workflow.state = next.to_owned();
    workflow.updated_at = crate::time::now_utc();
    Ok(ApiResponse::no_content())
}

fn selector_matches(workflow: &WorkflowRecord, selector: &str) -> bool {
    if let Ok(id) = selector.parse::<u64>() {
        workflow.id == id
    } else {
        let path = if selector.contains('/') {
            selector.to_owned()
        } else {
            format!(".github/workflows/{selector}")
        };
        workflow.path == path
    }
}

impl Handler for ops::ActionsDisableWorkflow {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        set_workflow_state(state, request, "disabled_manually")
    }
}

impl Handler for ops::ActionsEnableWorkflow {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        set_workflow_state(state, request, "active")
    }
}

impl Handler for ops::ActionsCreateWorkflowDispatch {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let body: types::ActionsCreateWorkflowDispatchRequest = request.json_as()?;
        let mut state = lock(state);
        let found = require_actions_write(&state, request)?;
        sync_workflows(&mut state, &found.owner, &found.name);
        let selector = request.param("workflow_id").to_owned();
        let reference = body.ref_.clone();
        let details = body.return_run_details.unwrap_or(false);
        let (workflow_id, workflow_name, workflow_path, workflow_state, content, git_dir) = {
            let repo = found.repo(&state);
            let workflow = find_workflow(repo, &selector).ok_or_else(ApiResponse::not_found)?;
            (
                workflow.id,
                workflow.name.clone(),
                workflow.path.clone(),
                workflow.state.clone(),
                workflow.content.clone(),
                repo.git_dir.clone(),
            )
        };
        if workflow_state != "active" {
            return Err(ApiResponse::unprocessable("Workflow is disabled"));
        }
        if !content.is_empty() && !content.contains("workflow_dispatch") {
            return Err(ApiResponse::unprocessable(
                "Workflow does not have 'workflow_dispatch' trigger",
            ));
        }
        let sha = git_dir
            .as_ref()
            .and_then(|git_dir| {
                ninot_common::git::resolve_commit(git_dir, &reference)
                    .ok()
                    .flatten()
            })
            .ok_or_else(|| ApiResponse::unprocessable("No commit found for the ref"))?;
        let id = state.next_id();
        let repo = found.repo_mut(&mut state);
        let run_number = repo
            .workflow_runs
            .iter()
            .filter(|run| run.workflow_id == workflow_id)
            .map(|run| run.run_number)
            .max()
            .unwrap_or(0)
            + 1;
        let mut run = crate::fixtures::workflow_run(id, sha);
        run.name = Some(workflow_name.clone());
        run.display_title = workflow_name;
        run.path = workflow_path;
        run.workflow_id = workflow_id;
        run.run_number = run_number;
        run.run_attempt = 1;
        run.event = "workflow_dispatch".to_owned();
        run.status = Some("queued".to_owned());
        run.conclusion = None;
        run.head_branch = head_branch(&reference);
        run.run_started_at = None;
        run.html_url = None;
        repo.workflow_runs.push(run);
        repo.workflow_jobs.insert(id, Vec::new());
        if !details {
            return Ok(ApiResponse::no_content());
        }
        let urls = request
            .urls()
            .repo(&found.owner, &found.name)
            .workflow_run(id);
        Ok(ApiResponse::typed(
            200,
            &types::ActionsCreateWorkflowDispatchResponse(types::WorkflowDispatchResponse {
                html_url: urls.html,
                run_url: urls.api,
                workflow_run_id: types::WorkflowRunId(render::id(id)),
            }),
        ))
    }
}

fn head_branch(reference: &str) -> Option<String> {
    let name = reference.strip_prefix("refs/heads/").unwrap_or(reference);
    if reference.starts_with("refs/tags/")
        || (name.len() == 40 && name.chars().all(|character| character.is_ascii_hexdigit()))
    {
        return None;
    }
    Some(name.to_owned())
}

impl Handler for ops::ActionsListWorkflowRuns {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let mut state = lock(state);
        let found = find_repo(&state, request)?;
        sync_workflows(&mut state, &found.owner, &found.name);
        let selector = request.param("workflow_id").to_owned();
        let workflow_id = find_workflow(found.repo(&state), &selector)
            .ok_or_else(ApiResponse::not_found)?
            .id;
        runs_page(request, &state, &found, Some(workflow_id))
    }
}
