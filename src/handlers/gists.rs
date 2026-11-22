//! Gists created through the public Gists API.

use std::sync::Mutex;

use super::Handler;
use crate::api::{ApiRequest, ApiResponse, ApiResult, paginate};
use crate::operation::ops;
use crate::state::{Gist, State, lock};
use crate::types;

impl Handler for ops::GistsCreate {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        // Source: cli/cli v2.101.0 pkg/cmd/gist/create/create.go:283
        request.require_scope(&["gist"])?;
        let body: types::GistsCreateRequest = request.json_as()?;
        if body.files.is_empty() {
            return Err(crate::errors::GithubError::FilesRequired.to_response());
        }

        let mut state = lock(state);
        state.last_gist_id += 1;
        let timestamp = crate::time::now_utc();
        let gist = Gist {
            number: state.last_gist_id,
            description: body.description,
            public: matches!(
                body.public,
                Some(types::GistsCreateRequestPublic::Boolean(true))
            ),
            files: body
                .files
                .into_iter()
                .map(|(filename, file)| (filename, file.content))
                .collect(),
            created_at: timestamp,
            updated_at: timestamp,
        };
        let rendered = gist_simple(request, &gist);
        state.gists.push(gist);
        Ok(ApiResponse::created(&types::GistsCreateResponse(rendered)))
    }
}

impl Handler for ops::GistsList {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        request.require_caller()?;
        let state = lock(state);
        let gists = state
            .gists
            .iter()
            .map(|gist| base_gist(request, gist))
            .collect::<Vec<_>>();
        Ok(paginate(request, &request.path, gists))
    }
}

/// The gist as `gists/create` returns it, with file contents.
fn gist_simple(request: &ApiRequest, gist: &Gist) -> types::GistSimple {
    let id = gist.id();
    let urls = request.urls().gist(&id);
    let timestamp = crate::time::format(&gist.created_at);
    let files = gist
        .files
        .iter()
        .map(|(filename, content)| {
            let file = types::GistSimpleFilesValue {
                content: Some(content.clone()),
                encoding: "utf-8".to_owned(),
                filename: Some(filename.clone()),
                language: language(filename),
                raw_url: Some(urls.raw(filename)),
                size: Some(size(content)),
                truncated: Some(false),
                type_: Some("text/plain".to_owned()),
            };
            (filename.clone(), Some(file))
        })
        .collect();
    types::GistSimple {
        comments: Some(0),
        comments_enabled: Some(true),
        comments_url: Some(urls.comments()),
        commits_url: Some(urls.commits()),
        created_at: Some(timestamp),
        description: gist.description.clone(),
        files,
        fork_of: None,
        forks: Some(Vec::new()),
        forks_url: Some(urls.forks()),
        git_pull_url: Some(urls.git()),
        git_push_url: Some(urls.git()),
        history: Some(Vec::new()),
        html_url: Some(urls.html.clone()),
        id: Some(id.clone()),
        node_id: Some(format!("G_{id}")),
        owner: None,
        public: Some(gist.public),
        truncated: Some(false),
        updated_at: Some(crate::time::format(&gist.updated_at)),
        url: Some(urls.api.clone()),
        user: None,
    }
}

/// The gist as `gists/list` returns it, without file contents.
fn base_gist(request: &ApiRequest, gist: &Gist) -> types::BaseGist {
    let id = gist.id();
    let urls = request.urls().gist(&id);
    types::BaseGist {
        comments: 0,
        comments_enabled: Some(true),
        comments_url: urls.comments(),
        commits_url: urls.commits(),
        created_at: gist.created_at,
        description: gist.description.clone(),
        files: gist
            .files
            .iter()
            .map(|(filename, content)| {
                (
                    filename.clone(),
                    types::BaseGistFilesValue {
                        encoding: "utf-8".to_owned(),
                        filename: Some(filename.clone()),
                        language: language(filename),
                        raw_url: Some(urls.raw(filename)),
                        size: Some(size(content)),
                        type_: Some("text/plain".to_owned()),
                    },
                )
            })
            .collect(),
        forks: Vec::new(),
        forks_url: urls.forks(),
        git_pull_url: urls.git(),
        git_push_url: urls.git(),
        history: Vec::new(),
        html_url: urls.html.clone(),
        node_id: format!("G_{id}"),
        id,
        owner: None,
        public: gist.public,
        truncated: Some(false),
        updated_at: gist.updated_at,
        url: urls.api,
        user: None,
    }
}

/// The language GitHub's linguist would report; only Rust is recognised.
fn language(filename: &str) -> Option<String> {
    filename.ends_with(".rs").then(|| "Rust".to_owned())
}

fn size(content: &str) -> i64 {
    i64::try_from(content.len()).expect("gist file size fits in i64")
}
