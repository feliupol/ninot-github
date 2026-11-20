//! Refs, commits, branches and file contents read from the hosted git repositories.

use std::sync::Mutex;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;

use super::Handler;
use super::git_store::{Located, empty_repository, locate, wrapped_base64};
use super::repos::{Found, find_repo};
use crate::api::{ApiRequest, ApiResponse, ApiResult, paginate};
use crate::operation::ops;
use crate::render::{self, node_id};
use crate::state::{State, lock};
use crate::types;
use crate::urls::RepoUrls;
use ninot_common::git::{self, PathContent, TreeEntry};

fn resolve_fork_network_branch(
    state: &Mutex<State>,
    found: &Found,
    target_git_dir: &std::path::Path,
    owner: &str,
    branch: &str,
) -> Result<Option<String>, ApiResponse> {
    let state = lock(state);
    let target = state
        .account(&found.owner)
        .and_then(|account| account.repo(&found.name))
        .expect("located repository exists");
    let lineage = target
        .fork_source
        .clone()
        .unwrap_or_else(|| (found.owner.clone(), found.name.clone()));
    let source = state
        .accounts()
        .find(|account| account.login.eq_ignore_ascii_case(owner))
        .and_then(|account| {
            account.repos().find_map(|candidate| {
                let same_network = candidate.belongs_to_fork_network(&account.login, &lineage);
                let git_dir = candidate.git_dir.as_ref()?;
                (same_network
                    && git::resolve_commit(git_dir, branch)
                        .ok()
                        .flatten()
                        .is_some())
                .then(|| git_dir.clone())
            })
        });
    let Some(source) = source else {
        return Ok(None);
    };
    crate::fixture_git::copy_refs(target_git_dir, &source).map_err(ApiResponse::internal)?;
    git::resolve_commit(target_git_dir, branch).map_err(ApiResponse::internal)
}

/// A git reference as the REST API renders it.
pub(crate) fn render_ref(
    request: &ApiRequest,
    located: &Located,
    found: &git::Ref,
) -> types::GitRef {
    let repo = located.urls(request);
    let object_kind = if found.kind == "tag" {
        "tags"
    } else {
        "commits"
    };
    types::GitRef {
        node_id: node_id("Ref", found.name.len() as u64),
        object: types::GitRefObject {
            sha: found
                .sha
                .clone()
                .try_into()
                .expect("git object ids are 40 hex digits"),
            type_: found.kind.clone(),
            url: repo.git_object(object_kind, &found.sha),
        },
        ref_: found.name.clone(),
        url: repo.git_ref(&found.name),
    }
}

impl Handler for ops::GitGetRef {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let located = locate(state, request)?;
        let name = format!("refs/{}", request.param("ref"));
        let refs = git::refs(&located.git_dir, "refs/").map_err(ApiResponse::internal)?;
        if refs.is_empty() {
            return Err(empty_repository());
        }
        // Source: <https://docs.github.com/en/rest/git/refs#get-a-reference>
        let found = refs
            .iter()
            .find(|found| found.name == name)
            .ok_or_else(ApiResponse::not_found)?;
        Ok(ApiResponse::typed(
            200,
            &render_ref(request, &located, found),
        ))
    }
}

impl Handler for ops::GitCreateRef {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        request.require_scope(&["repo", "public_repo"])?;
        let body: types::GitCreateRefRequest = request.json_as()?;
        // Source: <https://docs.github.com/en/rest/git/refs#create-a-reference>
        if !body.ref_.starts_with("refs/")
            || body.ref_.split('/').count() < 3
            || gix::refs::FullName::try_from(body.ref_.clone()).is_err()
        {
            return Err(ApiResponse::validation_failed(
                "Reference",
                "ref",
                "invalid",
            ));
        }
        let mut state = lock(state);
        let found = find_repo(&state, request)?;
        if !found.permission.push {
            return Err(ApiResponse::error(
                403,
                "Resource not accessible by integration",
            ));
        }
        let (git_dir, default_branch) = {
            let repo = state
                .account(&found.owner)
                .and_then(|account| account.repo(&found.name))
                .expect("repository exists");
            (
                repo.git_dir
                    .clone()
                    .ok_or_else(|| ApiResponse::internal("repository has no git storage"))?,
                repo.default_branch.clone(),
            )
        };
        let object = super::git_store::open_repo(&git_dir)?
            .rev_parse_single(body.sha.as_str())
            .ok()
            .map(|id| id.to_string())
            .ok_or_else(|| ApiResponse::validation_failed("Reference", "sha", "invalid"))?;
        if git::refs(&git_dir, &body.ref_)
            .map_err(ApiResponse::internal)?
            .iter()
            .any(|reference| reference.name == body.ref_)
        {
            return Err(ApiResponse::error(409, "Reference already exists"));
        }
        git::create_ref(&git_dir, &body.ref_, &object, &super::BOT)
            .map_err(ApiResponse::internal)?;
        let located = Located {
            found,
            git_dir: git_dir.clone(),
            default_branch,
        };
        let reference = git::refs(&git_dir, &body.ref_)
            .map_err(ApiResponse::internal)?
            .into_iter()
            .find(|reference| reference.name == body.ref_)
            .ok_or_else(|| ApiResponse::internal("created reference could not be read"))?;
        let response = types::GitCreateRefResponse(render_ref(request, &located, &reference));
        if let Some((short, kind)) = crate::webhooks::events::create::ref_kind(&body.ref_) {
            let mut ctx = crate::webhooks::events::Ctx::new(
                &mut state,
                request,
                &located.found.owner,
                &located.found.name,
            );
            crate::webhooks::events::create::reference(&mut ctx, short, kind)?;
        }
        Ok(ApiResponse::typed(201, &response))
    }
}

