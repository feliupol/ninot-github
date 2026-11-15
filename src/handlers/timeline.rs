//! Pull-request history: the issue timeline, the commits on a pull request and its files.
//!
//! The timeline is assembled on every read from the pull request's stored events, its reviews,
//! its issue comments and the commits git has between its base and head, so it always agrees
//! with the endpoints that list each of those.

use std::io;
use std::path::Path;
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use ninot_common::git;
use serde_json::Value;

use super::Handler;
use super::git_data::render_commit;
use super::repos::find_repo;
use crate::api::{ApiRequest, ApiResponse, ApiResult, paginate};
use crate::operation::ops;
use crate::render::{self, node_id};
use crate::state::{
    NewTimelineEvent, PullRequest, Repo, Sha, State, TimelineEvent, TimelineEventKind, lock,
};
use crate::types;
use crate::urls::RepoUrls;

/// The commits on a pull request, oldest first: those reachable from its head but not from its
/// base. Empty when the repository has no git storage or git does not know the head.
///
/// # Errors
///
/// Returns git failures reading the repository.
pub(crate) fn pull_commits(
    git_dir: Option<&Path>,
    data: &types::PullRequest,
) -> io::Result<Vec<git::Commit>> {
    let Some(git_dir) = git_dir else {
        return Ok(Vec::new());
    };
    let Some(head) = git::resolve_commit(git_dir, &data.head.sha)? else {
        return Ok(Vec::new());
    };
    let base = match git::resolve_commit(git_dir, &data.base.sha)? {
        Some(base) => Some(base),
        None => git::resolve_commit(git_dir, &format!("refs/heads/{}", data.base.ref_))?,
    };
    let excluded = base.iter().map(String::as_str).collect::<Vec<_>>();
    git::new_commits(git_dir, &head, &excluded)?
        .iter()
        .map(|sha| git::commit(git_dir, sha))
        .collect()
}

/// Moves an open pull request's head to `new_sha`, as a push to its head branch does.
///
/// Commits that reach the head for the first time are recorded as pushed at `at`. When the old
/// head is not an ancestor of the new one, a `head_ref_force_pushed` event with id `event_id`
/// is recorded. Closed pull requests and unchanged heads are left alone.
///
/// # Errors
///
/// Returns git failures reading the repository.
pub(crate) fn record_head_update(
    git_dir: &Path,
    pull: &mut PullRequest,
    new_sha: &str,
    at: DateTime<Utc>,
    actor: types::SimpleUser,
    event_id: u64,
) -> io::Result<()> {
    let Some(data) = pull.data.as_mut() else {
        return Ok(());
    };
    if data.state != types::PullRequestState::Open || data.head.sha == new_sha {
        return Ok(());
    }
    let old = git::resolve_commit(git_dir, &data.head.sha)?;
    let base = git::resolve_commit(git_dir, &data.base.sha)?;
    let excluded = old
        .iter()
        .chain(base.iter())
        .map(String::as_str)
        .collect::<Vec<_>>();
    for sha in git::new_commits(git_dir, new_sha, &excluded)? {
        let sha = Sha::parse(&sha).map_err(io::Error::other)?;
        pull.pushed_at.entry(sha).or_insert(at);
    }
    let forced = match &old {
        Some(old) => !git::new_commits(git_dir, old, &[new_sha])?.is_empty(),
        None => true,
    };
    if forced {
        pull.apply_event(NewTimelineEvent {
            id: event_id,
            kind: TimelineEventKind::HeadRefForcePushed,
            actor,
            created_at: at,
            commit_id: Some(new_sha.to_owned()),
            requested_reviewer: None,
            requested_team: None,
        })
        .map_err(io::Error::other)?;
    } else {
        let data = pull.data.as_mut().expect("checked above");
        data.statuses_url = data.statuses_url.replace(&data.head.sha, new_sha);
        data.links.statuses.href = data.links.statuses.href.replace(&data.head.sha, new_sha);
        data.head.sha = new_sha.to_owned();
        if data.updated_at < at {
            data.updated_at = at;
        }
    }
    Ok(())
}

/// An open pull request whose stored head SHA changed.
pub(crate) struct MovedPull {
    /// Head SHA before the move.
    pub before: String,
    /// Head SHA after the move.
    pub after: String,
    /// The pull request after the move.
    pub pull: types::PullRequest,
}

