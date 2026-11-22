//! Actions runs, jobs, and commit checks read from repository state.

use std::sync::Mutex;

use super::Handler;
use super::repos::find_repo;
use crate::api::{ApiRequest, ApiResponse, ApiResult, paginate_with};
use crate::operation::ops;
use crate::render;
use crate::state::{State, combined_state, latest_per_context, lock};
use crate::types;

impl Handler for ops::ReposGetCombinedStatusForRef {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let found = find_repo(&state, request)?;
        let account = state
            .account(&found.owner)
            .expect("repository owner exists");
        let repo = account.repo(&found.name).expect("repository exists");
        let ref_name = request.param("ref");
        let sha = repo
            .git_dir
            .as_ref()
            .and_then(|git_dir| {
                ninot_common::git::resolve_commit(git_dir, ref_name)
                    .ok()
                    .flatten()
            })
            .unwrap_or_else(|| ref_name.to_owned());
        let urls = request.urls().repo(&found.owner, &found.name);
        let stored = repo
            .commit_statuses
            .get(&sha)
            .map_or(&[][..], Vec::as_slice);
        let latest = latest_per_context(stored);
        let state = combined_state(&latest);
        let statuses = latest
            .into_iter()
            .map(|status| super::status_model::simple_status(&urls, &sha, status))
            .collect::<Vec<_>>();
        let repository =
            crate::render::minimal_repository(request, account, repo, found.permission);
        let commit = urls.commit(&sha);
        let response = types::ReposGetCombinedStatusForRefResponse(types::CombinedCommitStatus {
            commit_url: commit.api.clone(),
            repository,
            sha: sha.clone(),
            state: state.name().to_owned(),
            total_count: statuses.len() as i64,
            statuses,
            url: commit.status(),
        });
        Ok(ApiResponse::ok_typed(&response))
    }
}

impl Handler for ops::ActionsListWorkflowRunsForRepo {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let found = find_repo(&state, request)?;
        let owner = state.account(&found.owner).expect("repository exists");
        let repo = owner.repo(&found.name).expect("repository exists");
        let mut runs = repo
            .workflow_runs
            .iter()
            .filter(|run| super::actions_workflows::run_visible(run, request))
            .collect::<Vec<_>>();
        // Newest first, as GitHub lists them.
        runs.sort_by_key(|run| std::cmp::Reverse((run.created_at, run.id)));
        let total_count = runs.len() as i64;
        let (runs, link) = crate::api::page(request, &request.path, runs);
        let workflow_runs = runs
            .into_iter()
            .map(|run| render::workflow_run(request, owner, repo, run))
            .collect();
        let response = types::ActionsListWorkflowRunsForRepoResponse {
            total_count,
            workflow_runs,
        };
        let response = ApiResponse::conditional_typed(request, &response);
        Ok(match link {
            Some(link) if crate::errors::is_ok(response.status) => {
                response.with_header("link", link)
            }
            _ => response,
        })
    }
}

impl Handler for ops::ActionsGetWorkflowRun {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let found = find_repo(&state, request)?;
        let owner = state.account(&found.owner).expect("repository exists");
        let repo = owner.repo(&found.name).expect("repository exists");
        let id = request
            .param("run_id")
            .parse::<u64>()
            .map_err(|_| ApiResponse::not_found())?;
        let run = repo
            .workflow_runs
            .iter()
            .find(|run| run.id == id)
            .ok_or_else(ApiResponse::not_found)?;
        Ok(ApiResponse::conditional_typed(
            request,
            &types::ActionsGetWorkflowRunResponse(render::workflow_run(request, owner, repo, run)),
        ))
    }
}

