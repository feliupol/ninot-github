//! Check run create, update, annotations, and rerequest.

use std::sync::Mutex;

use chrono::{DateTime, Utc};

use super::checks_model::{
    cap_same_name, checks_forbidden, integration, open_suite, require_checks_app, touch_suite,
};
use super::repos::find_repo;
use super::{Handler, checks_model};
use crate::api::{ApiRequest, ApiResponse, ApiResult, paginate};
use crate::operation::ops;
use crate::render;
use crate::state::{Repo, State, lock};
use crate::types;

struct Phase {
    status: types::CheckRunStatus,
    conclusion: Option<types::CheckRunConclusion>,
    completed_at: Option<DateTime<Utc>>,
}

struct AnnotationInput {
    path: String,
    start_line: i64,
    end_line: i64,
    start_column: Option<i64>,
    end_column: Option<i64>,
    level: String,
    message: String,
    title: Option<String>,
    raw_details: Option<String>,
}

impl Handler for ops::ChecksCreate {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let body: types::ChecksCreateRequest = request.json_as()?;
        if body.head_sha.is_empty() {
            return Err(ApiResponse::validation_failed(
                "CheckRun",
                "head_sha",
                "missing_field",
            ));
        }
        let now = render::timestamp(&crate::time::now());
        let phase = create_phase(&body, now)?;
        let annotations = create_annotations(&body)?;
        let mut state = lock(state);
        let found = find_repo(&state, request)?;
        let app_id = require_checks_app(request)?;
        let (suite_id, _) = open_suite(&mut state, &found, app_id, &body.head_sha);
        let account = state
            .account(&found.owner)
            .expect("repository owner exists");
        let app = integration(request, account, app_id, now);
        let head_sha = {
            let repo = repo(&state, &found);
            checks_model::commit_key(repo, &body.head_sha)
        };
        let raw = state.next_id();
        let id = render::id(raw);
        let api = format!("{}/repos/{}/{}", request.api_url, found.owner, found.name);
        let web = format!("{}/{}/{}", request.web_url, found.owner, found.name);
        let stored = annotations
            .iter()
            .map(|item| annotation(item, &web, &head_sha))
            .collect::<Vec<_>>();
        let (title, summary, text) = output_fields(body.output.as_ref());
        let run = types::CheckRun {
            app: Some(app.clone()),
            check_suite: Some(types::CheckRunCheckSuite { id: suite_id }),
            completed_at: phase.completed_at,
            conclusion: phase.conclusion,
            deployment: None,
            details_url: Some(body.details_url.clone().unwrap_or(app.html_url)),
            external_id: body.external_id.clone(),
            head_sha: head_sha.clone(),
            html_url: Some(format!("{web}/runs/{id}")),
            id,
            name: body.name.clone(),
            node_id: render::node_id("CheckRun", raw),
            output: types::CheckRunOutput {
                annotations_count: i64::try_from(stored.len()).unwrap_or(i64::MAX),
                annotations_url: format!("{api}/check-runs/{id}/annotations"),
                summary,
                text,
                title,
            },
            pull_requests: Vec::new(),
            started_at: Some(body.started_at.unwrap_or(now)),
            status: phase.status,
            url: format!("{api}/check-runs/{id}"),
        };
        let repo = repo_mut(&mut state, &found);
        cap_same_name(repo, suite_id, &run.name);
        repo.check_annotations.insert(id, stored);
        repo.check_runs
            .entry(head_sha)
            .or_default()
            .push(run.clone());
        touch_suite(repo, suite_id);
        Ok(ApiResponse::typed(201, &types::ChecksCreateResponse(run)))
    }
}

impl Handler for ops::ChecksUpdate {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let body: types::ChecksUpdateRequest = request.json_as()?;
        let id = parse_id(request.param("check_run_id"))?;
        let now = render::timestamp(&crate::time::now());
        let extra = update_annotations(&body)?;
        let mut state = lock(state);
        let found = find_repo(&state, request)?;
        let app_id = require_checks_app(request)?;
        let current = find_run(repo(&state, &found), id).ok_or_else(ApiResponse::not_found)?;
        if current.app.as_ref().map(|app| app.id) != Some(app_id) {
            return Err(checks_forbidden(request));
        }
        let phase = update_phase(&body, &current, now)?;
        let web = format!("{}/{}/{}", request.web_url, found.owner, found.name);
        let appended = extra
            .iter()
            .map(|item| annotation(item, &web, &current.head_sha))
            .collect::<Vec<_>>();
        let repo = repo_mut(&mut state, &found);
        let count = {
            let stored = repo.check_annotations.entry(id).or_default();
            stored.extend(appended);
            i64::try_from(stored.len()).unwrap_or(i64::MAX)
        };
        let run = find_run_mut(repo, id).expect("check run exists");
        if let Some(name) = body.name.clone() {
            run.name = name;
        }
        if let Some(details_url) = body.details_url.clone() {
            run.details_url = Some(details_url);
        }
        if let Some(external_id) = body.external_id.clone() {
            run.external_id = Some(external_id);
        }
        if let Some(started_at) = body.started_at {
            run.started_at = Some(started_at);
        }
        run.status = phase.status;
        run.conclusion = phase.conclusion;
        run.completed_at = phase.completed_at;
        if let Some(output) = body.output.as_ref() {
            if let Some(title) = output.title.clone() {
                run.output.title = Some(title);
            }
            run.output.summary = Some(String::from(output.summary.clone()));
            if let Some(text) = output.text.clone() {
                run.output.text = Some(String::from(text));
            }
            run.output.annotations_count = count;
        }
        let suite_id = run.check_suite.as_ref().map(|suite| suite.id);
        let updated = run.clone();
        if let Some(suite_id) = suite_id {
            touch_suite(repo, suite_id);
        }
        Ok(ApiResponse::typed(
            200,
            &types::ChecksUpdateResponse(updated),
        ))
    }
}

