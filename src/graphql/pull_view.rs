//! Builds the `PullRequest` object from the stored REST model and git.

use std::collections::HashMap;

use async_graphql::Error;

use super::Env;
use super::objects::{
    Actor, Commit, Connection, GitObject, HeadCommit, IssueCommentLink, Label, PullRequestCommit,
    Ref, RepositoryName, Status, TotalCount,
};
use super::pull_objects::{AutoMergeRequest, ChangedFile, GitActor};
use super::pulls::review_objects;
use super::query::{account_object, repository_object};
use super::review_rollup::review_decision;
use super::types::{
    MergeStateStatus, MergeableState, PatchStatus, PullRequestMergeMethod, PullRequestReviewState,
    PullRequestState, StatusState,
};
use crate::handlers::{pull_git, timeline};
use crate::state::{Account, Repo};
use crate::time;
use ninot_common::git;

/// The `PullRequest` object of a pull request with a REST model; `None` without one.
pub(super) fn pull_object(
    env: &Env,
    account: &Account,
    repo: &Repo,
    number: u64,
    pull: &crate::state::PullRequest,
) -> Result<Option<super::objects::PullRequest>, Error> {
    let Some(data) = pull.data.as_ref() else {
        return Ok(None);
    };
    let (reviews, opinions) = review_objects(env, account, repo, number, pull, &data.head.sha)?;
    let latest_at = latest_review_indices(&reviews);
    let (again, _) = review_objects(env, account, repo, number, pull, &data.head.sha)?;
    let mut again: Vec<Option<_>> = again.into_iter().map(Some).collect();
    let latest = take_indices(&mut again, &latest_at);
    let labels = data
        .labels
        .iter()
        .map(|label| Label {
            name: label.name.clone(),
            id: Some(label.node_id.clone()),
            color: Some(label.color.clone()),
            description: label.description.clone(),
            is_default: Some(label.default),
        })
        .collect();
    let assignees = data
        .assignees
        .iter()
        .map(|user| Actor::of_login(&user.login, &user.type_))
        .collect();
    let review_requests = data
        .requested_reviewers
        .iter()
        .map(|user| super::objects::ReviewRequest {
            requested_reviewer: Actor::of_login(&user.login, &user.type_),
        })
        .collect();
    let threads = pull
        .review_threads
        .iter()
        .map(|thread| super::objects::ReviewThread {
            typename: "PullRequestReviewThread",
            id: thread.id.clone(),
            is_resolved: thread.is_resolved,
            path: thread.path.clone(),
            line: thread.line,
            comments: Connection::complete(
                thread
                    .comments
                    .iter()
                    .map(|comment| super::objects::ReviewComment {
                        typename: "PullRequestReviewComment",
                        id: comment.id.clone(),
                        body: comment.body.clone(),
                        updated_at: comment.updated_at.clone(),
                    })
                    .collect(),
            ),
        })
        .collect();
    let urls = env.urls().repo(&account.login, &repo.name);
    let files = changed_files(repo, &urls, data)?;
    let (additions, deletions) = files.iter().fold((0, 0), |sum, file| {
        (sum.0 + file.additions, sum.1 + file.deletions)
    });
    let commits = pull_commits(env, account, repo, data)?;
    let (mergeable, merge_state_status) = merge_state(repo, data);
    // Source: <https://docs.github.com/en/graphql/reference/pulls#enum-pullrequeststate>
    let state = if data.merged {
        PullRequestState::Merged
    } else if data.state == crate::types::PullRequestState::Open {
        PullRequestState::Open
    } else {
        PullRequestState::Closed
    };
    let comments = pull
        .issue_comments
        .iter()
        .map(|comment| {
            let login = comment.user.as_ref().map(|user| user.login.as_str());
            IssueCommentLink {
                typename: "IssueComment",
                body: comment.body.clone(),
                id: comment.node_id.clone(),
                url: comment.html_url.clone(),
                author: comment
                    .user
                    .as_ref()
                    .map(|user| Actor::of_login(&user.login, &user.type_)),
                author_association: super::issue_view::association(account, login.unwrap_or("")),
                created_at: time::format(&comment.created_at),
                updated_at: time::format(&comment.updated_at),
                last_edited_at: (comment.updated_at != comment.created_at)
                    .then(|| time::format(&comment.updated_at)),
                includes_created_edit: false,
                is_minimized: false,
                minimized_reason: None,
                reaction_groups: Vec::new(),
                viewer_did_author: login.is_some_and(|login| login == env.caller.login),
            }
        })
        .collect();
    let sha = data.head.sha.as_str();
    let rollup = super::pull_status::status_rollup(account, repo, sha);
    Ok(Some(super::objects::PullRequest {
        typename: "PullRequest",
        // The stored REST `node_id` is this object's GraphQL id.
        id: data.node_id.clone(),
        database_id: super::pulls::pull_id(repo.id, number),
        full_database_id: super::pulls::pull_id(repo.id, number).to_string(),
        number,
        title: data.title.clone(),
        body: data.body.clone().unwrap_or_default(),
        is_draft: data.draft.unwrap_or(false),
        state,
        url: data.html_url.clone(),
        permalink: data.html_url.clone(),
        closed: data.state != crate::types::PullRequestState::Open,
        locked: data.locked,
        active_lock_reason: data
            .active_lock_reason
            .as_deref()
            .and_then(crate::state::LockReason::parse)
            .map(super::issue_view::gql_lock_reason),
        created_at: time::format(&data.created_at),
        updated_at: time::format(&data.updated_at),
        closed_at: data.closed_at.as_ref().map(time::format),
        merged_at: data.merged_at.as_ref().map(time::format),
        merged: data.merged,
        head_ref_name: data.head.ref_.clone(),
        base_ref_name: data.base.ref_.clone(),
        head_ref_oid: data.head.sha.clone(),
        base_ref_oid: data.base.sha.clone(),
        head_ref: branch_ref(env, account, repo, &data.head.ref_, &data.head.sha),
        base_ref: branch_ref(env, account, repo, &data.base.ref_, &data.base.sha),
        is_cross_repository: data.head.repo.id != data.base.repo.id,
        mergeable,
        merge_state_status,
        author: Actor::of_login(&data.user.login, &data.user.type_),
        author_association: super::issue_view::association(account, &data.user.login),
        repository: repository_object(env, account, repo),
        head_repository: repository_object(env, account, repo),
        head_repository_owner: account_object(env, account),
        additions,
        deletions,
        changed_files: i64::try_from(files.len()).unwrap_or(i64::MAX),
        maintainer_can_modify: data.maintainer_can_modify,
        is_in_merge_queue: false,
        is_merge_queue_enabled: false,
        auto_merge_request: pull.auto_merge_enabled_at.map(|at| auto_merge(data, at)),
        merged_by: data
            .merged_by
            .as_ref()
            .map(|user| Actor::of_login(&user.login, &user.type_)),
        merge_commit: data.merge_commit_sha.clone().map(|oid| Commit {
            typename: "Commit",
            oid,
            message_headline: None,
            message_body: None,
        }),
        potential_merge_commit: None,
        milestone: None,
        labels: Connection::complete(labels),
        assignees: Connection::complete(assignees),
        reviews: Connection::complete(reviews),
        latest_reviews: Connection::complete(latest),
        review_requests: Connection::complete(review_requests),
        review_threads: Connection::complete(threads),
        review_decision: super::read::seeded_review_decision(pull)
            .or_else(|| review_decision(&opinions)),
        comments: Connection::complete(comments),
        commits: Connection::complete(commits),
        files: Connection::complete(files),
        status_check_rollup: rollup,
        reaction_groups: Vec::new(),
        project_cards: Connection::complete(Vec::new()),
        project_items: Connection::complete(Vec::new()),
        closing_issues_references: Connection::complete(Vec::new()),
    }))
}

