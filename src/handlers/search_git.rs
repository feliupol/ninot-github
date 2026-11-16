//! Commit and code search over the default branch of every repository the caller can read.
//!
//! Source: <https://docs.github.com/en/rest/search/search#search-commits> and
//! <https://docs.github.com/en/rest/search/search#search-code>

use std::path::Path;
use std::sync::Mutex;

use ninot_common::git;

use super::Handler;
use super::search_more::Query;
use crate::api::{ApiRequest, ApiResponse, ApiResult, paginate_with};
use crate::operation::ops;
use crate::render;
use crate::state::{Account, AccountKind, Permission, Repo, State, lock};
use crate::types;

/// Files larger than this are not indexed by code search.
/// Source: <https://docs.github.com/en/search-github/searching-on-github/searching-code#considerations-for-code-search>
const MAX_INDEXED_FILE: u64 = 384 * 1024;

/// A readable repository the query's `repo:`, `user:` and `org:` qualifiers keep, with the git
/// directory and default-branch commit of a non-empty one.
struct Searched<'a> {
    account: &'a Account,
    repo: &'a Repo,
    permission: Permission,
    git_dir: &'a Path,
    head: String,
}

fn searched<'a>(state: &'a State, request: &ApiRequest, query: &Query) -> Vec<Searched<'a>> {
    let mut found = Vec::new();
    for account in state.accounts() {
        let owner_ok = match account.kind {
            AccountKind::User => query.values("org").next().is_none(),
            AccountKind::Organization => query.values("user").next().is_none(),
        } && query.allows("user", &account.login)
            && query.allows("org", &account.login);
        if !owner_ok {
            continue;
        }
        for repo in account.repos() {
            let full_name = format!("{}/{}", account.login, repo.name);
            let Some(permission) =
                state.permission(request.caller.as_ref(), &account.login, &repo.name)
            else {
                continue;
            };
            let Some(git_dir) = repo.git_dir.as_deref() else {
                continue;
            };
            let head = git::resolve_commit(git_dir, &format!("refs/heads/{}", repo.default_branch));
            if let (true, Ok(Some(head))) = (query.allows("repo", &full_name), head) {
                found.push(Searched {
                    account,
                    repo,
                    permission,
                    git_dir,
                    head,
                });
            }
        }
    }
    found
}

fn minimal(request: &ApiRequest, searched: &Searched<'_>) -> types::MinimalRepository {
    render::minimal_repository(
        request,
        searched.account,
        searched.repo,
        searched.permission,
    )
}

fn timestamp(date: &str) -> chrono::DateTime<chrono::Utc> {
    crate::time::parse(date).expect("git dates are ISO 8601")
}

fn git_user(signature: &git::Signature) -> types::NullableGitUser {
    types::NullableGitUser {
        date: Some(timestamp(&signature.date)),
        email: Some(signature.email.clone()),
        name: Some(signature.name.clone()),
    }
}

/// Whether `commit` matches the message keywords and the signature and hash qualifiers.
/// Source: <https://docs.github.com/en/search-github/searching-on-github/searching-commits>
fn commit_matches(query: &Query, commit: &git::Commit) -> bool {
    query.keywords_in(&[Some(&commit.message)])
        && query.allows("author-name", &commit.author.name)
        && query.allows("author-email", &commit.author.email)
        && query.allows("committer-name", &commit.committer.name)
        && query.allows("committer-email", &commit.committer.email)
        && query
            .values("hash")
            .all(|hash| commit.sha.starts_with(hash.to_ascii_lowercase().as_str()))
}

fn commit_item(
    request: &ApiRequest,
    searched: &Searched<'_>,
    commit: git::Commit,
) -> types::CommitSearchResultItem {
    let urls = request
        .urls()
        .repo(&searched.account.login, &searched.repo.name);
    let commit_urls = urls.commit(&commit.sha);
    types::CommitSearchResultItem {
        author: None,
        comments_url: commit_urls.comments(),
        commit: types::CommitSearchResultItemCommit {
            author: types::CommitSearchResultItemCommitAuthor {
                date: timestamp(&commit.author.date),
                email: commit.author.email.clone(),
                name: commit.author.name.clone(),
            },
            comment_count: 0,
            committer: Some(git_user(&commit.committer)),
            message: commit.message.clone(),
            tree: types::CommitSearchResultItemCommitTree {
                url: urls.git_tree(&commit.tree),
                sha: commit.tree.clone(),
            },
            url: commit_urls.git.clone(),
            verification: None,
        },
        committer: Some(git_user(&commit.committer)),
        html_url: commit_urls.html.clone(),
        node_id: super::git_store::object_node_id("Commit", &commit.sha),
        parents: commit
            .parents
            .iter()
            .map(|parent| {
                let parent_urls = urls.commit(parent);
                types::CommitSearchResultItemParentsItem {
                    html_url: Some(parent_urls.html),
                    sha: Some(parent.clone()),
                    url: Some(parent_urls.api),
                }
            })
            .collect(),
        repository: minimal(request, searched),
        score: 1.0,
        sha: commit.sha.clone(),
        text_matches: None,
        url: commit_urls.api,
    }
}