impl Handler for ops::ChecksListAnnotations {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let found = find_repo(&state, request)?;
        let id = parse_id(request.param("check_run_id"))?;
        let repo = repo(&state, &found);
        if find_run(repo, id).is_none() {
            return Err(ApiResponse::not_found());
        }
        let annotations = repo.check_annotations.get(&id).cloned().unwrap_or_default();
        Ok(paginate(request, &request.path, annotations))
    }
}

impl Handler for ops::ChecksRerequestRun {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let id = parse_id(request.param("check_run_id"))?;
        let mut state = lock(state);
        let found = find_repo(&state, request)?;
        let app_id = require_checks_app(request)?;
        let run = find_run(repo(&state, &found), id).ok_or_else(ApiResponse::not_found)?;
        if run.app.as_ref().map(|app| app.id) != Some(app_id) {
            return Err(checks_forbidden(request));
        }
        let Some(suite_id) = run.check_suite.as_ref().map(|suite| suite.id) else {
            return Err(not_rerequestable());
        };
        let repo = repo_mut(&mut state, &found);
        if !checks_model::mark_rerequested(repo, suite_id) {
            return Err(not_rerequestable());
        }
        Ok(ApiResponse::typed(
            201,
            &types::ChecksRerequestRunResponse(types::EmptyObject {}),
        ))
    }
}

fn not_rerequestable() -> ApiResponse {
    ApiResponse::error(422, "Check run is not rerequestable")
}

fn parse_id(value: &str) -> Result<i64, ApiResponse> {
    value.parse().map_err(|_| ApiResponse::not_found())
}

fn repo<'a>(state: &'a State, found: &super::repos::Found) -> &'a Repo {
    state
        .account(&found.owner)
        .and_then(|account| account.repo(&found.name))
        .expect("repository exists")
}

fn repo_mut<'a>(state: &'a mut State, found: &super::repos::Found) -> &'a mut Repo {
    state
        .account_mut(&found.owner)
        .and_then(|account| account.repo_mut(&found.name))
        .expect("repository exists")
}

fn find_run(repo: &Repo, id: i64) -> Option<types::CheckRun> {
    repo.check_runs
        .values()
        .flatten()
        .find(|run| run.id == id)
        .cloned()
}

fn find_run_mut(repo: &mut Repo, id: i64) -> Option<&mut types::CheckRun> {
    repo.check_runs
        .values_mut()
        .flatten()
        .find(|run| run.id == id)
}

fn create_phase(
    body: &types::ChecksCreateRequest,
    now: DateTime<Utc>,
) -> Result<Phase, ApiResponse> {
    if let Some(conclusion) = body.conclusion {
        return Ok(Phase {
            status: types::CheckRunStatus::Completed,
            conclusion: Some(conclusion_of(conclusion)?),
            completed_at: Some(body.completed_at.unwrap_or(now)),
        });
    }
    if body.completed_at.is_some()
        || matches!(body.status, types::ChecksCreateRequestStatus::Completed)
    {
        return Err(ApiResponse::validation_failed(
            "CheckRun",
            "conclusion",
            "missing_field",
        ));
    }
    Ok(Phase {
        status: run_status(body.status)?,
        conclusion: None,
        completed_at: None,
    })
}

fn update_phase(
    body: &types::ChecksUpdateRequest,
    current: &types::CheckRun,
    now: DateTime<Utc>,
) -> Result<Phase, ApiResponse> {
    let conclusion = body.conclusion.map(conclusion_of).transpose()?;
    if let Some(conclusion) = conclusion {
        return Ok(Phase {
            status: types::CheckRunStatus::Completed,
            conclusion: Some(conclusion),
            completed_at: Some(body.completed_at.unwrap_or(now)),
        });
    }
    let status = body.status.map(run_status).transpose()?;
    if status == Some(types::CheckRunStatus::Completed) || body.completed_at.is_some() {
        return Err(ApiResponse::validation_failed(
            "CheckRun",
            "conclusion",
            "missing_field",
        ));
    }
    if let Some(status) = status {
        return Ok(Phase {
            status,
            conclusion: None,
            completed_at: None,
        });
    }
    Ok(Phase {
        status: current.status,
        conclusion: current.conclusion,
        completed_at: current.completed_at,
    })
}