impl Handler for ops::GitListMatchingRefs {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let located = locate(state, request)?;
        let refs = git::refs(&located.git_dir, "refs/").map_err(ApiResponse::internal)?;
        if refs.is_empty() {
            return Err(empty_repository());
        }
        // Refs that start with the requested name match.
        // Source: <https://docs.github.com/en/rest/git/refs#list-matching-references>
        let prefix = format!("refs/{}", request.param("ref"));
        let items = refs
            .iter()
            .filter(|found| found.name.starts_with(&prefix))
            .map(|found| render_ref(request, &located, found))
            .collect::<Vec<_>>();
        Ok(ApiResponse::typed(200, &items))
    }
}

/// A git commit as the REST API renders it, for the repository at `repo`.
pub(crate) fn render_commit(repo: &RepoUrls, commit: &git::Commit) -> types::Commit {
    let urls = repo.commit(&commit.sha);
    let signature = |signature: &git::Signature| types::NullableGitUser {
        date: Some(render::timestamp(&signature.date)),
        email: Some(signature.email.clone()),
        name: Some(signature.name.clone()),
    };
    types::Commit {
        author: None,
        comments_url: urls.comments(),
        commit: types::CommitCommit {
            author: Some(signature(&commit.author)),
            comment_count: 0,
            committer: Some(signature(&commit.committer)),
            message: commit.message.clone(),
            tree: types::CommitCommitTree {
                sha: commit.tree.clone(),
                url: repo.git_tree(&commit.tree),
            },
            url: urls.git.clone(),
            verification: Some(types::Verification {
                payload: None,
                reason: "unsigned".to_owned(),
                signature: None,
                verified: false,
                verified_at: None,
            }),
        },
        committer: None,
        files: Vec::new(),
        html_url: urls.html,
        node_id: node_id("Commit", 0),
        parents: commit
            .parents
            .iter()
            .map(|parent| {
                let urls = repo.commit(parent);
                types::CommitParentsItem {
                    html_url: Some(urls.html),
                    sha: parent.clone(),
                    url: urls.api,
                }
            })
            .collect(),
        sha: commit.sha.clone(),
        stats: None,
        url: urls.api,
    }
}

/// Resolves `reference` in a repository that must have commits.
fn read_commit(located: &Located, reference: &str) -> Result<git::Commit, ApiResponse> {
    if git::is_empty(&located.git_dir).map_err(ApiResponse::internal)? {
        return Err(empty_repository());
    }
    let sha = git::resolve_commit(&located.git_dir, reference)
        .map_err(ApiResponse::internal)?
        .ok_or_else(|| {
            ApiResponse::unprocessable(&format!("No commit found for SHA: {reference}"))
        })?;
    git::commit(&located.git_dir, &sha).map_err(ApiResponse::internal)
}

impl Handler for ops::ReposGetCommit {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let located = locate(state, request)?;
        let commit = read_commit(&located, request.param("ref"))?;
        if let Some(response) = super::diff_media::commit(request, &located.git_dir, &commit.sha)? {
            return Ok(response);
        }
        Ok(ApiResponse::typed(
            200,
            &render_commit(&located.urls(request), &commit),
        ))
    }
}

