//! Git trees: create and read directory snapshots in the hosted object database.

use std::sync::Mutex;

use gix::objs::tree::EntryKind;

use super::Handler;
use super::git_store::{
    empty_repository, find_existing_object, internal, locate, locate_writable, object_id,
    open_repo, tree_entry_kind, write_blob_in,
};
use crate::api::{ApiRequest, ApiResponse, ApiResult};
use crate::operation::ops;
use crate::state::State;
use crate::types;

fn tree_url(request: &ApiRequest, located: &super::git_store::Located, sha: &str) -> String {
    format!("{}/git/trees/{sha}", located.api(request))
}

fn entry_url(api: &str, kind: EntryKind, sha: &str) -> String {
    let collection = match kind {
        EntryKind::Tree => "trees",
        EntryKind::Commit => "commits",
        EntryKind::Blob | EntryKind::BlobExecutable | EntryKind::Link => "blobs",
    };
    format!("{api}/git/{collection}/{sha}")
}

fn github_type(kind: EntryKind) -> String {
    match kind {
        EntryKind::Tree => "tree",
        EntryKind::Commit => "commit",
        EntryKind::Blob | EntryKind::BlobExecutable | EntryKind::Link => "blob",
    }
    .to_owned()
}

fn list_tree(
    repo: &gix::Repository,
    located: &super::git_store::Located,
    request: &ApiRequest,
    tree_id: gix::ObjectId,
    recursive: bool,
) -> Result<(String, Vec<types::GitTreeTreeItem>), ApiResponse> {
    let api = located.api(request);
    let mut items = Vec::new();
    collect_tree(repo, &api, tree_id, "", recursive, &mut items)?;
    Ok((tree_id.to_string(), items))
}

fn collect_tree(
    repo: &gix::Repository,
    api: &str,
    tree_id: gix::ObjectId,
    prefix: &str,
    recursive: bool,
    items: &mut Vec<types::GitTreeTreeItem>,
) -> Result<(), ApiResponse> {
    let tree = repo.find_tree(tree_id).map_err(internal)?;
    for entry in tree.iter() {
        let entry = entry.map_err(internal)?;
        let name = entry.filename().to_string();
        let path = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        let kind = entry.kind();
        let sha = entry.object_id().to_string();
        let size = match kind {
            EntryKind::Blob | EntryKind::BlobExecutable | EntryKind::Link => {
                let header = repo.find_header(entry.object_id()).map_err(internal)?;
                Some(header.size() as i64)
            }
            EntryKind::Tree | EntryKind::Commit => None,
        };
        items.push(types::GitTreeTreeItem {
            mode: format!("{:06o}", entry.mode().value()),
            path,
            sha,
            size,
            type_: github_type(kind),
            url: Some(entry_url(api, kind, &entry.object_id().to_string())),
        });
        if recursive && kind == EntryKind::Tree {
            let child_path = items
                .last()
                .map(|item| item.path.clone())
                .unwrap_or_default();
            collect_tree(repo, api, entry.object_id(), &child_path, true, items)?;
        }
    }
    Ok(())
}

fn peel_to_tree(repo: &gix::Repository, spec: &str) -> Result<gix::ObjectId, ApiResponse> {
    let id = super::git_store::resolve_object(repo, spec)?
        .ok_or_else(|| ApiResponse::error(422, "The base_tree is not a valid git tree"))?;
    let object = repo.find_object(id).map_err(internal)?;
    Ok(object
        .peel_to_tree()
        .map_err(|_| ApiResponse::error(422, "The base_tree is not a valid git tree"))?
        .id)
}

fn entry_sha_and_content(
    item: &types::GitCreateTreeRequestTreeItem,
) -> Result<TreeEntrySource, ApiResponse> {
    let sha = item.sha.clone();
    let content = item.content.clone();
    match (sha, content) {
        (Some(None), None) => Ok(TreeEntrySource::Delete),
        (Some(Some(_)), Some(_)) | (Some(None), Some(_)) => Err(ApiResponse::error(
            422,
            "Cannot specify both sha and content for a tree entry",
        )),
        (Some(Some(sha)), None) => Ok(TreeEntrySource::Sha(sha)),
        (None, Some(content)) => Ok(TreeEntrySource::Content(content)),
        (None, None) => Err(ApiResponse::validation_failed(
            "Git",
            "sha",
            "missing_field",
        )),
    }
}

