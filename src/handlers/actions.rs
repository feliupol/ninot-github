//! Actions runs, jobs, and commit checks read from repository state.

use std::collections::BTreeMap;
use std::sync::Mutex;

use chrono::{DateTime, Utc};

use super::Handler;
use super::repos::find_repo;
use crate::api::{ApiRequest, ApiResponse, ApiResult, paginate_with};
use crate::operation::ops;
use crate::state::{State, lock};
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
        let mut latest = BTreeMap::new();
        for status in repo.commit_statuses.get(&sha).into_iter().flatten() {
            latest.insert(status.context.clone(), status.clone());
        }
        let statuses = latest
            .into_values()
            .map(simple_commit_status)
            .collect::<Result<Vec<_>, _>>()?;
        let state = if statuses
            .iter()
            .any(|status| matches!(status.state.as_str(), "failure" | "error"))
        {
            "failure"
        } else if statuses.iter().any(|status| status.state != "success") {
            "pending"
        } else {
            "success"
        };
        let repository =
            crate::render::minimal_repository(request, account, repo, found.permission);
        let api = format!("{}/repos/{}/{}", request.api_url, found.owner, found.name);
        let response = types::ReposGetCombinedStatusForRefResponse(types::CombinedCommitStatus {
            commit_url: format!("{api}/commits/{sha}"),
            repository,
            sha: sha.clone(),
            state: state.to_owned(),
            total_count: statuses.len() as i64,
            statuses,
            url: format!("{api}/commits/{sha}/status"),
        });
        Ok(ApiResponse::typed(200, &response))
    }
}

impl Handler for ops::ActionsListWorkflowRunsForRepo {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let found = find_repo(&state, request)?;
        let repo = state
            .account(&found.owner)
            .and_then(|owner| owner.repo(&found.name))
            .expect("repository exists");
        let mut runs = repo
            .workflow_runs
            .iter()
            .filter(|run| {
                request
                    .query("branch")
                    .is_none_or(|value| run.head_branch.as_deref() == Some(value))
                    && request
                        .query("event")
                        .is_none_or(|value| run.event == value)
                    && request
                        .query("status")
                        .is_none_or(|value| run.status.as_deref() == Some(value))
                    && request
                        .query("head_sha")
                        .is_none_or(|value| run.head_sha == value)
            })
            .cloned()
            .collect::<Vec<_>>();
        // Newest first, as GitHub lists them.
        runs.sort_by_key(|run| std::cmp::Reverse((run.created_at, run.id)));
        let total_count = runs.len() as i64;
        let (workflow_runs, link) = crate::api::page(request, &request.path, runs);
        let response = types::ActionsListWorkflowRunsForRepoResponse {
            total_count,
            workflow_runs,
        };
        let response = ApiResponse::conditional_typed(request, &response);
        Ok(match link {
            Some(link) if response.status == 200 => response.with_header("link", link),
            _ => response,
        })
    }
}

impl Handler for ops::ActionsGetWorkflowRun {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let found = find_repo(&state, request)?;
        let repo = state
            .account(&found.owner)
            .and_then(|owner| owner.repo(&found.name))
            .expect("repository exists");
        let id = request
            .param("run_id")
            .parse::<i64>()
            .map_err(|_| ApiResponse::not_found())?;
        let run = repo
            .workflow_runs
            .iter()
            .find(|run| run.id == id)
            .cloned()
            .ok_or_else(ApiResponse::not_found)?;
        Ok(ApiResponse::conditional_typed(
            request,
            &types::ActionsGetWorkflowRunResponse(run),
        ))
    }
}