impl Handler for ops::ReposListBranches {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let located = locate(state, request)?;
        let repo = located.urls(request);
        let items = git::refs(&located.git_dir, "refs/heads/")
            .map_err(ApiResponse::internal)?
            .into_iter()
            .map(|found| types::ShortBranch {
                commit: types::ShortBranchCommit {
                    url: repo.commit(&found.sha).api,
                    sha: found.sha,
                },
                name: found.name.trim_start_matches("refs/heads/").to_owned(),
                protected: false,
                protection: None,
                protection_url: None,
            })
            .collect();
        Ok(paginate(request, &request.path, items))
    }
}

impl Handler for ops::ReposGetBranch {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let located = locate(state, request)?;
        let branch = request.param("branch");
        let name = format!("refs/heads/{branch}");
        let found = git::refs(&located.git_dir, &name)
            .map_err(ApiResponse::internal)?
            .into_iter()
            .find(|found| found.name == name)
            .ok_or_else(|| ApiResponse::error(404, "Branch not found"))?;
        let commit = git::commit(&located.git_dir, &found.sha).map_err(ApiResponse::internal)?;
        let repo = located.urls(request);
        Ok(ApiResponse::typed(
            200,
            &types::BranchWithProtection {
                commit: render_commit(&repo, &commit),
                links: types::BranchWithProtectionLinks {
                    html: repo.tree_html(branch),
                    self_: repo.branch(branch),
                },
                name: branch.to_owned(),
                pattern: None,
                protected: false,
                protection: unprotected(),
                protection_url: repo.branch_protection(branch),
                required_approving_review_count: None,
            },
        ))
    }
}

/// The protection of a branch without rules.
fn unprotected() -> types::BranchProtection {
    types::BranchProtection {
        allow_deletions: None,
        allow_force_pushes: None,
        allow_fork_syncing: None,
        block_creations: None,
        enabled: Some(false),
        enforce_admins: None,
        lock_branch: None,
        name: None,
        protection_url: None,
        required_conversation_resolution: None,
        required_linear_history: None,
        required_pull_request_reviews: None,
        required_signatures: None,
        required_status_checks: None,
        restrictions: None,
        url: None,
    }
}

/// What a tree entry is, and the URLs every kind of content entry carries.
struct Entry<'a> {
    kind: types::ContentDirectoryItemType,
    name: &'a str,
    self_url: String,
    git_url: String,
    html_url: String,
    download_url: Option<String>,
}

fn entry<'a>(
    request: &ApiRequest,
    located: &Located,
    reference: &str,
    entry: &'a TreeEntry,
) -> Entry<'a> {
    use types::ContentDirectoryItemType as Kind;
    let repo = located.urls(request);
    let (kind, tree_or_blob) = match (entry.kind.as_str(), entry.mode.as_str()) {
        ("tree", _) => (Kind::Dir, "tree"),
        ("commit", _) => (Kind::Submodule, "tree"),
        (_, "120000") => (Kind::Symlink, "blob"),
        _ => (Kind::File, "blob"),
    };
    let download_url = (kind == Kind::File).then(|| repo.raw(reference, &entry.path));
    Entry {
        kind,
        name: entry.path.rsplit('/').next().unwrap_or(&entry.path),
        self_url: repo.contents(&entry.path, reference),
        git_url: repo.git_object(&format!("{tree_or_blob}s"), &entry.sha),
        html_url: repo.entry_html(tree_or_blob, reference, &entry.path),
        download_url,
    }
}

/// The `.gitmodules` URL of a gitlink, and the git and html URLs of the commit it pins when that
/// repository is hosted here.
struct Gitlink {
    url: String,
    git_url: Option<String>,
    html_url: Option<String>,
}

/// Describes the gitlink `tree_entry` from the `.gitmodules` entry whose `path` matches it at
/// `commit`.
fn gitlink(
    request: &ApiRequest,
    located: &Located,
    commit: &str,
    tree_entry: &TreeEntry,
) -> Result<Gitlink, ApiResponse> {
    let unconfigured =
        || ApiResponse::internal(format!("no .gitmodules entry has path {}", tree_entry.path));
    let Some(PathContent::File(file)) = git::path_content(&located.git_dir, commit, ".gitmodules")
        .map_err(ApiResponse::internal)?
    else {
        return Err(unconfigured());
    };
    let bytes = git::blob(&located.git_dir, &file.sha).map_err(ApiResponse::internal)?;
    let modules = gix::submodule::File::from_bytes(&bytes, None, &Default::default())
        .map_err(|error| ApiResponse::internal(format!("invalid .gitmodules: {error}")))?;
    let name = modules
        .name_by_path(tree_entry.path.as_str().into())
        .ok_or_else(unconfigured)?;
    let url = modules
        .url(name)
        .map_err(|error| ApiResponse::internal(format!("invalid .gitmodules: {error}")))?
        .to_bstring()
        .to_string();
    let hosted = url
        .strip_prefix(&request.urls().web("/"))
        .map(|rest| rest.strip_suffix(".git").unwrap_or(rest))
        .and_then(|rest| rest.split_once('/'))
        .filter(|(_, name)| !name.contains('/'))
        .map(|(owner, name)| request.urls().repo(owner, name));
    let sha = &tree_entry.sha;
    Ok(Gitlink {
        git_url: hosted.as_ref().map(|repo| repo.git_tree(sha)),
        html_url: hosted.as_ref().map(|repo| repo.tree_html(sha)),
        url,
    })
}

