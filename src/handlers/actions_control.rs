//! Cancel, rerun, delete, and historical attempts of workflow runs.

use std::sync::Mutex;

use super::Handler;
use super::actions_workflows::require_actions_write;
use super::repos::find_repo;
use crate::api::{ApiRequest, ApiResponse, ApiResult};
use crate::operation::ops;
use crate::render;
use crate::state::{AttemptRecord, JobRecord, Repo, State, WorkflowRunRecord, lock};
use crate::types;

/// Stores the current attempt of `run_id` before a rerun replaces it.
pub(crate) fn remember_attempt(repo: &mut Repo, run_id: u64) {
    let Some(run) = repo
        .workflow_runs
        .iter()
        .find(|run| run.id == run_id)
        .cloned()
    else {
        return;
    };
    let jobs = repo.workflow_jobs.get(&run_id).cloned().unwrap_or_default();
    repo.workflow_attempts
        .entry(run_id)
        .or_default()
        .entry(run.run_attempt)
        .or_insert(AttemptRecord { run, jobs });
}

fn run_id(request: &ApiRequest) -> Result<u64, ApiResponse> {
    request
        .param("run_id")
        .parse::<u64>()
        .map_err(|_| ApiResponse::not_found())
}

fn attempt_number(request: &ApiRequest) -> Result<u64, ApiResponse> {
    request
        .param("attempt_number")
        .parse::<u64>()
        .map_err(|_| ApiResponse::not_found())
}

fn owned_attempt(
    repo: &Repo,
    run_id: u64,
    attempt: u64,
) -> Option<(WorkflowRunRecord, Vec<JobRecord>)> {
    let run = repo.workflow_runs.iter().find(|run| run.id == run_id)?;
    if run.run_attempt == attempt {
        let jobs = repo.workflow_jobs.get(&run_id).cloned().unwrap_or_default();
        return Some((run.clone(), jobs));
    }
    let record = repo.workflow_attempts.get(&run_id)?.get(&attempt)?;
    Some((record.run.clone(), record.jobs.clone()))
}

fn cancel_run(state: &Mutex<State>, request: &ApiRequest, force: bool) -> ApiResult {
    let mut state = lock(state);
    let found = require_actions_write(&state, request)?;
    let id = run_id(request)?;
    let now = crate::time::now_utc();
    let repo = found.repo_mut(&mut state);
    {
        let run = repo
            .workflow_runs
            .iter_mut()
            .find(|run| run.id == id)
            .ok_or_else(ApiResponse::not_found)?;
        if run.status.as_deref() == Some("completed") {
            return Err(ApiResponse::error(
                409,
                "Cannot cancel a workflow run that is completed",
            ));
        }
        run.status = Some("completed".to_owned());
        run.conclusion = Some("cancelled".to_owned());
        run.updated_at = now;
    }
    if let Some(jobs) = repo.workflow_jobs.get_mut(&id) {
        for job in jobs
            .iter_mut()
            .filter(|job| job.status != types::JobStatus::Completed)
        {
            job.status = types::JobStatus::Completed;
            job.conclusion = Some(types::JobConclusion::Cancelled);
            job.completed_at = Some(now);
        }
    }
    if force {
        Ok(ApiResponse::typed(
            202,
            &types::ActionsForceCancelWorkflowRunResponse(types::EmptyObject {}),
        ))
    } else {
        Ok(ApiResponse::typed(
            202,
            &types::ActionsCancelWorkflowRunResponse(types::EmptyObject {}),
        ))
    }
}

impl Handler for ops::ActionsCancelWorkflowRun {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        cancel_run(state, request, false)
    }
}

impl Handler for ops::ActionsForceCancelWorkflowRun {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        cancel_run(state, request, true)
    }
}