fn rerun_workflow(state: &Mutex<State>, request: &ApiRequest, failed_only: bool) -> ApiResult {
    // Source: <https://docs.github.com/en/rest/actions/workflow-runs#re-run-a-workflow>
    request.require_scope(&["repo", "workflow"])?;
    if failed_only {
        let _body: Option<types::ActionsReRunWorkflowFailedJobsRequestBody> = request.json_as()?;
    } else {
        let _body: Option<types::ActionsReRunWorkflowRequestBody> = request.json_as()?;
    }
    let mut state = lock(state);
    let found = find_repo(&state, request)?;
    if !found.permission.push {
        return Err(crate::errors::GithubError::IntegrationForbidden.to_response());
    }
    let id = request
        .param("run_id")
        .parse::<u64>()
        .map_err(|_| ApiResponse::not_found())?;
    let (run_snapshot, job_snapshots) = {
        let repo = state
            .account_mut(&found.owner)
            .and_then(|owner| owner.repo_mut(&found.name))
            .expect("repository exists");
        super::actions_control::remember_attempt(repo, id);
        let run = repo
            .workflow_runs
            .iter_mut()
            .find(|run| run.id == id)
            .ok_or_else(ApiResponse::not_found)?;
        if run.status.as_deref() != Some("completed") {
            return Err(crate::errors::GithubError::WorkflowRunNotCompleted.to_response());
        }
        let jobs = repo
            .workflow_jobs
            .get_mut(&id)
            .ok_or_else(ApiResponse::not_found)?;
        let selected = jobs.iter_mut().filter(|job| {
            !failed_only
                || matches!(
                    job.conclusion,
                    Some(
                        types::JobConclusion::Failure
                            | types::JobConclusion::TimedOut
                            | types::JobConclusion::ActionRequired
                    )
                )
        });
        let mut rerun_ids = Vec::new();
        for job in selected {
            rerun_ids.push(job.id);
            job.status = types::JobStatus::Queued;
            job.conclusion = None;
            job.completed_at = None;
            job.run_attempt = run.run_attempt.saturating_add(1);
            for step in &mut job.steps {
                step.status = types::JobStepsItemStatus::Queued;
                step.conclusion = None;
                step.completed_at = None;
                step.started_at = None;
            }
        }
        if failed_only && rerun_ids.is_empty() {
            return Err(crate::errors::GithubError::WorkflowRunNoFailedJobs.to_response());
        }
        run.run_attempt = run.run_attempt.saturating_add(1);
        run.status = Some("queued".to_owned());
        run.conclusion = None;
        run.run_started_at = None;
        run.updated_at = crate::time::now_utc();
        let run_snapshot = run.clone();
        let job_snapshots = jobs
            .iter()
            .filter(|job| rerun_ids.contains(&job.id))
            .cloned()
            .collect::<Vec<_>>();
        (run_snapshot, job_snapshots)
    };
    let (run, jobs) = {
        let owner = state.account(&found.owner).expect("repository exists");
        let repo = owner.repo(&found.name).expect("repository exists");
        let jobs = job_snapshots
            .iter()
            .map(|job| render::workflow_job(request, &found.owner, &found.name, &run_snapshot, job))
            .collect::<Vec<_>>();
        (
            render::workflow_run(request, owner, repo, &run_snapshot),
            jobs,
        )
    };
    let mut ctx = crate::webhooks::events::Ctx::new(&mut state, request, &found.owner, &found.name);
    crate::webhooks::events::workflow_run::deliver(&mut ctx, &run)?;
    for job in &jobs {
        crate::webhooks::events::workflow_job::deliver(&mut ctx, &run, job)?;
    }
    if failed_only {
        Ok(ApiResponse::created(
            &types::ActionsReRunWorkflowFailedJobsResponse(types::EmptyObject {}),
        ))
    } else {
        Ok(ApiResponse::created(&types::ActionsReRunWorkflowResponse(
            types::EmptyObject {},
        )))
    }
}

impl Handler for ops::ActionsReRunWorkflow {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        rerun_workflow(state, request, false)
    }
}

impl Handler for ops::ActionsReRunWorkflowFailedJobs {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        rerun_workflow(state, request, true)
    }
}

impl Handler for ops::ActionsListJobsForWorkflowRun {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let found = find_repo(&state, request)?;
        let repo = state
            .account(&found.owner)
            .and_then(|owner| owner.repo(&found.name))
            .expect("repository exists");
        let id = request
            .param("run_id")
            .parse::<u64>()
            .map_err(|_| ApiResponse::not_found())?;
        let run = repo
            .workflow_runs
            .iter()
            .find(|run| run.id == id)
            .ok_or_else(ApiResponse::not_found)?;
        let jobs = repo
            .workflow_jobs
            .get(&id)
            .into_iter()
            .flatten()
            .map(|job| render::workflow_job(request, &found.owner, &found.name, run, job))
            .collect::<Vec<_>>();
        let response = types::ActionsListJobsForWorkflowRunResponse {
            total_count: jobs.len() as i64,
            jobs,
        };
        Ok(ApiResponse::conditional_typed(request, &response))
    }
}