/// A directory listing item; GitHub lists a gitlink as a `file` without a download URL.
fn directory_item(
    request: &ApiRequest,
    located: &Located,
    reference: &str,
    commit: &str,
    tree_entry: &TreeEntry,
) -> Result<types::ContentDirectoryItem, ApiResponse> {
    let mut entry = entry(request, located, reference, tree_entry);
    let (git_url, html_url) = if tree_entry.kind == "commit" {
        let gitlink = gitlink(request, located, commit, tree_entry)?;
        entry.kind = types::ContentDirectoryItemType::File;
        (gitlink.git_url, gitlink.html_url)
    } else {
        (Some(entry.git_url), Some(entry.html_url))
    };
    Ok(types::ContentDirectoryItem {
        content: None,
        download_url: entry.download_url,
        git_url: git_url.clone(),
        html_url: html_url.clone(),
        links: types::ContentDirectoryItemLinks {
            git: git_url,
            html: html_url,
            self_: entry.self_url.clone(),
        },
        name: entry.name.to_owned(),
        path: tree_entry.path.clone(),
        sha: tree_entry.sha.clone(),
        size: tree_entry.size as i64,
        type_: entry.kind,
        url: entry.self_url,
    })
}

impl Handler for ops::ReposGetContent {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let located = locate(state, request)?;
        if git::is_empty(&located.git_dir).map_err(ApiResponse::internal)? {
            return Err(ApiResponse::error(404, "This repository is empty."));
        }
        let reference = request
            .query("ref")
            .unwrap_or(&located.default_branch)
            .to_owned();
        let sha = git::resolve_commit(&located.git_dir, &reference)
            .map_err(ApiResponse::internal)?
            .ok_or_else(|| {
                ApiResponse::error(404, &format!("No commit found for the ref {reference}"))
            })?;
        let content = git::path_content(&located.git_dir, &sha, request.param("path"))
            .map_err(ApiResponse::internal)?
            .ok_or_else(ApiResponse::not_found)?;
        match content {
            PathContent::Directory(entries) => Ok(ApiResponse::typed(
                200,
                &entries
                    .iter()
                    .map(|entry| directory_item(request, &located, &reference, &sha, entry))
                    .collect::<Result<Vec<_>, _>>()?,
            )),
            PathContent::File(tree_entry) if tree_entry.kind == "commit" => {
                let gitlink = gitlink(request, &located, &sha, &tree_entry)?;
                let entry = entry(request, &located, &reference, &tree_entry);
                Ok(ApiResponse::typed(
                    200,
                    &types::ContentSubmodule {
                        download_url: None,
                        git_url: gitlink.git_url.clone(),
                        html_url: gitlink.html_url.clone(),
                        links: types::ContentSubmoduleLinks {
                            git: gitlink.git_url,
                            html: gitlink.html_url,
                            self_: entry.self_url.clone(),
                        },
                        name: entry.name.to_owned(),
                        path: tree_entry.path.clone(),
                        sha: tree_entry.sha.clone(),
                        size: tree_entry.size as i64,
                        submodule_git_url: gitlink.url,
                        type_: types::ContentSubmoduleType::Submodule,
                        url: entry.self_url,
                    },
                ))
            }
            PathContent::File(tree_entry) => {
                let bytes =
                    git::blob(&located.git_dir, &tree_entry.sha).map_err(ApiResponse::internal)?;
                if request.accept().contains("application/vnd.github.raw") {
                    return Ok(ApiResponse::bytes(200, "application/octet-stream", bytes));
                }
                let entry = entry(request, &located, &reference, &tree_entry);
                if entry.kind == types::ContentDirectoryItemType::Symlink {
                    return Ok(ApiResponse::typed(
                        200,
                        &types::ContentSymlink {
                            download_url: entry.download_url,
                            git_url: Some(entry.git_url.clone()),
                            html_url: Some(entry.html_url.clone()),
                            links: types::ContentSymlinkLinks {
                                git: Some(entry.git_url),
                                html: Some(entry.html_url),
                                self_: entry.self_url.clone(),
                            },
                            name: entry.name.to_owned(),
                            path: tree_entry.path.clone(),
                            sha: tree_entry.sha.clone(),
                            size: tree_entry.size as i64,
                            target: String::from_utf8_lossy(&bytes).into_owned(),
                            type_: types::ContentSymlinkType::Symlink,
                            url: entry.self_url,
                        },
                    ));
                }
                Ok(ApiResponse::typed(
                    200,
                    &types::ContentFile {
                        content: wrapped_base64(&bytes),
                        download_url: entry.download_url,
                        encoding: "base64".to_owned(),
                        git_url: Some(entry.git_url.clone()),
                        html_url: Some(entry.html_url.clone()),
                        links: types::ContentFileLinks {
                            git: Some(entry.git_url),
                            html: Some(entry.html_url),
                            self_: entry.self_url.clone(),
                        },
                        name: entry.name.to_owned(),
                        path: tree_entry.path.clone(),
                        sha: tree_entry.sha.clone(),
                        size: tree_entry.size as i64,
                        submodule_git_url: None,
                        target: None,
                        type_: types::ContentFileType::File,
                        url: entry.self_url,
                    },
                ))
            }
        }
    }
}