enum TreeEntrySource {
    Delete,
    Sha(String),
    Content(String),
}

impl Handler for ops::GitCreateTree {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let (_caller, located) = locate_writable(state, request)?;
        let body: types::GitCreateTreeRequest = request.json_as()?;
        let repo = open_repo(&located.git_dir)?;
        let base = match &body.base_tree {
            Some(base) => peel_to_tree(&repo, base)?,
            None => gix::ObjectId::empty_tree(repo.object_hash()),
        };
        let mut editor = repo.edit_tree(base).map_err(internal)?;
        for item in &body.tree {
            let path = item
                .path
                .as_deref()
                .filter(|path| !path.is_empty())
                .ok_or_else(|| ApiResponse::validation_failed("Git", "path", "missing_field"))?;
            let mode = item
                .mode
                .as_ref()
                .map(std::string::ToString::to_string)
                .ok_or_else(|| ApiResponse::validation_failed("Git", "mode", "missing_field"))?;
            let type_ = item
                .type_
                .as_ref()
                .map(std::string::ToString::to_string)
                .ok_or_else(|| ApiResponse::validation_failed("Git", "type", "missing_field"))?;
            let kind = tree_entry_kind(&mode, &type_)?;
            match entry_sha_and_content(item)? {
                TreeEntrySource::Delete => {
                    if editor.get(path).is_none() {
                        return Err(ApiResponse::error(
                            422,
                            &format!("The path {path} does not exist in the tree"),
                        ));
                    }
                    editor.remove(path).map_err(internal)?;
                }
                TreeEntrySource::Content(content) => {
                    if kind != EntryKind::Blob
                        && kind != EntryKind::BlobExecutable
                        && kind != EntryKind::Link
                    {
                        return Err(ApiResponse::validation_failed("Git", "content", "invalid"));
                    }
                    let sha = write_blob_in(&repo, content.as_bytes())?;
                    let id = object_id(&sha)?;
                    editor.upsert(path, kind, id).map_err(internal)?;
                }
                TreeEntrySource::Sha(sha) => {
                    let id = object_id(&sha)?;
                    if !repo.has_object(id) {
                        return Err(ApiResponse::error(422, "The git object does not exist."));
                    }
                    editor.upsert(path, kind, id).map_err(internal)?;
                }
            }
        }
        let tree_id = editor.write().map_err(internal)?.detach();
        let (_, items) = list_tree(&repo, &located, request, tree_id, false)?;
        let sha = tree_id.to_string();
        let url = tree_url(request, &located, &sha);
        Ok(ApiResponse::typed(
            201,
            &types::GitCreateTreeResponse(types::GitTree {
                sha,
                tree: items,
                truncated: false,
                url: Some(url.clone()),
            }),
        )
        .with_header("location", url))
    }
}

impl Handler for ops::GitGetTree {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let located = locate(state, request)?;
        let tree_sha = request.param("tree_sha");
        let repo = open_repo(&located.git_dir)?;
        let Some(id) = super::git_store::resolve_object(&repo, tree_sha)? else {
            drop(repo);
            let _ = find_existing_object(&located.git_dir, tree_sha)?;
            return Err(ApiResponse::not_found());
        };
        let tree_id = repo
            .find_object(id)
            .map_err(|_| {
                if ninot_common::git::is_empty(&located.git_dir).unwrap_or(false) {
                    empty_repository()
                } else {
                    ApiResponse::not_found()
                }
            })?
            .peel_to_tree()
            .map_err(|_| ApiResponse::validation_failed("Git", "sha", "invalid"))?
            .id;
        let recursive = request.query("recursive").is_some();
        let (sha, items) = list_tree(&repo, &located, request, tree_id, recursive)?;
        let url = tree_url(request, &located, &sha);
        Ok(ApiResponse::typed(
            200,
            &types::GitGetTreeResponse(types::GitTree {
                sha,
                tree: items,
                truncated: false,
                url: Some(url),
            }),
        ))
    }
}