/// Moves every open pull request of `owner/name` whose head is `branch` of that same repository
/// to `new_sha`, as a push to the branch does; see [`record_head_update`].
///
/// Returns the pull requests whose head SHA changed.
///
/// # Errors
///
/// Returns git failures reading the repository; pull requests already moved stay moved.
pub(crate) fn branch_moved(
    state: &mut State,
    owner: &str,
    name: &str,
    branch: &str,
    new_sha: &str,
    at: DateTime<Utc>,
    actor: &types::SimpleUser,
) -> io::Result<Vec<MovedPull>> {
    let numbers = state
        .account(owner)
        .and_then(|account| account.repo(name))
        .map(|repo| {
            repo.pull_requests
                .iter()
                .filter(|(_, pull)| {
                    pull.data.as_ref().is_some_and(|data| {
                        data.head.ref_ == branch && data.head.user.login.eq_ignore_ascii_case(owner)
                    })
                })
                .map(|(number, _)| *number)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let mut moved = Vec::new();
    for number in numbers {
        let event_id = state.next_id();
        let Some(repo) = state
            .account_mut(owner)
            .and_then(|account| account.repo_mut(name))
        else {
            return Ok(moved);
        };
        let Some(git_dir) = repo.git_dir.clone() else {
            return Ok(moved);
        };
        if let Some(pull) = repo.pull_requests.get_mut(&number) {
            let before = pull.data.as_ref().map(|data| data.head.sha.clone());
            record_head_update(&git_dir, pull, new_sha, at, actor.clone(), event_id)?;
            if let (Some(before), Some(data)) = (before, pull.data.clone())
                && before != data.head.sha
            {
                moved.push(MovedPull {
                    before,
                    after: data.head.sha.clone(),
                    pull: data,
                });
            }
        }
    }
    Ok(moved)
}

/// Serializes a response model into one timeline entry.
fn entry(value: &impl serde::Serialize) -> Value {
    serde_json::to_value(value).expect("timeline events serialize to JSON")
}

/// A stored event as the timeline renders it, with its people as their accounts are now.
fn stored_event(people: &People, repo: &RepoUrls, event: &TimelineEvent) -> Value {
    let id = render::id(event.id);
    let url = repo.issue_event(event.id);
    let commit_url = event.commit_id.as_ref().map(|sha| repo.commit(sha).api);
    let kind = event.kind.name();
    let node = node_id(kind, event.id);
    let created_at = crate::time::format(&event.created_at);
    let actor = people.user(&event.actor);
    let requested_reviewer = event
        .requested_reviewer
        .as_deref()
        .map(|login| people.user(login));
    let requested_team = event
        .requested_team
        .and_then(|id| people.team(id))
        .map(|team| super::pulls::full_team(&team));
    match event.kind {
        TimelineEventKind::ReviewRequested => entry(&types::ReviewRequestedIssueEvent {
            actor: actor.clone(),
            commit_id: None,
            commit_url: None,
            created_at,
            event: kind.to_owned(),
            id,
            node_id: node,
            performed_via_github_app: None,
            requested_reviewer: requested_reviewer.clone(),
            requested_team: requested_team.clone(),
            review_requester: actor.clone(),
            url,
        }),
        TimelineEventKind::ReviewRequestRemoved => entry(&types::ReviewRequestRemovedIssueEvent {
            actor: actor.clone(),
            commit_id: None,
            commit_url: None,
            created_at,
            event: kind.to_owned(),
            id,
            node_id: node,
            performed_via_github_app: None,
            requested_reviewer: requested_reviewer.clone(),
            requested_team: requested_team.clone(),
            review_requester: actor.clone(),
            url,
        }),
        _ => {
            let mut row = entry(&types::StateChangeIssueEvent {
                actor,
                commit_id: event.commit_id.clone(),
                commit_url,
                created_at,
                event: kind.to_owned(),
                id,
                intent: None,
                node_id: node,
                performed_via_github_app: None,
                state_reason: None,
                url,
            });
            // GitHub writes these as null rather than leaving them out.
            let nulls: &[&str] = match event.kind {
                TimelineEventKind::Closed => &["state_reason", "intent"],
                TimelineEventKind::Reopened => &["state_reason"],
                _ => &[],
            };
            for field in nulls {
                row[*field] = Value::Null;
            }
            row
        }
    }
}

/// An issue comment as the timeline renders it; `None` for a comment without an author.
fn comment_event(comment: &types::IssueComment) -> Option<Value> {
    let user: types::SimpleUser = comment.user.clone()?.into();
    Some(entry(&types::TimelineCommentEvent {
        actor: user.clone(),
        author_association: comment
            .author_association
            .unwrap_or(types::AuthorAssociation::None),
        body: comment.body.clone(),
        body_html: None,
        body_text: None,
        created_at: comment.created_at,
        event: "commented".to_owned(),
        html_url: comment.html_url.clone(),
        id: comment.id,
        issue_url: comment.issue_url.clone(),
        minimized: None,
        node_id: comment.node_id.clone(),
        performed_via_github_app: None,
        pin: None,
        reactions: None,
        updated_at: comment.updated_at,
        url: comment.url.clone(),
        user,
    }))
}

/// A submitted review as the timeline renders it, with its state in lower case as the timeline
/// has it; `None` for a pending review or one without an author.
fn review_event(review: &types::PullRequestReview, head_sha: &str) -> Option<Value> {
    let submitted_at = review.submitted_at?;
    if review.state == "PENDING" {
        return None;
    }
    let user: types::SimpleUser = review.user.clone()?.into();
    Some(entry(&types::TimelineReviewedEvent {
        author_association: review.author_association,
        body: Some(review.body.clone()),
        body_html: None,
        body_text: None,
        commit_id: review
            .commit_id
            .clone()
            .unwrap_or_else(|| head_sha.to_owned()),
        event: "reviewed".to_owned(),
        html_url: review.html_url.clone(),
        id: review.id,
        links: types::TimelineReviewedEventLinks {
            html: types::TimelineReviewedEventLinksHtml {
                href: review.links.html.href.clone(),
            },
            pull_request: types::TimelineReviewedEventLinksPullRequest {
                href: review.links.pull_request.href.clone(),
            },
        },
        node_id: review.node_id.clone(),
        pull_request_url: review.pull_request_url.clone(),
        state: review.state.to_lowercase(),
        submitted_at: Some(submitted_at),
        updated_at: Some(submitted_at),
        user,
    }))
}

/// A commit on the pull request as the timeline renders it.
fn committed_event(repo: &RepoUrls, commit: &git::Commit) -> Value {
    let urls = repo.commit(&commit.sha);
    let person = |signature: &git::Signature| types::TimelineCommittedEventAuthor {
        date: render::timestamp(&signature.date),
        email: signature.email.clone(),
        name: signature.name.clone(),
    };
    let committer = person(&commit.committer);
    entry(&types::TimelineCommittedEvent {
        author: person(&commit.author),
        committer: types::TimelineCommittedEventCommitter {
            date: committer.date,
            email: committer.email,
            name: committer.name,
        },
        event: Some("committed".to_owned()),
        html_url: urls.html,
        message: commit.message.clone(),
        node_id: render_commit(repo, commit).node_id,
        parents: commit
            .parents
            .iter()
            .map(|parent| {
                let urls = repo.commit(parent);
                types::TimelineCommittedEventParentsItem {
                    html_url: urls.html,
                    sha: parent.clone(),
                    url: urls.git,
                }
            })
            .collect(),
        sha: commit.sha.clone(),
        tree: types::TimelineCommittedEventTree {
            sha: commit.tree.clone(),
            url: repo.git_tree(&commit.tree),
        },
        url: urls.git,
        verification: types::TimelineCommittedEventVerification {
            payload: None,
            reason: "unsigned".to_owned(),
            signature: None,
            verified: false,
            verified_at: None,
        },
    })
}

/// The timeline of pull request `pull` in `repo`, oldest first.
///
/// `committed` events carry no timestamp; they are placed when their commit first reached the
/// head branch, or at the pull request's creation for commits that were there from the start.
/// Renders the accounts and teams stored timeline events reference.
struct People<'a> {
    state: &'a State,
    request: &'a ApiRequest,
    /// Login of the organization whose teams are requested.
    owner: &'a str,
}

impl People<'_> {
    fn user(&self, login: &str) -> types::SimpleUser {
        render::referenced_user(self.state, self.request, login)
    }

    fn team(&self, id: u64) -> Option<types::TeamSimple> {
        render::referenced_team(self.state, self.request, self.owner, id)
    }
}