/// Auto-merge details stored on the REST pull request.
pub(super) fn auto_merge(
    data: &crate::types::PullRequest,
    enabled_at: chrono::DateTime<chrono::Utc>,
) -> AutoMergeRequest {
    let auto = data.auto_merge.as_ref();
    let merge_method = match auto.map(|auto| &auto.merge_method) {
        Some(crate::types::AutoMergeMergeMethod::Merge) => PullRequestMergeMethod::Merge,
        Some(crate::types::AutoMergeMergeMethod::Rebase) => PullRequestMergeMethod::Rebase,
        _ => PullRequestMergeMethod::Squash,
    };
    let text = |value: &str| (!value.is_empty()).then(|| value.to_owned());
    AutoMergeRequest {
        enabled_at,
        commit_headline: auto.and_then(|auto| text(&auto.commit_title)),
        commit_body: auto.and_then(|auto| text(&auto.commit_message)),
        merge_method,
        enabled_by: auto
            .map(|auto| Actor::of_login(&auto.enabled_by.login, &auto.enabled_by.type_)),
        author_email: None,
    }
}

fn changed_files(
    repo: &Repo,
    urls: &crate::urls::RepoUrls,
    data: &crate::types::PullRequest,
) -> Result<Vec<ChangedFile>, Error> {
    let files = timeline::pull_files(repo.git_dir.as_deref(), urls, data)
        .map_err(|error| Error::new(error.to_string()))?;
    Ok(files
        .into_iter()
        .map(|file| ChangedFile {
            path: file.filename,
            additions: file.additions,
            deletions: file.deletions,
            change_type: match file.status {
                crate::types::DiffEntryStatus::Added => PatchStatus::Added,
                crate::types::DiffEntryStatus::Removed => PatchStatus::Deleted,
                crate::types::DiffEntryStatus::Renamed => PatchStatus::Renamed,
                crate::types::DiffEntryStatus::Copied => PatchStatus::Copied,
                crate::types::DiffEntryStatus::Changed => PatchStatus::Changed,
                crate::types::DiffEntryStatus::Modified => PatchStatus::Modified,
                crate::types::DiffEntryStatus::Unchanged => PatchStatus::Modified,
            },
        })
        .collect())
}

