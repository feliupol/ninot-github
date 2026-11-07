//! Git blobs: create and read file contents in the hosted object database.

use std::sync::Mutex;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;

use super::Handler;
use super::git_store::{
    empty_repository, find_existing_object, internal, locate, locate_writable, object_node_id,
    open_repo, wrapped_base64, write_blob,
};
use crate::api::{ApiRequest, ApiResponse, ApiResult};
use crate::operation::ops;
use crate::state::State;
use crate::types;

/// Decodes blob content according to GitHub's `utf-8` and `base64` encodings.
fn decode_content(content: &str, encoding: &str) -> Result<Vec<u8>, ApiResponse> {
    match encoding {
        "utf-8" => Ok(content.as_bytes().to_vec()),
        "base64" => STANDARD
            .decode(content.split_whitespace().collect::<String>())
            .map_err(|_| ApiResponse::error(422, "content is not valid Base64")),
        _ => Err(ApiResponse::error(
            422,
            &format!("The encoding '{encoding}' is not supported"),
        )),
    }
}

fn blob_url(request: &ApiRequest, located: &super::git_store::Located, sha: &str) -> String {
    format!("{}/git/blobs/{sha}", located.api(request))
}

impl Handler for ops::GitCreateBlob {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let (_caller, located) = locate_writable(state, request)?;
        let body: types::GitCreateBlobRequest = request.json_as()?;
        let bytes = decode_content(&body.content, &body.encoding)?;
        let sha = write_blob(&located.git_dir, &bytes)?;
        let url = blob_url(request, &located, &sha);
        Ok(ApiResponse::typed(
            201,
            &types::GitCreateBlobResponse(types::ShortBlob {
                sha: sha.clone(),
                url: url.clone(),
            }),
        )
        .with_header("location", url))
    }
}

impl Handler for ops::GitGetBlob {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let located = locate(state, request)?;
        let file_sha = request.param("file_sha");
        if file_sha.chars().any(|ch| !ch.is_ascii_hexdigit()) {
            return Err(ApiResponse::validation_failed("Git", "sha", "invalid"));
        }
        let id = find_existing_object(&located.git_dir, file_sha)?;
        let repo = open_repo(&located.git_dir)?;
        let header = repo.find_header(id).map_err(|_| {
            if ninot_common::git::is_empty(&located.git_dir).unwrap_or(false) {
                empty_repository()
            } else {
                ApiResponse::not_found()
            }
        })?;
        if header.kind() != gix::objs::Kind::Blob {
            return Err(ApiResponse::not_found());
        }
        let bytes = repo.find_blob(id).map_err(internal)?.data.clone();
        if request.accept().contains("application/vnd.github.raw") {
            return Ok(ApiResponse::bytes(200, "application/octet-stream", bytes));
        }
        let sha = id.to_string();
        let url = blob_url(request, &located, &sha);
        Ok(ApiResponse::typed(
            200,
            &types::GitGetBlobResponse(types::Blob {
                content: wrapped_base64(&bytes),
                encoding: "base64".to_owned(),
                highlighted_content: None,
                node_id: object_node_id("Blob", &sha),
                sha,
                size: Some(bytes.len() as i64),
                url,
            }),
        ))
    }
}