impl Handler for ops::ActionsGetJobForWorkflowRun {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let found = find_repo(&state, request)?;
        let repo = state
            .account(&found.owner)
            .and_then(|owner| owner.repo(&found.name))
            .expect("repository exists");
        let job_id = request
            .param("job_id")
            .parse::<u64>()
            .map_err(|_| ApiResponse::not_found())?;
        let (run_id, job) = repo
            .workflow_jobs
            .iter()
            .find_map(|(run_id, jobs)| {
                jobs.iter()
                    .find(|job| job.id == job_id)
                    .map(|job| (*run_id, job))
            })
            .ok_or_else(ApiResponse::not_found)?;
        let run = repo
            .workflow_runs
            .iter()
            .find(|run| run.id == run_id)
            .ok_or_else(ApiResponse::not_found)?;
        let response = types::ActionsGetJobForWorkflowRunResponse(render::workflow_job(
            request,
            &found.owner,
            &found.name,
            run,
            job,
        ));
        Ok(ApiResponse::conditional_typed(request, &response))
    }
}

impl Handler for ops::ActionsDownloadJobLogsForWorkflowRun {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let found = find_repo(&state, request)?;
        let repo = state
            .account(&found.owner)
            .and_then(|owner| owner.repo(&found.name))
            .expect("repository exists");
        let id = request
            .param("job_id")
            .parse::<u64>()
            .map_err(|_| ApiResponse::not_found())?;
        if !repo.workflow_job_logs.contains_key(&id) {
            return Err(ApiResponse::not_found());
        }
        let location = request.urls().job_logs(&found.owner, &found.name, id);
        Ok(ApiResponse::found(location))
    }
}

impl Handler for ops::ActionsDownloadWorkflowRunLogs {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let found = find_repo(&state, request)?;
        let repo = state
            .account(&found.owner)
            .and_then(|owner| owner.repo(&found.name))
            .expect("repository exists");
        let run_id = request
            .param("run_id")
            .parse::<u64>()
            .map_err(|_| ApiResponse::not_found())?;
        if !repo.workflow_runs.iter().any(|run| run.id == run_id) {
            return Err(ApiResponse::not_found());
        }
        let location = request
            .urls()
            .run_log_archive(&found.owner, &found.name, run_id);
        Ok(ApiResponse::found(location))
    }
}

impl Handler for ops::ChecksListForRef {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let found = find_repo(&state, request)?;
        let repo = state
            .account(&found.owner)
            .and_then(|owner| owner.repo(&found.name))
            .expect("repository exists");
        let account = state
            .account(&found.owner)
            .expect("repository owner exists");
        let sha = super::checks_model::commit_key(repo, request.param("ref"));
        let checks =
            super::checks_model::select_runs(super::checks_model::commit_runs(repo, &sha), request)
                .into_iter()
                .map(|run| super::checks_model::render_run(request, account, &found.name, run))
                .collect::<Vec<_>>();
        Ok(paginate_with(
            request,
            &request.path,
            checks,
            |total_count, check_runs| types::ChecksListForRefResponse {
                total_count,
                check_runs,
            },
        ))
    }
}

impl Handler for ops::ChecksGet {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let found = find_repo(&state, request)?;
        let repo = state
            .account(&found.owner)
            .and_then(|owner| owner.repo(&found.name))
            .expect("repository exists");
        let id = request
            .param("check_run_id")
            .parse::<i64>()
            .map_err(|_| ApiResponse::not_found())?;
        let record = super::checks_model::find_run(repo, id).ok_or_else(ApiResponse::not_found)?;
        let account = state
            .account(&found.owner)
            .expect("repository owner exists");
        let check = super::checks_model::render_run(request, account, &found.name, record);
        let response = types::ChecksGetResponse(check);
        Ok(ApiResponse::conditional_typed(request, &response))
    }
}

impl Handler for ops::ReposListCommitStatusesForRef {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let found = find_repo(&state, request)?;
        let repo = state
            .account(&found.owner)
            .and_then(|owner| owner.repo(&found.name))
            .expect("repository exists");
        let sha = request.param("ref");
        let urls = request.urls().repo(&found.owner, &found.name);
        let response = types::ReposListCommitStatusesForRefResponse(
            repo.commit_statuses
                .get(sha)
                .into_iter()
                .flatten()
                .map(|status| super::status_model::status(&urls, sha, status))
                .collect(),
        );
        Ok(ApiResponse::ok_typed(&response))
    }
}

impl Handler for ops::ReposListPullRequestsAssociatedWithCommit {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let found = find_repo(&state, request)?;
        let repo = state
            .account(&found.owner)
            .and_then(|owner| owner.repo(&found.name))
            .expect("repository exists");
        let sha = request.param("commit_sha");
        let pulls = repo
            .pull_requests
            .values()
            .filter_map(|pull| pull.data.as_ref())
            .filter(|pull| pull.head.sha == sha)
            .map(crate::render::pull_request_simple)
            .collect();
        let response = types::ReposListPullRequestsAssociatedWithCommitResponse(pulls);
        Ok(ApiResponse::ok_typed(&response))
    }
}