/// A commit author or committer the request names, or the authenticated user.
struct Person {
    name: String,
    email: String,
    date: Option<String>,
}

impl Person {
    fn named(name: &str, email: &str, date: Option<&String>) -> Result<Self, ApiResponse> {
        // Source: <https://docs.github.com/en/rest/repos/contents#create-or-update-file-contents>
        if name.trim().is_empty() || email.trim().is_empty() {
            return Err(ApiResponse::validation_failed(
                "Commit",
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
            date: date.cloned(),
        })
    }

    /// The person as git records them, dated `now` unless the request gave a date.
    fn identity<'a>(&'a self, now: &'a str) -> crate::fixture_git::Identity<'a> {
        crate::fixture_git::Identity {
            name: &self.name,
            email: &self.email,
            date: self.date.as_deref().unwrap_or(now),
        }
    }
}

fn file_commit(
    request: &ApiRequest,
    located: &Located,
    branch: &str,
    commit: &git::Commit,
    file: &TreeEntry,
) -> types::FileCommit {
    let repo = located.urls(request);
    let urls = repo.commit(&commit.sha);
    let file_entry = entry(request, located, branch, file);
    let person = |signature: &git::Signature| types::FileCommitCommitAuthor {
        date: Some(crate::time::format(&render::timestamp(&signature.date))),
        email: Some(signature.email.clone()),
        name: Some(signature.name.clone()),
    };
    let committer = person(&commit.committer);
    types::FileCommit {
        content: Some(types::FileCommitContent {
            download_url: file_entry.download_url,
            git_url: Some(file_entry.git_url.clone()),
            html_url: Some(file_entry.html_url.clone()),
            links: Some(types::FileCommitContentLinks {
                git: Some(file_entry.git_url),
                html: Some(file_entry.html_url),
                self_: Some(file_entry.self_url.clone()),
            }),
            name: Some(file_entry.name.to_owned()),
            path: Some(file.path.clone()),
            sha: Some(file.sha.clone()),
            size: Some(file.size as i64),
            type_: Some("file".to_owned()),
            url: Some(file_entry.self_url),
        }),
        commit: types::FileCommitCommit {
            author: Some(person(&commit.author)),
            committer: Some(types::FileCommitCommitCommitter {
                date: committer.date,
                email: committer.email,
                name: committer.name,
            }),
            html_url: Some(urls.html),
            message: Some(commit.message.clone()),
            node_id: Some(node_id("Commit", 0)),
            parents: commit
                .parents
                .iter()
                .map(|parent| {
                    let urls = repo.commit(parent);
                    types::FileCommitCommitParentsItem {
                        html_url: Some(urls.html),
                        sha: Some(parent.clone()),
                        url: Some(urls.git),
                    }
                })
                .collect(),
            sha: Some(commit.sha.clone()),
            tree: Some(types::FileCommitCommitTree {
                sha: Some(commit.tree.clone()),
                url: Some(repo.git_tree(&commit.tree)),
            }),
            url: Some(urls.git),
            verification: Some(types::FileCommitCommitVerification {
                payload: None,
                reason: Some("unsigned".to_owned()),
                signature: None,
                verified: Some(false),
                verified_at: None,
            }),
        },
    }
}

impl Handler for ops::ReposCreateOrUpdateFileContents {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = request.require_scope(&["repo", "public_repo"])?;
        let body: types::ReposCreateOrUpdateFileContentsRequest = request.json_as()?;
        let bytes = STANDARD
            .decode(body.content.split_whitespace().collect::<String>())
            .map_err(|_| ApiResponse::unprocessable("content is not valid Base64"))?;
        // The lock is held until the commit lands so concurrent writes see each other's sha.
        let state = lock(state);
        let found = find_repo(&state, request)?;
        if !found.permission.push {
            return Err(ApiResponse::not_found());
        }
        let user = state
            .account(&caller.login)
            .expect("authenticated callers have accounts");
        let committer = match &body.committer {
            Some(committer) => {
                Person::named(&committer.name, &committer.email, committer.date.as_ref())?
            }
            // Default committer: the authenticated user.
            // Source: <https://docs.github.com/en/rest/repos/contents#create-or-update-file-contents>
            None => Person {
                name: user.name.clone().unwrap_or_else(|| user.login.clone()),
                email: format!("{}+{}@users.noreply.github.com", user.id, user.login),
                date: None,
            },
        };
        let author = match &body.author {
            Some(author) => Person::named(&author.name, &author.email, author.date.as_ref())?,
            // Default author: the committer.
            // Source: <https://docs.github.com/en/rest/repos/contents#create-or-update-file-contents>
            None => Person {
                name: committer.name.clone(),
                email: committer.email.clone(),
                date: committer.date.clone(),
            },
        };
        let repo = state
            .account(&found.owner)
            .and_then(|account| account.repo(&found.name))
            .expect("repository exists");
        let located = Located {
            git_dir: repo
                .git_dir
                .clone()
                .ok_or_else(|| ApiResponse::internal("repository has no git storage"))?,
            default_branch: repo.default_branch.clone(),
            found,
        };
        let branch = body.branch.as_deref().unwrap_or(&located.default_branch);
        let path = request.param("path").trim_matches('/');
        // PUT contents may initialize an empty repository.
        // Source: <https://docs.github.com/en/rest/guides/using-the-rest-api-to-interact-with-your-git-database#overview>
        let existing = if git::is_empty(&located.git_dir).map_err(ApiResponse::internal)? {
            None
        } else {
            let head = git::resolve_commit(&located.git_dir, &format!("refs/heads/{branch}"))
                .map_err(ApiResponse::internal)?
                .ok_or_else(|| ApiResponse::error(404, &format!("Branch {branch} not found")))?;
            match git::path_content(&located.git_dir, &head, path).map_err(ApiResponse::internal)? {
                Some(PathContent::File(file)) => Some(file.sha),
                Some(PathContent::Directory(_)) => {
                    return Err(ApiResponse::validation_failed("Content", "path", "invalid"));
                }
                None => None,
            }
        };
        match (&existing, body.sha.as_deref()) {
            (Some(_), None) => {
                return Err(ApiResponse::unprocessable(
                    "Invalid request.\n\n\"sha\" wasn't supplied.",
                ));
            }
            (Some(current), Some(sha)) if current != sha => {
                return Err(ApiResponse::error(
                    409,
                    &format!("{path} does not match {sha}"),
                ));
            }
            _ => {}
        }
        let now = crate::time::now();
        let branch_existed = git::resolve_commit(&located.git_dir, &format!("refs/heads/{branch}"))
            .map_err(ApiResponse::internal)?
            .is_some();
        let sha = crate::fixture_git::commit(crate::fixture_git::Commit {
            git_dir: &located.git_dir,
            branch,
            default_branch: branch,
            path: std::path::Path::new(path),
            bytes: &bytes,
            message: &body.message,
            author: &author.identity(&now),
            committer: &committer.identity(&now),
        })
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::InvalidInput {
                ApiResponse::validation_failed("Content", "path", "invalid")
            } else {
                ApiResponse::internal(error)
            }
        })?;
        let mut state = state;
        let actor = state
            .account(&caller.login)
            .map(|account| render::simple_user(request, account))
            .expect("authenticated callers have accounts");
        let moved = crate::handlers::timeline::branch_moved(
            &mut state,
            &located.found.owner,
            &located.found.name,
            branch,
            &sha,
            render::timestamp(&now),
            &actor,
        )
        .map_err(ApiResponse::internal)?;
        {
            let mut ctx = crate::webhooks::events::Ctx::new(
                &mut state,
                request,
                &located.found.owner,
                &located.found.name,
            );
            for item in &moved {
                crate::webhooks::events::pull_request::synchronize(
                    &mut ctx,
                    &item.pull,
                    &item.before,
                    &item.after,
                    &actor,
                )?;
            }
            if !branch_existed {
                crate::webhooks::events::create::reference(&mut ctx, branch, "branch")?;
            }
        }
        drop(state);
        let commit = git::commit(&located.git_dir, &sha).map_err(ApiResponse::internal)?;
        let Some(PathContent::File(file)) =
            git::path_content(&located.git_dir, &sha, path).map_err(ApiResponse::internal)?
        else {
            return Err(ApiResponse::internal("committed file could not be read"));
        };
        let status = if existing.is_some() { 200 } else { 201 };
        Ok(ApiResponse::typed(
            status,
            &types::ReposCreateOrUpdateFileContentsResponse(file_commit(
                request, &located, branch, &commit, &file,
            )),
        ))
    }
}

