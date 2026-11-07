//! Shared helpers for Git database handlers: locating a repository and writing objects.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use gix::objs::tree::EntryKind;

use super::repos::{Found, find_repo};
use crate::api::{ApiRequest, ApiResponse, Caller};
use crate::render::{self, node_id};
use crate::state::{Account, State, lock};
use crate::types;

/// A visible repository and its bare git repository, with the state lock released.
pub(crate) struct Located {
    /// Owner and name the caller can see, with their permission.
    pub(crate) found: Found,
    /// Bare repository clients fetch and the Git database writes into.
    pub(crate) git_dir: PathBuf,
    /// Default branch of the repository, used when a request omits a ref.
    pub(crate) default_branch: String,
}

impl Located {
    /// REST collection URL of this repository, such as `http://host/repos/o/r`.
    pub(crate) fn api(&self, request: &ApiRequest) -> String {
        format!(
            "{}/repos/{}/{}",
            request.api_url, self.found.owner, self.found.name
        )
    }

    /// HTML URL of this repository, such as `http://host/o/r`.
    pub(crate) fn html(&self, request: &ApiRequest) -> String {
        format!(
            "{}/{}/{}",
            request.web_url, self.found.owner, self.found.name
        )
    }
}

/// Resolves `{owner}/{repo}` and its on-disk git directory.
pub(crate) fn locate(state: &Mutex<State>, request: &ApiRequest) -> Result<Located, ApiResponse> {
    let state = lock(state);
    let found = find_repo(&state, request)?;
    let repo = state
        .account(&found.owner)
        .unwrap()
        .repo(&found.name)
        .unwrap();
    let git_dir = repo
        .git_dir
        .clone()
        .ok_or_else(|| ApiResponse::error(500, "repository has no git storage"))?;
    let default_branch = repo.default_branch.clone();
    Ok(Located {
        found,
        git_dir,
        default_branch,
    })
}

/// Requires contents-write scope and push permission, then locates the repository.
pub(crate) fn locate_writable(
    state: &Mutex<State>,
    request: &ApiRequest,
) -> Result<(Caller, Located), ApiResponse> {
    let caller = request.require_scope(&["repo", "public_repo"])?.clone();
    let located = locate(state, request)?;
    if !located.found.permission.push {
        return Err(ApiResponse::error(
            403,
            "Resource not accessible by integration",
        ));
    }
    Ok((caller, located))
}

/// Wraps a gitoxide or I/O failure as an internal server error.
pub(crate) fn internal(error: impl std::fmt::Display) -> ApiResponse {
    ApiResponse::error(500, &error.to_string())
}

/// GitHub's answer for git data in a repository without commits.
pub(crate) fn empty_repository() -> ApiResponse {
    ApiResponse::error(409, "Git Repository is empty.")
}

/// Opens the bare repository, ignoring the user's and system's configuration.
pub(crate) fn open_repo(git_dir: &Path) -> Result<gix::Repository, ApiResponse> {
    gix::open_opts(git_dir, gix::open::Options::isolated()).map_err(internal)
}

/// Parses a full git object id, or 422 when it is not 40 hex digits.
pub(crate) fn object_id(sha: &str) -> Result<gix::ObjectId, ApiResponse> {
    gix::ObjectId::from_hex(sha.as_bytes())
        .map_err(|_| ApiResponse::validation_failed("Git", "sha", "invalid"))
}

/// Resolves `spec` to an object id without peeling, as GitHub does for git data SHAs and refs.
pub(crate) fn resolve_object(
    repo: &gix::Repository,
    spec: &str,
) -> Result<Option<gix::ObjectId>, ApiResponse> {
    if spec.starts_with('-') || spec.is_empty() {
        return Ok(None);
    }
    Ok(repo.rev_parse_single(spec).ok().map(|id| id.detach()))
}

/// Looks up `spec` or answers GitHub's empty-repository 409 / not-found 404.
pub(crate) fn find_existing_object(
    git_dir: &Path,
    spec: &str,
) -> Result<gix::ObjectId, ApiResponse> {
    let repo = open_repo(git_dir)?;
    match resolve_object(&repo, spec)? {
        Some(id) => Ok(id),
        None if ninot_common::git::is_empty(git_dir).map_err(internal)? => Err(empty_repository()),
        None => Err(ApiResponse::not_found()),
    }
}

/// Unsigned verification object GitHub attaches to unsigned tags.
pub(crate) fn unsigned_verification() -> types::Verification {
    types::Verification {
        payload: None,
        reason: "unsigned".to_owned(),
        signature: None,
        verified: false,
        verified_at: None,
    }
}

/// Unsigned verification object GitHub attaches to unsigned git-commit responses.
pub(crate) fn unsigned_commit_verification() -> types::GitCommitVerification {
    types::GitCommitVerification {
        payload: None,
        reason: "unsigned".to_owned(),
        signature: None,
        verified: false,
        verified_at: None,
    }
}

