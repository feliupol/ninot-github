//! Actions artifacts: list, fetch, delete, and the zip download redirect.

use std::fmt::Display;
use std::sync::Mutex;

use super::Handler;
use super::actions_workflows::require_actions_write;
use super::repos::find_repo;
use crate::api::{ApiRequest, ApiResponse, ApiResult};
use crate::operation::ops;
use crate::render;
use crate::state::{ArtifactRecord, State, lock};
use crate::types;

fn artifact_id(request: &ApiRequest) -> Result<u64, ApiResponse> {
    request
        .param("artifact_id")
        .parse::<u64>()
        .map_err(|_| ApiResponse::not_found())
}

fn render_artifact(
    request: &ApiRequest,
    owner: &str,
    repo: &str,
    artifact: &ArtifactRecord,
) -> types::Artifact {
    let urls = request.urls();
    let id = artifact.id.to_string();
    types::Artifact {
        archive_download_url: urls.api_for::<ops::ActionsDownloadArtifact>(&[
            ("owner", &owner as &dyn Display),
            ("repo", &repo as &dyn Display),
            ("artifact_id", &id as &dyn Display),
            ("archive_format", &"zip" as &dyn Display),
        ]),
        created_at: Some(artifact.created_at),
        digest: None,
        expired: artifact.expired,
        expires_at: artifact.expires_at,
        id: render::id(artifact.id),
        name: artifact.name.clone(),
        node_id: render::node_id("Artifact", artifact.id),
        size_in_bytes: artifact.size_in_bytes,
        updated_at: Some(artifact.updated_at),
        url: urls.api_for::<ops::ActionsGetArtifact>(&[
            ("owner", &owner as &dyn Display),
            ("repo", &repo as &dyn Display),
            ("artifact_id", &id as &dyn Display),
        ]),
        workflow_run: None,
    }
}

fn list_artifacts(request: &ApiRequest, artifacts: Vec<types::Artifact>) -> ApiResponse {
    crate::api::paginate_with(
        request,
        &request.path,
        artifacts,
        |total_count, artifacts| types::ActionsListArtifactsForRepoResponse {
            total_count,
            artifacts,
        },
    )
}

fn selected<'a>(
    artifacts: impl Iterator<Item = &'a ArtifactRecord>,
    request: &ApiRequest,
    run_id: Option<u64>,
) -> Vec<&'a ArtifactRecord> {
    artifacts
        .filter(|artifact| run_id.is_none_or(|id| artifact.run_id == id))
        .filter(|artifact| {
            request
                .query("name")
                .is_none_or(|name| artifact.name == name)
        })
        .collect()
}

impl Handler for ops::ActionsListArtifactsForRepo {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let found = find_repo(&state, request)?;
        let repo = found.repo(&state);
        let artifacts = selected(repo.artifacts.iter(), request, None)
            .into_iter()
            .map(|artifact| render_artifact(request, &found.owner, &found.name, artifact))
            .collect();
        Ok(list_artifacts(request, artifacts))
    }
}

impl Handler for ops::ActionsListWorkflowRunArtifacts {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let found = find_repo(&state, request)?;
        let run_id = request
            .param("run_id")
            .parse::<u64>()
            .map_err(|_| ApiResponse::not_found())?;
        let repo = found.repo(&state);
        if !repo.workflow_runs.iter().any(|run| run.id == run_id) {
            return Err(ApiResponse::not_found());
        }
        let artifacts = selected(repo.artifacts.iter(), request, Some(run_id))
            .into_iter()
            .map(|artifact| render_artifact(request, &found.owner, &found.name, artifact))
            .collect();
        let response = crate::api::paginate_with(
            request,
            &request.path,
            artifacts,
            |total_count, artifacts| types::ActionsListWorkflowRunArtifactsResponse {
                total_count,
                artifacts,
            },
        );
        Ok(response)
    }
}

impl Handler for ops::ActionsGetArtifact {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let found = find_repo(&state, request)?;
        let id = artifact_id(request)?;
        let repo = found.repo(&state);
        let artifact = repo
            .artifacts
            .iter()
            .find(|artifact| artifact.id == id)
            .ok_or_else(ApiResponse::not_found)?;
        Ok(ApiResponse::conditional_typed(
            request,
            &types::ActionsGetArtifactResponse(render_artifact(
                request,
                &found.owner,
                &found.name,
                artifact,
            )),
        ))
    }
}

impl Handler for ops::ActionsDeleteArtifact {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let mut state = lock(state);
        let found = require_actions_write(&state, request)?;
        let id = artifact_id(request)?;
        let repo = found.repo_mut(&mut state);
        let Some(index) = repo.artifacts.iter().position(|artifact| artifact.id == id) else {
            return Err(ApiResponse::not_found());
        };
        repo.artifacts.remove(index);
        Ok(ApiResponse::no_content())
    }
}

impl Handler for ops::ActionsDownloadArtifact {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let found = find_repo(&state, request)?;
        let id = artifact_id(request)?;
        let repo = found.repo(&state);
        let artifact = repo
            .artifacts
            .iter()
            .find(|artifact| artifact.id == id)
            .ok_or_else(ApiResponse::not_found)?;
        if artifact.expired {
            return Err(ApiResponse::error(410, "Artifact has expired"));
        }
        let location = request.urls().web(&format!(
            "/_ninot/actions/artifacts/{}/{}/{id}",
            found.owner, found.name
        ));
        Ok(ApiResponse {
            status: 302,
            headers: vec![("location".to_owned(), location)],
            body: crate::api::Body::Empty,
        })
    }
}