impl Handler for ops::ReposCompareCommits {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let located = locate(state, request)?;
        let (base, head) = request
            .param("basehead")
            .split_once("...")
            .ok_or_else(|| ApiResponse::unprocessable("base and head must be separated by ..."))?;
        let head_branch = head.split_once(':').map_or(head, |(_, branch)| branch);
        let base_sha = git::resolve_commit(&located.git_dir, base)
            .map_err(ApiResponse::internal)?
            .ok_or_else(|| ApiResponse::error(404, "base commit not found"))?;
        // `owner:branch` names a branch elsewhere in the repository network.
        // Source: <https://docs.github.com/en/rest/commits/commits#compare-two-commits>
        let head_sha = if let Some((owner, branch)) = head.split_once(':') {
            resolve_fork_network_branch(state, &located.found, &located.git_dir, owner, branch)?
                .or(git::resolve_commit(&located.git_dir, head_branch)
                    .map_err(ApiResponse::internal)?)
        } else {
            git::resolve_commit(&located.git_dir, head_branch).map_err(ApiResponse::internal)?
        }
        .ok_or_else(|| ApiResponse::error(404, "head commit not found"))?;
        if let Some(response) =
            super::diff_media::compare(request, &located.git_dir, &base_sha, &head_sha)?
        {
            return Ok(response);
        }
        let ahead = git::new_commits(&located.git_dir, &head_sha, &[&base_sha])
            .map_err(ApiResponse::internal)?;
        let behind = git::new_commits(&located.git_dir, &base_sha, &[&head_sha])
            .map_err(ApiResponse::internal)?;
        let status = if ahead.is_empty() && behind.is_empty() {
            types::CommitComparisonStatus::Identical
        } else if behind.is_empty() {
            types::CommitComparisonStatus::Ahead
        } else if ahead.is_empty() {
            types::CommitComparisonStatus::Behind
        } else {
            types::CommitComparisonStatus::Diverged
        };
        // Commits in chronological order, oldest first.
        // Source: <https://docs.github.com/en/rest/commits/commits#compare-two-commits>
        let commits = ahead
            .iter()
            .map(|sha| git::commit(&located.git_dir, sha))
            .collect::<Result<Vec<_>, _>>()
            .map_err(ApiResponse::internal)?;
        let base_commit =
            git::commit(&located.git_dir, &base_sha).map_err(ApiResponse::internal)?;
        let merge_base_commit =
            git::commit(&located.git_dir, &base_sha).map_err(ApiResponse::internal)?;
        let repo = located.urls(request);
        let compare = repo.compare(base, head);
        let owner = &located.found.owner;
        let response = types::CommitComparison {
            url: compare.api.clone(),
            html_url: compare.html.clone(),
            permalink_url: repo
                .compare(
                    &format!("{owner}:{base_sha}"),
                    &format!("{owner}:{head_sha}"),
                )
                .html,
            diff_url: compare.diff(),
            patch_url: compare.patch(),
            base_commit: render_commit(&repo, &base_commit),
            merge_base_commit: render_commit(&repo, &merge_base_commit),
            status,
            ahead_by: ahead.len() as i64,
            behind_by: behind.len() as i64,
            total_commits: ahead.len() as i64,
            commits: commits
                .iter()
                .map(|commit| render_commit(&repo, commit))
                .collect(),
            files: Vec::new(),
        };
        Ok(ApiResponse::typed(200, &response))
    }
}