impl Handler for ops::SearchCommits {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let query = Query::from_request(request)?;
        let by_author = match request.query("sort") {
            None | Some("committer-date") => false,
            Some("author-date") => true,
            Some(_) => return Err(ApiResponse::validation_failed("Search", "sort", "invalid")),
        };
        let state = lock(state);
        let mut found = Vec::new();
        for searched in searched(&state, request, &query) {
            let shas = git::new_commits(searched.git_dir, &searched.head, &[])
                .map_err(ApiResponse::internal)?;
            for sha in shas {
                let commit = git::commit(searched.git_dir, &sha).map_err(ApiResponse::internal)?;
                if commit_matches(&query, &commit) {
                    let item = commit_item(request, &searched, commit);
                    found.push(item);
                }
            }
        }
        let date = |item: &types::CommitSearchResultItem| {
            if by_author {
                Some(item.commit.author.date)
            } else {
                item.commit.committer.as_ref().and_then(|user| user.date)
            }
        };
        found.sort_by(|left, right| {
            let ordered = date(left).cmp(&date(right));
            let ordered = if query.ascending {
                ordered
            } else {
                ordered.reverse()
            };
            ordered.then_with(|| left.sha.cmp(&right.sha))
        });
        Ok(paginate_with(
            request,
            &request.path,
            found,
            |total, items| types::SearchCommitsResponse {
                incomplete_results: false,
                items,
                total_count: total,
            },
        ))
    }
}

/// A file of the default branch's tree.
struct File {
    path: String,
    sha: String,
    size: u64,
}

/// Every file of the tree at `commit`, depth first.
fn files(git_dir: &Path, commit: &str) -> Result<Vec<File>, ApiResponse> {
    let repo = super::git_store::open_repo(git_dir)?;
    let tree = repo
        .find_commit(super::git_store::object_id(commit)?)
        .map_err(ApiResponse::internal)?
        .tree()
        .map_err(ApiResponse::internal)?;
    let mut files = Vec::new();
    walk(&repo, &tree, "", &mut files)?;
    Ok(files)
}

fn walk(
    repo: &gix::Repository,
    tree: &gix::Tree<'_>,
    prefix: &str,
    files: &mut Vec<File>,
) -> Result<(), ApiResponse> {
    for entry in tree.iter() {
        let entry = entry.map_err(ApiResponse::internal)?;
        let path = if prefix.is_empty() {
            entry.filename().to_string()
        } else {
            format!("{prefix}/{}", entry.filename())
        };
        let (mode, id) = (entry.mode(), entry.object_id());
        if mode.is_tree() {
            let subtree = repo.find_tree(id).map_err(ApiResponse::internal)?;
            walk(repo, &subtree, &path, files)?;
        } else if mode.is_blob() {
            let size = repo.find_header(id).map_err(ApiResponse::internal)?.size();
            files.push(File {
                path,
                sha: id.to_string(),
                size,
            });
        }
    }
    Ok(())
}

/// Whether the file at `path` passes the path, filename and extension qualifiers.
/// Source: <https://docs.github.com/en/search-github/searching-on-github/searching-code>
fn path_matches(query: &Query, path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    let extension = name.rsplit_once('.').map_or("", |(_, extension)| extension);
    query.values("path").all(|wanted| {
        path.to_lowercase()
            .starts_with(wanted.trim_matches('/').to_lowercase().as_str())
    }) && query.allows("filename", name)
        && query.allows("extension", extension)
}

impl Handler for ops::SearchCode {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        // Code search needs a signed-in user.
        // Source: <https://docs.github.com/en/rest/search/search#search-code>
        request.require_caller()?;
        let query = Query::from_request(request)?;
        let in_path = query
            .values("in")
            .any(|field| field.eq_ignore_ascii_case("path"));
        let state = lock(state);
        let mut items = Vec::new();
        for searched in searched(&state, request, &query) {
            let urls = request
                .urls()
                .repo(&searched.account.login, &searched.repo.name);
            let entries = files(searched.git_dir, &searched.head)?;
            for entry in entries {
                if entry.size > MAX_INDEXED_FILE || !path_matches(&query, &entry.path) {
                    continue;
                }
                let bytes =
                    git::blob(searched.git_dir, &entry.sha).map_err(ApiResponse::internal)?;
                let Ok(text) = std::str::from_utf8(&bytes) else {
                    continue;
                };
                let field = if in_path { entry.path.as_str() } else { text };
                if !query.keywords_in(&[Some(field)]) {
                    continue;
                }
                items.push(types::CodeSearchResultItem {
                    file_size: None,
                    git_url: urls.git_object("blobs", &entry.sha),
                    html_url: urls.blob_html(&searched.head, &entry.path),
                    language: None,
                    last_modified_at: None,
                    line_numbers: Vec::new(),
                    name: entry
                        .path
                        .rsplit('/')
                        .next()
                        .unwrap_or(&entry.path)
                        .to_owned(),
                    path: entry.path.clone(),
                    repository: minimal(request, &searched),
                    score: 1.0,
                    sha: entry.sha.clone(),
                    text_matches: None,
                    url: urls.contents(&entry.path, &searched.head),
                });
            }
        }
        Ok(paginate_with(
            request,
            &request.path,
            items,
            |total, items| types::SearchCodeResponse {
                incomplete_results: false,
                items,
                total_count: total,
            },
        ))
    }
}
