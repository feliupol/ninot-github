//! Git commit objects: create and read commits in the hosted object database.

use std::sync::Mutex;

use chrono::{DateTime, Utc};

use super::Handler;
use super::git_store::{
    Person, find_existing_object, git_committer_from_signature, git_user_from_signature, locate,
    locate_writable, object_id, object_node_id, open_repo, unsigned_commit_verification,
};
use crate::api::{ApiRequest, ApiResponse, ApiResult};
use crate::operation::ops;
use crate::state::{State, lock};
use crate::types;

fn commit_url(request: &ApiRequest, located: &super::git_store::Located, sha: &str) -> String {
    located.urls(request).commit(sha).git
}

fn render_git_commit(
    request: &ApiRequest,
    located: &super::git_store::Located,
    commit: &ninot_common::git::Commit,
) -> types::GitCommit {
    let repo = located.urls(request);
    let urls = repo.commit(&commit.sha);
    types::GitCommit {
        author: git_user_from_signature(&commit.author),
        committer: git_committer_from_signature(&commit.committer),
        html_url: urls.html,
        message: commit.message.clone(),
        node_id: object_node_id("Commit", &commit.sha),
        parents: commit
            .parents
            .iter()
            .map(|parent| {
                let urls = repo.commit(parent);
                types::GitCommitParentsItem {
                    html_url: urls.html,
                    sha: parent.clone(),
                    url: urls.git,
                }
            })
            .collect(),
        sha: commit.sha.clone(),
        tree: types::GitCommitTree {
            sha: commit.tree.clone(),
            url: repo.git_tree(&commit.tree),
        },
        url: urls.git,
        verification: unsigned_commit_verification(),
    }
}

fn signature_from_request(
    name: &str,
    email: &str,
    date: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Result<gix::actor::Signature, ApiResponse> {
    Ok(Person::named(name, email, date, now)?.signature())
}

impl Handler for ops::GitCreateCommit {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let (caller, located) = locate_writable(state, request)?;
        let body: types::GitCreateCommitRequest = request.json_as()?;
        let repo = open_repo(&located.git_dir)?;
        let tree = object_id(&body.tree)?;
        let header = repo
            .find_header(tree)
            .map_err(|_| crate::errors::GithubError::TreeNotObject.to_response())?;
        if header.kind() != gix::objs::Kind::Tree {
            return Err(crate::errors::GithubError::TreeNotObject.to_response());
        }
        let mut parents = Vec::new();
        for parent in &body.parents {
            let id = object_id(parent)?;
            let header = repo
                .find_header(id)
                .map_err(|_| crate::errors::GithubError::ParentNotObject.to_response())?;
            if header.kind() != gix::objs::Kind::Commit {
                return Err(crate::errors::GithubError::ParentNotObject.to_response());
            }
            parents.push(id);
        }
        let now = crate::time::now_utc();
        let state = lock(state);
        let account = state
            .account(&caller.login)
            .expect("authenticated callers have accounts");
        let default = Person::from_account(account, now);
        drop(state);
        let author = match &body.author {
            Some(author) => signature_from_request(&author.name, &author.email, author.date, now)?,
            // Default author: the authenticated user and the current date.
            // Source: <https://docs.github.com/en/rest/git/commits#create-a-commit>
            None => default.signature(),
        };
        let committer = match &body.committer {
            Some(committer) => {
                let name = committer.name.as_deref().unwrap_or(&default.name);
                let email = committer.email.as_deref().unwrap_or(&default.email);
                signature_from_request(name, email, committer.date, now)?
            }
            // Default committer: the author.
            // Source: <https://docs.github.com/en/rest/git/commits#create-a-commit>
            None => author.clone(),
        };
        let mut extra_headers = Vec::new();
        if let Some(signature) = &body.signature {
            extra_headers.push(("gpgsig".into(), signature.as_str().into()));
        }
        let commit = gix::objs::Commit {
            tree,
            parents: parents.into(),
            author,
            committer,
            encoding: None,
            message: body.message.as_str().into(),
            extra_headers,
        };
        let sha = repo
            .write_object(&commit)
            .map_err(ApiResponse::internal)?
            .detach()
            .to_string();
        drop(repo);
        let stored =
            ninot_common::git::commit(&located.git_dir, &sha).map_err(ApiResponse::internal)?;
        let url = commit_url(request, &located, &sha);
        Ok(
            ApiResponse::created(&types::GitCreateCommitResponse(render_git_commit(
                request, &located, &stored,
            )))
            .with_header("location", url),
        )
    }
}

impl Handler for ops::GitGetCommit {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let located = locate(state, request)?;
        let commit_sha = request.param("commit_sha");
        let id = find_existing_object(&located.git_dir, commit_sha)?;
        let repo = open_repo(&located.git_dir)?;
        let header = repo.find_header(id).map_err(|_| ApiResponse::not_found())?;
        if header.kind() != gix::objs::Kind::Commit {
            return Err(ApiResponse::not_found());
        }
        drop(repo);
        let stored = ninot_common::git::commit(&located.git_dir, &id.to_string())
            .map_err(ApiResponse::internal)?;
        Ok(ApiResponse::ok_typed(&types::GitGetCommitResponse(
            render_git_commit(request, &located, &stored),
        )))
    }
}