impl Handler for ops::GitDeleteRef {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        request.require_scope(&["repo"])?;
        let located = locate(state, request)?;
        if !located.found.permission.push {
            return Err(ApiResponse::error(
                403,
                "Resource not accessible by personal access token",
            ));
        }
        let reference = format!("refs/{}", request.param("ref"));
        let check = std::process::Command::new("git")
            .args(["--git-dir"])
            .arg(&located.git_dir)
            .args(["check-ref-format", &reference])
            .output()
            .map_err(ApiResponse::internal)?;
        if !check.status.success() {
            return Err(ApiResponse::error(422, "invalid ref"));
        }
        let deleted = std::process::Command::new("git")
            .args(["--git-dir"])
            .arg(&located.git_dir)
            .args(["update-ref", "-d", &reference])
            .output()
            .map_err(ApiResponse::internal)?;
        if !deleted.status.success() {
            return Err(ApiResponse::not_found());
        }
        if let Some((short, kind)) = crate::webhooks::events::create::ref_kind(&reference) {
            let mut state = lock(state);
            let mut ctx = crate::webhooks::events::Ctx::new(
                &mut state,
                request,
                &located.found.owner,
                &located.found.name,
            );
            crate::webhooks::events::delete::reference(&mut ctx, short, kind)?;
        }
        Ok(ApiResponse::no_content())
    }
}