fn pull_commits(
    _env: &Env,
    account: &Account,
    repo: &Repo,
    data: &crate::types::PullRequest,
) -> Result<Vec<PullRequestCommit>, Error> {
    let stored = timeline::pull_commits(repo.git_dir.as_deref(), data)
        .map_err(|error| Error::new(error.to_string()))?;
    if stored.is_empty() {
        // Report Git commit metadata when stored, or the seeded PR creation time otherwise.
        let (date, parents) = super::pulls::stored_commit(repo.git_dir.as_deref(), &data.head.sha)?
            .map_or_else(
                || (time::format(&data.created_at), 1),
                |commit| (commit.committer.date, commit.parents.len()),
            );
        return Ok(vec![commit_node(
            CommitText {
                oid: &data.head.sha,
                headline: "commit",
                body: "",
                authored: &date,
                committed: &date,
                name: "",
                email: "",
                parents,
            },
            account,
            repo,
            true,
        )]);
    }
    let last = stored.len() - 1;
    Ok(stored
        .iter()
        .enumerate()
        .map(|(index, commit)| {
            let (headline, body) = split_message(&commit.message);
            commit_node(
                CommitText {
                    oid: &commit.sha,
                    headline: &headline,
                    body: &body,
                    authored: &commit.author.date,
                    committed: &commit.committer.date,
                    name: &commit.author.name,
                    email: &commit.author.email,
                    parents: commit.parents.len(),
                },
                account,
                repo,
                index == last,
            )
        })
        .collect())
}

struct CommitText<'a> {
    oid: &'a str,
    headline: &'a str,
    body: &'a str,
    authored: &'a str,
    committed: &'a str,
    name: &'a str,
    email: &'a str,
    parents: usize,
}