/// Base64 wrapped at 60 columns, as GitHub encodes blob contents.
pub(crate) fn wrapped_base64(bytes: &[u8]) -> String {
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD;
    let encoded = STANDARD.encode(bytes);
    let mut wrapped = String::with_capacity(encoded.len() + encoded.len() / 60 + 1);
    for chunk in encoded.as_bytes().chunks(60) {
        wrapped.push_str(std::str::from_utf8(chunk).unwrap());
        wrapped.push('\n');
    }
    wrapped
}

/// Author or committer taken from the request, or the authenticated user at `now`.
pub(crate) struct Person {
    /// Git user name.
    pub name: String,
    /// Git email address.
    pub email: String,
    /// Instant the person authored or committed, in UTC.
    pub at: DateTime<Utc>,
}

impl Person {
    /// Builds a person, requiring a non-empty name and email.
    pub(crate) fn named(
        name: &str,
        email: &str,
        date: Option<DateTime<Utc>>,
        now: DateTime<Utc>,
    ) -> Result<Self, ApiResponse> {
        if name.trim().is_empty() || email.trim().is_empty() {
            return Err(ApiResponse::validation_failed(
                "Git",
                if name.trim().is_empty() {
                    "name"
                } else {
                    "email"
                },
                "missing_field",
            ));
        }
        Ok(Self {
            name: name.to_owned(),
            email: email.to_owned(),
            at: date.unwrap_or(now),
        })
    }

    /// The authenticated user as GitHub uses them when a request omits author or tagger.
    pub(crate) fn from_account(account: &Account, now: DateTime<Utc>) -> Self {
        Self {
            name: account
                .name
                .clone()
                .unwrap_or_else(|| account.login.clone()),
            email: format!("{}+{}@users.noreply.github.com", account.id, account.login),
            at: now,
        }
    }

    /// The person as a git signature GitHub would write into an object.
    pub(crate) fn signature(&self) -> gix::actor::Signature {
        gix::actor::Signature {
            name: self.name.as_str().into(),
            email: self.email.as_str().into(),
            time: gix::date::Time::new(self.at.timestamp(), 0),
        }
    }
}

/// GitHub's `author` object from a stored git signature.
pub(crate) fn git_user_from_signature(
    signature: &ninot_common::git::Signature,
) -> types::GitCommitAuthor {
    types::GitCommitAuthor {
        date: render::timestamp(&signature.date),
        email: signature.email.clone(),
        name: signature.name.clone(),
    }
}

/// GitHub's `committer` object from a stored git signature.
pub(crate) fn git_committer_from_signature(
    signature: &ninot_common::git::Signature,
) -> types::GitCommitCommitter {
    types::GitCommitCommitter {
        date: render::timestamp(&signature.date),
        email: signature.email.clone(),
        name: signature.name.clone(),
    }
}

/// Writes `bytes` as a blob in an already-open repository and returns its sha.
pub(crate) fn write_blob_in(repo: &gix::Repository, bytes: &[u8]) -> Result<String, ApiResponse> {
    Ok(repo
        .write_blob(bytes)
        .map_err(internal)?
        .detach()
        .to_string())
}

/// Writes `bytes` as a blob and returns its sha.
pub(crate) fn write_blob(git_dir: &Path, bytes: &[u8]) -> Result<String, ApiResponse> {
    write_blob_in(&open_repo(git_dir)?, bytes)
}

/// Mode and GitHub `type` of a tree entry the create-tree API accepts.
pub(crate) fn tree_entry_kind(mode: &str, type_: &str) -> Result<EntryKind, ApiResponse> {
    match (mode, type_) {
        ("100644", "blob") => Ok(EntryKind::Blob),
        ("100755", "blob") => Ok(EntryKind::BlobExecutable),
        ("120000", "blob") => Ok(EntryKind::Link),
        ("040000", "tree") => Ok(EntryKind::Tree),
        ("160000", "commit") => Ok(EntryKind::Commit),
        _ => Err(ApiResponse::validation_failed("Git", "mode", "invalid")),
    }
}

/// Opaque node id derived from an object sha so distinct objects stay distinct.
pub(crate) fn object_node_id(kind: &str, sha: &str) -> String {
    let id = u64::from_str_radix(sha.get(..16).unwrap_or(sha), 16).unwrap_or(0);
    node_id(kind, id)
}

/// Whether `old` is an ancestor of `new`, as a fast-forward update requires.
pub(crate) fn is_ancestor(git_dir: &Path, old: &str, new: &str) -> Result<bool, ApiResponse> {
    if old.eq_ignore_ascii_case(new) {
        return Ok(true);
    }
    let reachable = ninot_common::git::new_commits(git_dir, new, &[])
        .map_err(|_| ApiResponse::error(422, "Update is not a fast forward"))?;
    Ok(reachable.iter().any(|sha| sha.eq_ignore_ascii_case(old)))
}