impl Handler for ops::GitUpdateRef {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = request.require_scope(&["repo", "public_repo"])?.clone();
        let body: types::GitUpdateRefRequest = request.json_as()?;
        let mutex = state;
        let mut state = lock(mutex);
        let found = find_repo(&state, request)?;
        if !found.permission.push {
            return Err(ApiResponse::error(
                403,
                "Resource not accessible by integration",
            ));
        }
        let repo = state
            .account(&found.owner)
            .and_then(|account| account.repo(&found.name))
            .expect("repository exists");
        let git_dir = repo
            .git_dir
            .clone()
            .ok_or_else(|| ApiResponse::internal("repository has no git storage"))?;
        let default_branch = repo.default_branch.clone();
        if git::is_empty(&git_dir).map_err(ApiResponse::internal)? {
            return Err(empty_repository());
        }
        let reference = format!("refs/{}", request.param("ref"));
        if gix::refs::FullName::try_from(reference.clone()).is_err() {
            return Err(ApiResponse::unprocessable("invalid ref"));
        }
        let current = git::refs(&git_dir, &reference)
            .map_err(ApiResponse::internal)?
            .into_iter()
            .find(|found| found.name == reference)
            .ok_or_else(|| ApiResponse::validation_failed("Reference", "ref", "invalid"))?;
        let new_sha = {
            let repo = super::git_store::open_repo(&git_dir)?;
            super::git_store::resolve_object(&repo, &body.sha)?
                .ok_or_else(|| ApiResponse::validation_failed("Reference", "sha", "invalid"))?
                .to_string()
        };
        let force = body.force.unwrap_or(false);
        if !force && !super::git_store::is_ancestor(&git_dir, &current.sha, &new_sha)? {
            // Source: cli/cli v2.101.0 pkg/cmd/repo/sync/sync.go:20
            return Err(ApiResponse::unprocessable("Update is not a fast forward"));
        }
        git::create_ref(&git_dir, &reference, &new_sha, &super::BOT)
            .map_err(ApiResponse::internal)?;
        let located = Located {
            found,
            git_dir: git_dir.clone(),
            default_branch,
        };
        if let Some(branch) = reference.strip_prefix("refs/heads/") {
            let actor = state
                .account(&caller.login)
                .map(|account| render::simple_user(request, account))
                .expect("authenticated callers have accounts");
            crate::handlers::timeline::branch_moved(
                &mut state,
                &located.found.owner,
                &located.found.name,
                branch,
                &new_sha,
                crate::time::now_utc(),
                &actor,
            )
            .map_err(ApiResponse::internal)?;
        }
        let updated = git::refs(&git_dir, &reference)
            .map_err(ApiResponse::internal)?
            .into_iter()
            .find(|found| found.name == reference)
            .ok_or_else(|| ApiResponse::internal("updated reference could not be read"))?;
        let response = ApiResponse::typed(
            200,
            &types::GitUpdateRefResponse(render_ref(request, &located, &updated)),
        );
        let owner = located.found.owner.clone();
        let name = located.found.name.clone();
        let old_sha = current.sha.clone();
        drop(state);
        crate::webhooks::events::push::enqueue_ref_push(
            mutex,
            request,
            (&owner, &name),
            &git_dir,
            &reference,
            &old_sha,
            &new_sha,
        );
        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_is_wrapped_like_github() {
        let wrapped = wrapped_base64(&[b'a'; 50]);
        let lines: Vec<&str> = wrapped.lines().collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].len(), 60);
        assert!(wrapped.ends_with('\n'));
    }
}