fn rerun_workflow(state: &Mutex<State>, request: &ApiRequest, failed_only: bool) -> ApiResult {
    request.require_scope(&["repo", "workflow"])?;
    if failed_only {
        let _body: Option<types::ActionsReRunWorkflowFailedJobsRequestBody> = request.json_as()?;
    } else {
        let _body: Option<types::ActionsReRunWorkflowRequestBody> = request.json_as()?;
    }
    let mut state = lock(state);
    let found = find_repo(&state, request)?;
    if !found.permission.push {
        return Err(ApiResponse::error(
            403,
            "Resource not accessible by integration",
        ));
    }
    let id = request
        .param("run_id")
        .parse::<i64>()
        .map_err(|_| ApiResponse::not_found())?;
    let (run_snapshot, job_snapshots) = {
        let repo = state
            .account_mut(&found.owner)
            .and_then(|owner| owner.repo_mut(&found.name))
            .expect("repository exists");
        let run = repo
            .workflow_runs
            .iter_mut()
            .find(|run| run.id == id)
            .ok_or_else(ApiResponse::not_found)?;
        if run.status.as_deref() != Some("completed") {
            return Err(ApiResponse::error(409, "Workflow run is not completed"));
        }
        let run_id = u64::try_from(id).map_err(|_| ApiResponse::not_found())?;
        let jobs = repo
            .workflow_jobs
            .get_mut(&run_id)
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
            job.run_attempt = Some(run.run_attempt.unwrap_or(1).saturating_add(1));
            for step in &mut job.steps {
                step.status = types::JobStepsItemStatus::Queued;
                step.conclusion = None;
                step.completed_at = None;
                step.started_at = None;
            }
        }
        if failed_only && rerun_ids.is_empty() {
            return Err(ApiResponse::error(409, "Workflow run has no failed jobs"));
        }
        let next_attempt = run.run_attempt.unwrap_or(1).saturating_add(1);
        run.run_attempt = Some(next_attempt);
        run.status = Some("queued".to_owned());
        run.conclusion = None;
        run.run_started_at = None;
        run.updated_at = DateTime::parse_from_rfc3339(&crate::state::now())
            .expect("server timestamp is RFC 3339")
            .with_timezone(&Utc);
        let run_snapshot = run.clone();
        let job_snapshots = jobs
            .iter()
            .filter(|job| rerun_ids.contains(&job.id))
            .cloned()
            .collect::<Vec<_>>();
        (run_snapshot, job_snapshots)
    };
    let mut ctx = crate::webhooks::events::Ctx::new(&mut state, request, &found.owner, &found.name);
    crate::webhooks::events::workflow_run::deliver(&mut ctx, &run_snapshot);
    for job in &job_snapshots {
        crate::webhooks::events::workflow_job::deliver(&mut ctx, &run_snapshot, job);
    }
    if failed_only {
        Ok(ApiResponse::typed(
            201,
            &types::ActionsReRunWorkflowFailedJobsResponse(types::EmptyObject {}),
        ))
    } else {
        Ok(ApiResponse::typed(
            201,
            &types::ActionsReRunWorkflowResponse(types::EmptyObject {}),
        ))
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
        let jobs = repo
            .workflow_jobs
            .get(&id)
            .cloned()
            .ok_or_else(ApiResponse::not_found)?;
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
            .parse::<i64>()
            .map_err(|_| ApiResponse::not_found())?;
        let job = repo
            .workflow_jobs
            .values()
            .flatten()
            .find(|job| job.id == job_id)
            .cloned()
            .ok_or_else(ApiResponse::not_found)?;
        let response = types::ActionsGetJobForWorkflowRunResponse(job);
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
        let location = format!(
            "{}/_ninot/actions/logs/{}/{}/{}",
            request.web_url, found.owner, found.name, id
        );
        Ok(ApiResponse {
            status: 302,
            headers: vec![("location".to_owned(), location)],
            body: crate::api::Body::Empty,
        })
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
        if !repo
            .workflow_runs
            .iter()
            .any(|run| u64::try_from(run.id).ok() == Some(run_id))
        {
            return Err(ApiResponse::not_found());
        }
        let location = format!(
            "{}/_ninot/actions/log-archives/{}/{}/{}",
            request.web_url, found.owner, found.name, run_id
        );
        Ok(ApiResponse {
            status: 302,
            headers: vec![("location".to_owned(), location)],
            body: crate::api::Body::Empty,
        })
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
        let sha = super::checks_model::commit_key(repo, request.param("ref"));
        let checks = super::checks_model::select_runs(
            repo.check_runs.get(&sha).cloned().unwrap_or_default(),
            request,
        );
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
        let check = repo
            .check_runs
            .values()
            .flatten()
            .find(|check| check.id == id)
            .cloned()
            .ok_or_else(ApiResponse::not_found)?;
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
        let response = types::ReposListCommitStatusesForRefResponse(
            repo.commit_statuses
                .get(request.param("ref"))
                .cloned()
                .unwrap_or_default(),
        );
        Ok(ApiResponse::typed(200, &response))
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
        Ok(ApiResponse::typed(200, &response))
    }
}

/// Converts a status to the smaller timestamp representation used by combined commit status.
fn simple_commit_status(status: types::Status) -> Result<types::SimpleCommitStatus, ApiResponse> {
    let created_at = parse_timestamp(&status.created_at)?;
    let updated_at = parse_timestamp(&status.updated_at)?;
    Ok(types::SimpleCommitStatus {
        avatar_url: status.avatar_url,
        context: status.context,
        created_at,
        description: status.description,
        id: status.id,
        node_id: status.node_id,
        required: None,
        state: status.state,
        target_url: status.target_url,
        updated_at,
        url: status.url,
    })
}

/// Parses a timestamp already accepted by the status fixture model.
fn parse_timestamp(value: &str) -> Result<DateTime<Utc>, ApiResponse> {
    DateTime::parse_from_rfc3339(value)
        .map(|timestamp| timestamp.with_timezone(&Utc))
        .map_err(|_| ApiResponse::error(500, "Stored status timestamp is invalid"))
}
