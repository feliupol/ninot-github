//! Root directory listings for `GET /repos/{owner}/{repo}/contents`.

use std::path::Path;

use super::git_store::{object_id, open_repo};
use crate::api::ApiResponse;
use ninot_common::git::{PathContent, TreeEntry};

/// The tree at `sha`, as the directory listing `path_content` would build for a subdirectory.
pub(super) fn at_root(git_dir: &Path, sha: &str) -> Result<PathContent, ApiResponse> {
    let repo = open_repo(git_dir)?;
    let tree = repo
        .find_commit(object_id(sha)?)
        .map_err(ApiResponse::internal)?
        .tree()
        .map_err(ApiResponse::internal)?;
    let mut children = Vec::new();
    for child in tree.iter() {
        let child = child.map_err(ApiResponse::internal)?;
        let mode = child.mode();
        let id = child.object_id();
        let (kind, size) = if mode.is_tree() {
            ("tree", 0)
        } else if mode.is_commit() {
            ("commit", 0)
        } else {
            (
                "blob",
                repo.find_header(id).map_err(ApiResponse::internal)?.size(),
            )
        };
        children.push(TreeEntry {
            kind: kind.to_owned(),
            mode: format!("{:06o}", mode.value()),
            sha: id.to_string(),
            size,
            path: child.filename().to_string(),
        });
    }
    Ok(PathContent::Directory(children))
}