fn pull_timeline(
    people: &People,
    urls: &RepoUrls,
    repo: &Repo,
    pull: &PullRequest,
) -> io::Result<Vec<Value>> {
    let Some(data) = &pull.data else {
        return Ok(Vec::new());
    };
    // Ordered by time; within one second, commits first in git order, then everything else by
    // id, which the fake hands out in the order things happen.
    let mut rows: Vec<(DateTime<Utc>, i64, Value)> = Vec::new();
    for (index, commit) in pull_commits(repo.git_dir.as_deref(), data)?
        .iter()
        .enumerate()
    {
        let at = pull
            .pushed_at
            .get(commit.sha.as_str())
            .copied()
            .unwrap_or(data.created_at);
        let order = i64::try_from(index).unwrap_or(i64::MAX) - i64::MAX;
        rows.push((at, order, committed_event(urls, commit)));
    }
    rows.extend(pull.events.iter().map(|event| {
        (
            event.created_at,
            render::id(event.id),
            stored_event(people, urls, event),
        )
    }));
    rows.extend(
        pull.issue_comments
            .iter()
            .filter_map(|comment| Some((comment.created_at, comment.id, comment_event(comment)?))),
    );
    // A thread reply is stored as a review holding only that reply; the timeline omits it.
    let reply_only = |review_id: i64| {
        let mut comments = pull
            .review_comments
            .iter()
            .filter(|comment| comment.pull_request_review_id == Some(review_id))
            .peekable();
        comments.peek().is_some() && comments.all(|comment| comment.in_reply_to_id.is_some())
    };
    rows.extend(pull.reviews.iter().filter_map(|review| {
        if reply_only(review.id) {
            return None;
        }
        Some((
            review.submitted_at?,
            review.id,
            review_event(review, &data.head.sha)?,
        ))
    }));
    rows.sort_by_key(|(at, order, _)| (*at, *order));
    Ok(rows.into_iter().map(|(_, _, row)| row).collect())
}