impl Handler for ops::ActionsDeleteWorkflowRun {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let mut state = lock(state);
        let found = require_actions_write(&state, request)?;
        let id = run_id(request)?;
        let repo = found.repo_mut(&mut state);
        let Some(index) = repo.workflow_runs.iter().position(|run| run.id == id) else {
            return Err(ApiResponse::not_found());
        };
        let job_ids: Vec<u64> = repo
            .workflow_jobs
            .get(&id)
            .map(|jobs| jobs.iter().map(|job| job.id).collect())
            .unwrap_or_default();
        repo.workflow_runs.remove(index);
        repo.workflow_jobs.remove(&id);
        for job_id in job_ids {
            repo.workflow_job_logs.remove(&job_id);
        }
        repo.artifacts.retain(|artifact| artifact.run_id != id);
        repo.workflow_attempts.remove(&id);
        Ok(ApiResponse::no_content())
    }
}

impl Handler for ops::ActionsReRunJobForWorkflowRun {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let _body: types::ActionsReRunJobForWorkflowRunRequest = request.json_as()?;
        let mut state = lock(state);
        let found = require_actions_write(&state, request)?;
        let job_id = request
            .param("job_id")
            .parse::<u64>()
            .map_err(|_| ApiResponse::not_found())?;
        let repo = found.repo_mut(&mut state);
        let (run_id, completed) = repo
            .workflow_jobs
            .iter()
            .find_map(|(run_id, jobs)| {
                jobs.iter()
                    .find(|job| job.id == job_id)
                    .map(|job| (*run_id, job.status == types::JobStatus::Completed))
            })
            .ok_or_else(ApiResponse::not_found)?;
        if !completed {
            return Err(ApiResponse::error(403, "Job cannot be rerun"));
        }
        remember_attempt(repo, run_id);
        let now = crate::time::now_utc();
        let attempt = {
            let run = repo
                .workflow_runs
                .iter_mut()
                .find(|run| run.id == run_id)
                .ok_or_else(ApiResponse::not_found)?;
            run.run_attempt = run.run_attempt.saturating_add(1);
            run.status = Some("queued".to_owned());
            run.conclusion = None;
            run.run_started_at = None;
            run.updated_at = now;
            run.run_attempt
        };
        let job = repo
            .workflow_jobs
            .get_mut(&run_id)
            .and_then(|jobs| jobs.iter_mut().find(|job| job.id == job_id))
            .expect("job exists");
        job.status = types::JobStatus::Queued;
        job.conclusion = None;
        job.completed_at = None;
        job.run_attempt = attempt;
        for step in &mut job.steps {
            step.status = types::JobStepsItemStatus::Queued;
            step.conclusion = None;
            step.completed_at = None;
            step.started_at = None;
        }
        Ok(ApiResponse::typed(
            201,
            &types::ActionsReRunJobForWorkflowRunResponse(types::EmptyObject {}),
        ))
    }
}

impl Handler for ops::ActionsGetWorkflowRunAttempt {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let found = find_repo(&state, request)?;
        let id = run_id(request)?;
        let attempt = attempt_number(request)?;
        let owner = state.account(&found.owner).expect("repository exists");
        let repo = owner.repo(&found.name).expect("repository exists");
        let (run, _) = owned_attempt(repo, id, attempt).ok_or_else(ApiResponse::not_found)?;
        Ok(ApiResponse::conditional_typed(
            request,
            &types::ActionsGetWorkflowRunAttemptResponse(render::workflow_run(
                request, owner, repo, &run,
            )),
        ))
    }
}

impl Handler for ops::ActionsListJobsForWorkflowRunAttempt {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let found = find_repo(&state, request)?;
        let id = run_id(request)?;
        let attempt = attempt_number(request)?;
        let repo = found.repo(&state);
        let (run, jobs) = owned_attempt(repo, id, attempt).ok_or_else(ApiResponse::not_found)?;
        let jobs = jobs
            .iter()
            .map(|job| render::workflow_job(request, &found.owner, &found.name, &run, job))
            .collect::<Vec<_>>();
        let response = types::ActionsListJobsForWorkflowRunAttemptResponse {
            total_count: jobs.len() as i64,
            jobs,
        };
        Ok(ApiResponse::conditional_typed(request, &response))
    }
}