fn run_status(value: impl std::fmt::Display) -> Result<types::CheckRunStatus, ApiResponse> {
    match value.to_string().as_str() {
        "queued" => Ok(types::CheckRunStatus::Queued),
        "in_progress" => Ok(types::CheckRunStatus::InProgress),
        "completed" => Ok(types::CheckRunStatus::Completed),
        _ => Err(ApiResponse::validation_failed(
            "CheckRun", "status", "invalid",
        )),
    }
}

fn conclusion_of(value: impl std::fmt::Display) -> Result<types::CheckRunConclusion, ApiResponse> {
    let text = value.to_string();
    if text == "stale" {
        return Err(ApiResponse::validation_failed(
            "CheckRun",
            "conclusion",
            "invalid",
        ));
    }
    text.parse()
        .map_err(|_| ApiResponse::validation_failed("CheckRun", "conclusion", "invalid"))
}

fn output_fields(
    output: Option<&types::ChecksCreateRequestOutput>,
) -> (Option<String>, Option<String>, Option<String>) {
    let Some(output) = output else {
        return (None, None, None);
    };
    (
        Some(output.title.clone()),
        Some(String::from(output.summary.clone())),
        output.text.clone().map(String::from),
    )
}

struct AnnotationDraft<'a> {
    path: &'a str,
    start_line: i64,
    end_line: i64,
    start_column: Option<i64>,
    end_column: Option<i64>,
    level: String,
    message: &'a str,
    title: Option<&'a str>,
    raw_details: Option<&'a str>,
}

fn create_annotations(
    body: &types::ChecksCreateRequest,
) -> Result<Vec<AnnotationInput>, ApiResponse> {
    let Some(output) = body.output.as_ref() else {
        return Ok(Vec::new());
    };
    output
        .annotations
        .iter()
        .map(|item| {
            annotation_input(AnnotationDraft {
                path: &item.path,
                start_line: item.start_line,
                end_line: item.end_line,
                start_column: item.start_column,
                end_column: item.end_column,
                level: item.annotation_level.to_string(),
                message: &item.message,
                title: item.title.as_deref(),
                raw_details: item.raw_details.as_deref(),
            })
        })
        .collect()
}

fn update_annotations(
    body: &types::ChecksUpdateRequest,
) -> Result<Vec<AnnotationInput>, ApiResponse> {
    let Some(output) = body.output.as_ref() else {
        return Ok(Vec::new());
    };
    output
        .annotations
        .iter()
        .map(|item| {
            annotation_input(AnnotationDraft {
                path: &item.path,
                start_line: item.start_line,
                end_line: item.end_line,
                start_column: item.start_column,
                end_column: item.end_column,
                level: item.annotation_level.to_string(),
                message: &item.message,
                title: item.title.as_deref(),
                raw_details: item.raw_details.as_deref(),
            })
        })
        .collect()
}

fn annotation_input(draft: AnnotationDraft<'_>) -> Result<AnnotationInput, ApiResponse> {
    if draft.start_line < 1 || draft.end_line < draft.start_line {
        return Err(ApiResponse::validation_failed(
            "CheckAnnotation",
            "start_line",
            "invalid",
        ));
    }
    if draft.start_line != draft.end_line
        && (draft.start_column.is_some() || draft.end_column.is_some())
    {
        return Err(ApiResponse::validation_failed(
            "CheckAnnotation",
            "start_column",
            "invalid",
        ));
    }
    Ok(AnnotationInput {
        path: draft.path.to_owned(),
        start_line: draft.start_line,
        end_line: draft.end_line,
        start_column: draft.start_column,
        end_column: draft.end_column,
        level: draft.level,
        message: draft.message.to_owned(),
        title: draft.title.map(str::to_owned),
        raw_details: draft.raw_details.map(str::to_owned),
    })
}

fn annotation(item: &AnnotationInput, web: &str, sha: &str) -> types::CheckAnnotation {
    types::CheckAnnotation {
        annotation_level: Some(item.level.clone()),
        blob_href: format!("{web}/blob/{sha}/{}", item.path),
        end_column: item.end_column,
        end_line: item.end_line,
        message: Some(item.message.clone()),
        path: item.path.clone(),
        raw_details: item.raw_details.clone(),
        start_column: item.start_column,
        start_line: item.start_line,
        title: item.title.clone(),
    }
}