/// Locates the pull request named by the `pull_number` path parameter.
fn with_pull<T>(
    state: &Mutex<State>,
    request: &ApiRequest,
    parameter: &str,
    read: impl FnOnce(&Repo, &PullRequest, &RepoUrls) -> Result<T, ApiResponse>,
) -> Result<T, ApiResponse> {
    let state = lock(state);
    let found = find_repo(&state, request)?;
    let repo = state
        .account(&found.owner)
        .and_then(|account| account.repo(&found.name))
        .expect("find_repo found the repository");
    let number = request
        .param(parameter)
        .parse::<u64>()
        .map_err(|_| ApiResponse::not_found())?;
    let pull = repo
        .pull_requests
        .get(&number)
        .ok_or_else(ApiResponse::not_found)?;
    read(repo, pull, &request.urls().repo(&found.owner, &found.name))
}

impl Handler for ops::IssuesListEventsForTimeline {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let rows = {
            let guard = lock(state);
            let found = find_repo(&guard, request)?;
            let repo = guard
                .account(&found.owner)
                .and_then(|account| account.repo(&found.name))
                .expect("find_repo found the repository");
            let number = request
                .param("issue_number")
                .parse::<u64>()
                .map_err(|_| ApiResponse::not_found())?;
            let urls = request.urls().repo(&found.owner, &found.name);
            if let Some(issue) = repo.issues.get(&number) {
                issue.comments.iter().filter_map(comment_event).collect()
            } else {
                let pull = repo
                    .pull_requests
                    .get(&number)
                    .ok_or_else(ApiResponse::not_found)?;
                let people = People {
                    state: &guard,
                    request,
                    owner: &found.owner,
                };
                pull_timeline(&people, &urls, repo, pull).map_err(ApiResponse::internal)?
            }
        };
        Ok(paginate(request, &request.path, rows))
    }
}

impl Handler for ops::PullsListCommits {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let commits = with_pull(state, request, "pull_number", |repo, pull, urls| {
            let data = pull
                .data
                .as_ref()
                .ok_or_else(|| ApiResponse::internal("Pull request REST model is not seeded"))?;
            Ok(pull_commits(repo.git_dir.as_deref(), data)
                .map_err(ApiResponse::internal)?
                .iter()
                .map(|commit| render_commit(urls, commit))
                .collect::<Vec<_>>())
        })?;
        Ok(paginate(request, &request.path, commits))
    }
}

