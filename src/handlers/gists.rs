//! Gists created through the public Gists API.

use std::sync::Mutex;

use super::Handler;
use crate::api::{ApiRequest, ApiResponse, ApiResult, paginate};
use crate::operation::ops;
use crate::state::{State, lock};
use crate::types;

impl Handler for ops::GistsCreate {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        request.require_scope(&["gist"])?;
        let body: types::GistsCreateRequest = request.json_as()?;
        if body.files.is_empty() {
            return Err(ApiResponse::error(422, "files is required"));
        }

        let mut state = lock(state);
        state.last_gist_id += 1;
        let id = format!("ninotgist{:06}", state.last_gist_id);
        let api = request.api_url.trim_end_matches('/');
        let web = request.web_url.trim_end_matches('/');
        let timestamp = crate::state::now();
        let files = body
            .files
            .into_iter()
            .map(|(filename, file)| {
                let content = file.content;
                let file = types::GistSimpleFilesValue {
                    content: Some(content.clone()),
                    encoding: "utf-8".to_owned(),
                    filename: Some(filename.clone()),
                    language: filename.ends_with(".rs").then(|| "Rust".to_owned()),
                    raw_url: Some(format!("{web}/raw/{id}/{filename}")),
                    size: Some(i64::try_from(content.len()).expect("gist file size fits in i64")),
                    truncated: Some(false),
                    type_: Some("text/plain".to_owned()),
                };
                (filename, Some(file))
            })
            .collect();
        let gist = types::GistSimple {
            comments: Some(0),
            comments_enabled: Some(true),
            comments_url: Some(format!("{api}/gists/{id}/comments")),
            commits_url: Some(format!("{api}/gists/{id}/commits")),
            created_at: Some(timestamp.clone()),
            description: body.description,
            files,
            fork_of: None,
            forks: Some(Vec::new()),
            forks_url: Some(format!("{api}/gists/{id}/forks")),
            git_pull_url: Some(format!("https://gist.github.test/{id}.git")),
            git_push_url: Some(format!("https://gist.github.test/{id}.git")),
            history: Some(Vec::new()),
            html_url: Some(format!("{web}/gist/{id}")),
            id: Some(id.clone()),
            node_id: Some(format!("G_{id}")),
            owner: None,
            public: Some(matches!(
                body.public,
                Some(types::GistsCreateRequestPublic::Boolean(true))
            )),
            truncated: Some(false),
            updated_at: Some(timestamp.clone()),
            url: Some(format!("{api}/gists/{id}")),
            user: None,
        };
        state.gists.push(gist.clone());
        Ok(ApiResponse::typed(201, &types::GistsCreateResponse(gist)))
    }
}

impl Handler for ops::GistsList {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        request.require_caller()?;
        let state = lock(state);
        let gists = state.gists.iter().map(base_gist).collect::<Vec<_>>();
        Ok(paginate(request, &request.path, gists))
    }
}

fn base_gist(gist: &types::GistSimple) -> types::BaseGist {
    types::BaseGist {
        comments: gist.comments.unwrap_or_default(),
        comments_enabled: gist.comments_enabled,
        comments_url: gist
            .comments_url
            .clone()
            .expect("created gist has comments_url"),
        commits_url: gist
            .commits_url
            .clone()
            .expect("created gist has commits_url"),
        created_at: crate::render::timestamp(
            gist.created_at
                .as_deref()
                .expect("created gist has created_at"),
        ),
        description: gist.description.clone(),
        files: gist
            .files
            .iter()
            .map(|(filename, file)| {
                let file = file.as_ref().expect("created gist has file metadata");
                (
                    filename.clone(),
                    types::BaseGistFilesValue {
                        encoding: file.encoding.clone(),
                        filename: file.filename.clone(),
                        language: file.language.clone(),
                        raw_url: file.raw_url.clone(),
                        size: file.size,
                        type_: file.type_.clone(),
                    },
                )
            })
            .collect(),
        forks: Vec::new(),
        forks_url: gist.forks_url.clone().expect("created gist has forks_url"),
        git_pull_url: gist
            .git_pull_url
            .clone()
            .expect("created gist has git_pull_url"),
        git_push_url: gist
            .git_push_url
            .clone()
            .expect("created gist has git_push_url"),
        history: Vec::new(),
        html_url: gist.html_url.clone().expect("created gist has html_url"),
        id: gist.id.clone().expect("created gist has id"),
        node_id: gist.node_id.clone().expect("created gist has node_id"),
        owner: gist.owner.clone(),
        public: gist.public.unwrap_or_default(),
        truncated: gist.truncated,
        updated_at: crate::render::timestamp(
            gist.updated_at
                .as_deref()
                .expect("created gist has updated_at"),
        ),
        url: gist.url.clone().expect("created gist has url"),
        user: None,
    }
}
