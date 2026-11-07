//! Git commit objects: create and read commits in the hosted object database.

use std::sync::Mutex;

use chrono::{DateTime, Utc};

use super::Handler;
use super::git_store::{
    Person, find_existing_object, git_committer_from_signature, git_user_from_signature, internal,
    locate, locate_writable, object_id, object_node_id, open_repo, unsigned_commit_verification,
};
use crate::api::{ApiRequest, ApiResponse, ApiResult};
use crate::operation::ops;
use crate::render;
use crate::state::{State, lock};
use crate::types;

fn commit_url(request: &ApiRequest, located: &super::git_store::Located, sha: &str) -> String {
    format!("{}/git/commits/{sha}", located.api(request))
}

fn render_git_commit(
    request: &ApiRequest,
    located: &super::git_store::Located,
    commit: &ninot_common::git::Commit,
) -> types::GitCommit {
    let api = located.api(request);
    let html = located.html(request);
    types::GitCommit {
        author: git_user_from_signature(&commit.author),
        committer: git_committer_from_signature(&commit.committer),
        html_url: format!("{html}/commit/{}", commit.sha),
        message: commit.message.clone(),
        node_id: object_node_id("Commit", &commit.sha),
        parents: commit
            .parents
            .iter()
            .map(|parent| types::GitCommitParentsItem {
                html_url: format!("{html}/commit/{parent}"),
                sha: parent.clone(),
                url: format!("{api}/git/commits/{parent}"),
            })
            .collect(),
        sha: commit.sha.clone(),
        tree: types::GitCommitTree {
            sha: commit.tree.clone(),
            url: format!("{api}/git/trees/{}", commit.tree),
        },
        url: format!("{api}/git/commits/{}", commit.sha),
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
            .map_err(|_| ApiResponse::error(422, "tree is not a valid git object"))?;
        if header.kind() != gix::objs::Kind::Tree {
            return Err(ApiResponse::error(422, "tree is not a valid git object"));
        }
        let mut parents = Vec::new();
        for parent in &body.parents {
            let id = object_id(parent)?;
            let header = repo
                .find_header(id)
                .map_err(|_| ApiResponse::error(422, "parent is not a valid git object"))?;
            if header.kind() != gix::objs::Kind::Commit {
                return Err(ApiResponse::error(422, "parent is not a valid git object"));
            }
            parents.push(id);
        }
        let now = render::timestamp(&crate::time::now());
        let state = lock(state);
        let account = state
            .account(&caller.login)
            .expect("authenticated callers have accounts");
        let default = Person::from_account(account, now);
        drop(state);
        let author = match &body.author {
            Some(author) => signature_from_request(&author.name, &author.email, author.date, now)?,
            None => default.signature(),
        };
        let committer = match &body.committer {
            Some(committer) => {
                let name = committer.name.as_deref().unwrap_or(&default.name);
                let email = committer.email.as_deref().unwrap_or(&default.email);
                signature_from_request(name, email, committer.date, now)?
            }
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
            .map_err(internal)?
            .detach()
            .to_string();
        drop(repo);
        let stored = ninot_common::git::commit(&located.git_dir, &sha).map_err(internal)?;
        let url = commit_url(request, &located, &sha);
        Ok(ApiResponse::typed(
            201,
            &types::GitCreateCommitResponse(render_git_commit(request, &located, &stored)),
        )
        .with_header("location", url))
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
        let stored =
            ninot_common::git::commit(&located.git_dir, &id.to_string()).map_err(internal)?;
        Ok(ApiResponse::typed(
            200,
            &types::GitGetCommitResponse(render_git_commit(request, &located, &stored)),
        ))
    }
}