/// The files a pull request changes, as `git diff base...head` lists them: the head tree against
/// the merge base of base and head. Empty when the repository has no git storage or git does not
/// know the head. Renames are listed as a removal and an addition, and `patch` is omitted.
///
/// # Errors
///
/// Returns git failures reading the repository.
pub(crate) fn pull_files(
    git_dir: Option<&Path>,
    urls: &RepoUrls,
    data: &types::PullRequest,
) -> io::Result<Vec<types::DiffEntry>> {
    let Some(git_dir) = git_dir else {
        return Ok(Vec::new());
    };
    let Some(head) = git::resolve_commit(git_dir, &data.head.sha)? else {
        return Ok(Vec::new());
    };
    let base = match git::resolve_commit(git_dir, &data.base.sha)? {
        Some(base) => Some(base),
        None => git::resolve_commit(git_dir, &format!("refs/heads/{}", data.base.ref_))?,
    };
    let repo = gix::open_opts(git_dir, gix::open::Options::isolated()).map_err(io::Error::other)?;
    let id = |sha: &str| gix::ObjectId::from_hex(sha.as_bytes()).map_err(io::Error::other);
    let head_id = id(&head)?;
    let tree_of = |commit: gix::ObjectId| -> io::Result<gix::Tree<'_>> {
        repo.find_commit(commit)
            .map_err(io::Error::other)?
            .tree()
            .map_err(io::Error::other)
    };
    let old_tree = match base {
        Some(base) => {
            let base = repo
                .merge_base(id(&base)?, head_id)
                .map_or(id(&base)?, |found| found.detach());
            tree_of(base)?
        }
        None => repo.empty_tree(),
    };
    let new_tree = tree_of(head_id)?;
    let mut options = gix::diff::Options::default().with_rewrites(None);
    options.track_path();
    let changes = repo
        .diff_tree_to_tree(&old_tree, &new_tree, options)
        .map_err(io::Error::other)?;
    let blob = |object: gix::ObjectId| -> io::Result<Vec<u8>> {
        Ok(repo
            .find_object(object)
            .map_err(io::Error::other)?
            .detach()
            .data)
    };
    let mut files = Vec::new();
    for change in changes {
        use gix::object::tree::diff::ChangeDetached as Change;
        if change.entry_mode().is_tree() {
            continue;
        }
        let (status, old, new) = match &change {
            Change::Addition { id, .. } => (types::DiffEntryStatus::Added, None, Some(*id)),
            Change::Deletion { id, .. } => (types::DiffEntryStatus::Removed, Some(*id), None),
            Change::Modification {
                previous_id, id, ..
            } => (
                types::DiffEntryStatus::Modified,
                Some(*previous_id),
                Some(*id),
            ),
            Change::Rewrite { source_id, id, .. } => (
                types::DiffEntryStatus::Modified,
                Some(*source_id),
                Some(*id),
            ),
        };
        let before = old.map(blob).transpose()?.unwrap_or_default();
        let after = new.map(blob).transpose()?.unwrap_or_default();
        let (additions, deletions) = line_changes(&before, &after);
        let filename = change.location().to_string();
        let sha = new.or(old).map(|object| object.to_string());
        files.push(types::DiffEntry {
            additions,
            blob_url: urls.blob_html(&head, &filename),
            changes: additions + deletions,
            contents_url: urls.contents(&filename, &head),
            deletions,
            patch: None,
            previous_filename: None,
            raw_url: urls.raw(&head, &filename),
            sha,
            status,
            filename,
        });
    }
    files.sort_by(|left, right| left.filename.cmp(&right.filename));
    Ok(files)
}

/// Lines added and removed between two blobs; none for binary content, as GitHub counts it.
fn line_changes(before: &[u8], after: &[u8]) -> (i64, i64) {
    use gix::diff::blob::{Algorithm, Diff, InternedInput, sources::byte_lines};
    if before.contains(&0) || after.contains(&0) {
        return (0, 0);
    }
    let input = InternedInput::new(byte_lines(before), byte_lines(after));
    let diff = Diff::compute(Algorithm::Histogram, &input);
    (
        i64::from(diff.count_additions()),
        i64::from(diff.count_removals()),
    )
}

impl Handler for ops::PullsListFiles {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let files = with_pull(state, request, "pull_number", |repo, pull, urls| {
            if let Some(files) = &pull.seeded_files {
                return Ok(files.clone());
            }
            let Some(data) = &pull.data else {
                return Ok(Vec::new());
            };
            pull_files(repo.git_dir.as_deref(), urls, data).map_err(ApiResponse::internal)
        })?;
        Ok(paginate(request, &request.path, files))
    }
}
