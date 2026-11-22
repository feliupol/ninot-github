//! Unified diffs and mbox patches for the diff and patch media types.
//!
//! `GET` a pull request, a commit, or a compare with `Accept: application/vnd.github.diff`
//! (or `.patch`, with or without the `.v3` version) and the body is text, as GitHub serves it.
//! Source: <https://docs.github.com/en/rest/pulls/pulls#get-a-pull-request>

use std::path::Path;

use super::{diff_patch, diff_text};
use crate::api::{ApiRequest, ApiResponse};
use crate::types;

const TEXT: &str = "text/plain; charset=utf-8";

enum Kind {
    Diff,
    Patch,
}

/// Diff or patch when `Accept` asks for one of those media types.
fn kind(request: &ApiRequest) -> Option<Kind> {
    for token in request.accept().split(',') {
        let token = token.split(';').next().unwrap_or("").trim();
        if vendor(token, "diff") {
            return Some(Kind::Diff);
        }
        if vendor(token, "patch") {
            return Some(Kind::Patch);
        }
    }
    None
}

/// `application/vnd.github.diff` and `application/vnd.github.v3.diff`, and the patch forms.
fn vendor(token: &str, suffix: &str) -> bool {
    let Some(rest) = token.strip_prefix("application/vnd.github.") else {
        return false;
    };
    rest == suffix || rest.strip_prefix("v3.").is_some_and(|rest| rest == suffix)
}

fn text(bytes: Vec<u8>) -> ApiResponse {
    ApiResponse::bytes_ok(TEXT, bytes)
}

/// The pull request's three-dot diff or mbox patch, when `Accept` asks for it.
pub(crate) fn pull(
    request: &ApiRequest,
    git_dir: Option<&Path>,
    pull: &types::PullRequest,
) -> Result<Option<ApiResponse>, ApiResponse> {
    let Some(kind) = kind(request) else {
        return Ok(None);
    };
    let Some(git_dir) = git_dir else {
        return Ok(Some(text(Vec::new())));
    };
    Ok(Some(text(range(
        git_dir,
        kind,
        &pull.base.sha,
        &pull.head.sha,
    )?)))
}

/// Diff or patch of `sha`, when `Accept` asks for it.
pub(crate) fn commit(
    request: &ApiRequest,
    git_dir: &Path,
    sha: &str,
) -> Result<Option<ApiResponse>, ApiResponse> {
    let Some(kind) = kind(request) else {
        return Ok(None);
    };
    let bytes = match kind {
        Kind::Diff => diff_text::commit_diff(git_dir, sha)?,
        Kind::Patch => diff_patch::commit_patch(git_dir, sha)?,
    };
    Ok(Some(text(bytes)))
}

/// Three-dot diff or patch of `base...head`, when `Accept` asks for it.
pub(crate) fn compare(
    request: &ApiRequest,
    git_dir: &Path,
    base: &str,
    head: &str,
) -> Result<Option<ApiResponse>, ApiResponse> {
    let Some(kind) = kind(request) else {
        return Ok(None);
    };
    Ok(Some(text(range(git_dir, kind, base, head)?)))
}

/// Three-dot diff, or format-patch of the commits reachable from `head` since the merge base.
fn range(git_dir: &Path, kind: Kind, base: &str, head: &str) -> Result<Vec<u8>, ApiResponse> {
    match kind {
        Kind::Diff => diff_text::range_diff(git_dir, base, head),
        Kind::Patch => diff_patch::range_patch(git_dir, base, head),
    }
}