fn commit_node(
    text: CommitText<'_>,
    account: &Account,
    repo: &Repo,
    with_status: bool,
) -> PullRequestCommit {
    let contexts = if with_status {
        super::read::status_contexts(repo, text.oid)
    } else {
        Vec::new()
    };
    PullRequestCommit {
        typename: "PullRequestCommit",
        commit: HeadCommit {
            typename: "Commit",
            oid: text.oid.to_owned(),
            message_headline: text.headline.to_owned(),
            message_body: text.body.to_owned(),
            authored_date: text.authored.to_owned(),
            committed_date: text.committed.to_owned(),
            authors: Connection::complete(vec![GitActor {
                typename: "GitActor",
                name: text.name.to_owned(),
                email: text.email.to_owned(),
                user: None,
            }]),
            parents: TotalCount {
                total_count: text.parents,
            },
            status: Status { contexts },
            status_check_rollup: with_status
                .then(|| super::pull_status::status_rollup(account, repo, text.oid))
                .flatten(),
        },
    }
}

fn merge_state(
    repo: &Repo,
    data: &crate::types::PullRequest,
) -> (MergeableState, MergeStateStatus) {
    // Source: <https://docs.github.com/en/graphql/reference/pulls#enum-mergeablestate>
    let stored = match data.mergeable {
        Some(true) => MergeableState::Mergeable,
        Some(false) => MergeableState::Conflicting,
        None => MergeableState::Unknown,
    };
    if data.mergeable == Some(false) {
        return (MergeableState::Conflicting, MergeStateStatus::Dirty);
    }
    if data.draft == Some(true) {
        return (stored, MergeStateStatus::Blocked);
    }
    let Some(git_dir) = repo.git_dir.as_deref() else {
        return (
            stored,
            if stored == MergeableState::Unknown {
                MergeStateStatus::Unknown
            } else {
                MergeStateStatus::Clean
            },
        );
    };
    let head = data.head.sha.as_str();
    let base = git::resolve_commit(git_dir, &format!("refs/heads/{}", data.base.ref_))
        .ok()
        .flatten()
        .unwrap_or_else(|| data.base.sha.clone());
    if pull_git::conflicts(git_dir, &base, head).unwrap_or(false) {
        return (MergeableState::Conflicting, MergeStateStatus::Dirty);
    }
    // A lagging head stays mergeable. GitHub reports BEHIND only when the base
    // branch requires an up-to-date head, and `gh pr merge` refuses that state.
    let failing = super::pull_status::rollup_nodes(repo, head)
        .state
        .is_some_and(|state| matches!(state, StatusState::Failure | StatusState::Error));
    let status = if failing {
        MergeStateStatus::Unstable
    } else if stored == MergeableState::Unknown {
        MergeStateStatus::Unknown
    } else {
        MergeStateStatus::Clean
    };
    (stored, status)
}

fn branch_ref(env: &Env, account: &Account, repo: &Repo, name: &str, oid: &str) -> Ref {
    Ref {
        typename: "Ref",
        name: name.to_owned(),
        prefix: "refs/heads/",
        target: Some(GitObject {
            typename: "Commit",
            oid: oid.to_owned(),
        }),
        repository: Some(RepositoryName {
            typename: "Repository",
            name_with_owner: format!("{}/{}", account.login, repo.name),
            url: Some(env.urls().repo(&account.login, &repo.name).html),
        }),
        branch_protection_rule: None,
    }
}

fn latest_review_indices(reviews: &[super::objects::Review]) -> Vec<usize> {
    let mut latest: HashMap<String, usize> = HashMap::new();
    for (index, review) in reviews.iter().enumerate() {
        if review.state == PullRequestReviewState::Pending {
            continue;
        }
        let login = review
            .author
            .as_ref()
            .map(|actor| actor.login.to_lowercase())
            .unwrap_or_default();
        latest.insert(login, index);
    }
    let mut indices: Vec<usize> = latest.into_values().collect();
    indices.sort_unstable();
    indices
}

fn take_indices<T>(items: &mut [Option<T>], indices: &[usize]) -> Vec<T> {
    indices
        .iter()
        .filter_map(|index| items.get_mut(*index).and_then(Option::take))
        .collect()
}

fn split_message(message: &str) -> (String, String) {
    match message.split_once('\n') {
        Some((headline, body)) => (
            headline.trim_end().to_owned(),
            body.trim_start_matches('\n').trim_end().to_owned(),
        ),
        None => (message.to_owned(), String::new()),
    }
}
