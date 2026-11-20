//! Unified diffs and mbox patches for the diff and patch media types.
//!
//! `GET` a pull request, a commit, or a compare with `Accept: application/vnd.github.diff`
//! (or `.patch`, with or without the `.v3` version) and the body is text, as GitHub serves it.
//! Source: <https://docs.github.com/en/rest/pulls/pulls#get-a-pull-request>

use std::path::Path;
use std::process::{Command, Stdio};

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
    ApiResponse::bytes(200, TEXT, bytes)
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

/// `git show` of `sha` as a diff, or `git format-patch -1`, when `Accept` asks for it.
pub(crate) fn commit(
    request: &ApiRequest,
    git_dir: &Path,
    sha: &str,
) -> Result<Option<ApiResponse>, ApiResponse> {
    let Some(kind) = kind(request) else {
        return Ok(None);
    };
    let bytes = match kind {
        Kind::Diff => git(
            git_dir,
            &[
                "diff-tree",
                "--root",
                "--no-commit-id",
                "-p",
                "--no-color",
                "--no-renames",
                "--src-prefix=a/",
                "--dst-prefix=b/",
                sha,
            ],
        )?,
        Kind::Patch => git(git_dir, &["format-patch", "-1", "--stdout", sha])?,
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

/// `git diff base...head`, or `format-patch` of the commits reachable from `head` since the merge base.
fn range(git_dir: &Path, kind: Kind, base: &str, head: &str) -> Result<Vec<u8>, ApiResponse> {
    match kind {
        Kind::Diff => {
            let spec = format!("{base}...{head}");
            git(
                git_dir,
                &[
                    "diff",
                    "--no-color",
                    "--no-renames",
                    "--src-prefix=a/",
                    "--dst-prefix=b/",
                    &spec,
                ],
            )
        }
        Kind::Patch => {
            let merge_base = merge_base(git_dir, base, head)?;
            let spec = format!("{merge_base}..{head}");
            git(git_dir, &["format-patch", "--stdout", &spec])
        }
    }
}

fn merge_base(git_dir: &Path, base: &str, head: &str) -> Result<String, ApiResponse> {
    let output = git(git_dir, &["merge-base", base, head])?;
    let sha = String::from_utf8_lossy(&output).trim().to_owned();
    if sha.is_empty() {
        return Err(ApiResponse::internal("no merge base"));
    }
    Ok(sha)
}

fn git(git_dir: &Path, args: &[&str]) -> Result<Vec<u8>, ApiResponse> {
    let output = Command::new("git")
        .arg("--git-dir")
        .arg(git_dir)
        .args([
            "-c",
            "color.ui=false",
            "-c",
            "diff.mnemonicPrefix=false",
            "-c",
            "diff.noprefix=false",
            "-c",
            "core.quotepath=false",
        ])
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .stdin(Stdio::null())
        .output()
        .map_err(ApiResponse::internal)?;
    if output.status.success() {
        return Ok(output.stdout);
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(ApiResponse::internal(format!(
        "git {} failed: {}",
        args.first().copied().unwrap_or("command"),
        stderr.trim()
    )))
}
