//! Pull-request connection fields, comparisons and merge-message text.

use async_graphql::Error;
use serde::Deserialize;

use super::objects::GitObject;
use super::page::page_window;
use super::pull_objects::{RollupContexts, StateCount, StatusCount};
use super::pull_view::pull_object;
use super::types::{ComparisonStatus, PullRequestMergeMethod};
use super::{Env, Map, Output, PageArgs, RepositoryKey, not_found, read};
use crate::render::node_id;
use crate::state::lock;
use ninot_common::git;

/// Pull-request field resolvers registered from this module.
pub(super) const RESOLVERS: &[(&str, super::Resolver)] = &[
    ("Repository.pullRequests", pull_requests),
    ("PullRequest.commits", page_commits),
    ("PullRequest.files", page_files),
    ("PullRequest.reviews", page_reviews),
    ("PullRequest.comments", page_comments),
    ("PullRequest.labels", page_labels),
    ("PullRequest.assignees", page_assignees),
    ("PullRequest.latestReviews", page_latest_reviews),
    ("PullRequest.viewerMergeHeadlineText", merge_headline),
    ("PullRequest.viewerMergeBodyText", merge_body),
    ("StatusCheckRollup.contexts", rollup_contexts),
    ("Ref.compare", compare_ref),
    ("StatusContext.isRequired", is_required),
    ("CheckRun.isRequired", is_required),
    ("User.id", user_id),
    ("User.databaseId", user_database_id),
];

#[derive(Deserialize)]
struct PullParent {
    number: u64,
    repository: RepositoryKey,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListArgs {
    states: Option<Vec<String>>,
    base_ref_name: Option<String>,
    head_ref_name: Option<String>,
    labels: Option<Vec<String>>,
    order_by: Option<OrderArgs>,
    first: Option<i64>,
    last: Option<i64>,
    after: Option<String>,
}

#[derive(Deserialize)]
struct OrderArgs {
    field: Option<String>,
    direction: Option<String>,
}

fn pull_requests(env: &Env, parent: &Map, args: &Map) -> Result<Output, Error> {
    let repository: RepositoryKey = read(parent)?;
    let args: ListArgs = read(args)?;
    let states = args
        .states
        .filter(|states| !states.is_empty())
        .unwrap_or_else(|| vec!["OPEN".to_owned()]);
    let state = lock(&env.state);
    let account = state
        .account(&repository.owner.login)
        .ok_or_else(|| not_found("repository does not exist".to_owned()))?;
    let repo = account
        .repo(&repository.name)
        .ok_or_else(|| not_found("repository does not exist".to_owned()))?;
    let mut rows = Vec::new();
    for (number, pull) in &repo.pull_requests {
        let Some(data) = pull.data.as_ref() else {
            continue;
        };
        let gql = if data.merged {
            "MERGED"
        } else if data.state == crate::types::PullRequestState::Open {
            "OPEN"
        } else {
            "CLOSED"
        };
        if !states.iter().any(|state| state.eq_ignore_ascii_case(gql)) {
            continue;
        }
        if args
            .base_ref_name
            .as_ref()
            .is_some_and(|base| !data.base.ref_.eq_ignore_ascii_case(base))
        {
            continue;
        }
        if args.head_ref_name.as_ref().is_some_and(|head| {
            !data.head.ref_.eq_ignore_ascii_case(head)
                && !data.head.label.eq_ignore_ascii_case(head)
        }) {
            continue;
        }
        if let Some(wanted) = &args.labels
            && !wanted.iter().all(|name| {
                data.labels
                    .iter()
                    .any(|label| label.name.eq_ignore_ascii_case(name))
            })
        {
            continue;
        }
        rows.push((*number, data.created_at, data.updated_at, data.comments));
    }
    let field = args
        .order_by
        .as_ref()
        .and_then(|order| order.field.as_deref())
        .unwrap_or("CREATED_AT");
    let descending = args
        .order_by
        .as_ref()
        .and_then(|order| order.direction.as_deref())
        .unwrap_or("DESC")
        .eq_ignore_ascii_case("DESC");
    rows.sort_by(|left, right| {
        let order = match field {
            "UPDATED_AT" => left.2.cmp(&right.2),
            "COMMENTS" => left.3.cmp(&right.3),
            _ => left.1.cmp(&right.1),
        };
        if descending { order.reverse() } else { order }
    });
    let mut nodes = Vec::new();
    for (number, _, _, _) in rows {
        let pull = repo.pull_requests.get(&number).expect("number was listed");
        if let Some(pull) = pull_object(env, account, repo, number, pull)? {
            nodes.push(pull);
        }
    }
    Output::of(page_window(nodes, args.first, args.last, args.after))
}

fn page_commits(env: &Env, parent: &Map, args: &Map) -> Result<Output, Error> {
    page_of(env, parent, args, |pull| pull.commits.nodes)
}

fn page_files(env: &Env, parent: &Map, args: &Map) -> Result<Output, Error> {
    page_of(env, parent, args, |pull| pull.files.nodes)
}

fn page_reviews(env: &Env, parent: &Map, args: &Map) -> Result<Output, Error> {
    page_of(env, parent, args, |pull| pull.reviews.nodes)
}

fn page_comments(env: &Env, parent: &Map, args: &Map) -> Result<Output, Error> {
    page_of(env, parent, args, |pull| pull.comments.nodes)
}

fn page_labels(env: &Env, parent: &Map, args: &Map) -> Result<Output, Error> {
    page_of(env, parent, args, |pull| pull.labels.nodes)
}

fn page_assignees(env: &Env, parent: &Map, args: &Map) -> Result<Output, Error> {
    page_of(env, parent, args, |pull| pull.assignees.nodes)
}

fn page_latest_reviews(env: &Env, parent: &Map, args: &Map) -> Result<Output, Error> {
    page_of(env, parent, args, |pull| pull.latest_reviews.nodes)
}

fn page_of<T: serde::Serialize>(
    env: &Env,
    parent: &Map,
    args: &Map,
    pick: impl FnOnce(super::objects::PullRequest) -> Vec<T>,
) -> Result<Output, Error> {
    let PageArgs { first, last, after } = read(args)?;
    Output::of(page_window(
        pick(owned_pull(env, parent)?),
        first,
        last,
        after,
    ))
}

/// The stored pull request for a parent that names its repository and number.
pub(super) fn owned_pull(env: &Env, parent: &Map) -> Result<super::objects::PullRequest, Error> {
    let PullParent { number, repository } = read(parent)?;
    let state = lock(&env.state);
    let account = state
        .account(&repository.owner.login)
        .ok_or_else(|| not_found("repository does not exist".to_owned()))?;
    let repo = account
        .repo(&repository.name)
        .ok_or_else(|| not_found("repository does not exist".to_owned()))?;
    let pull = repo
        .pull_requests
        .get(&number)
        .ok_or_else(|| not_found("pull request does not exist".to_owned()))?;
    pull_object(env, account, repo, number, pull)?
        .ok_or_else(|| not_found("pull request REST model does not exist".to_owned()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MergeTypeArgs {
    merge_type: Option<PullRequestMergeMethod>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MergeTextParent {
    number: u64,
    title: String,
    body: String,
    head_ref_name: String,
    repository: RepositoryKey,
}

fn merge_headline(env: &Env, parent: &Map, args: &Map) -> Result<Output, Error> {
    let _ = env;
    let parent: MergeTextParent = read(parent)?;
    let method = read::<MergeTypeArgs>(args)?.merge_type;
    let text = match method {
        Some(PullRequestMergeMethod::Squash | PullRequestMergeMethod::Rebase) => parent.title,
        _ => format!(
            "Merge pull request #{} from {}/{}",
            parent.number, parent.repository.owner.login, parent.head_ref_name
        ),
    };
    Output::of(text)
}

fn merge_body(_: &Env, parent: &Map, args: &Map) -> Result<Output, Error> {
    let parent: MergeTextParent = read(parent)?;
    let method = read::<MergeTypeArgs>(args)?.merge_type;
    let text = match method {
        Some(PullRequestMergeMethod::Rebase) => String::new(),
        Some(PullRequestMergeMethod::Squash) => parent.body,
        _ if parent.body.is_empty() => parent.title,
        _ => format!("{}\n\n{}", parent.title, parent.body),
    };
    Output::of(text)
}

#[derive(Deserialize)]
struct RollupParent {
    commit: OidParent,
    #[serde(rename = "_owner")]
    owner: String,
    #[serde(rename = "_name")]
    name: String,
}

#[derive(Deserialize)]
struct OidParent {
    oid: String,
}

fn rollup_contexts(env: &Env, parent: &Map, args: &Map) -> Result<Output, Error> {
    let parent: RollupParent = read(parent)?;
    let PageArgs { first, last, after } = read(args)?;
    let state = lock(&env.state);
    let repo = state
        .account(&parent.owner)
        .and_then(|account| account.repo(&parent.name))
        .ok_or_else(|| not_found("repository does not exist".to_owned()))?;
    let rollup = super::pull_status::rollup_nodes(repo, &parent.commit.oid);
    let paged = page_window(rollup.nodes, first, last, after);
    Output::of(RollupContexts {
        total_count: paged.total_count,
        nodes: paged.nodes,
        page_info: paged.page_info,
        check_run_count: rollup.check_counts.values().sum(),
        status_context_count: rollup.status_counts.values().sum(),
        check_run_counts_by_state: rollup
            .check_counts
            .into_iter()
            .map(|(state, count)| StateCount { count, state })
            .collect(),
        status_context_counts_by_state: rollup
            .status_counts
            .into_iter()
            .map(|(state, count)| StatusCount { count, state })
            .collect(),
    })
}

#[derive(Deserialize)]
struct RefParent {
    name: String,
    repository: Option<OwnerName>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OwnerName {
    name_with_owner: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CompareArgs {
    head_ref: String,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct Comparison {
    #[serde(rename = "__typename")]
    typename: &'static str,
    id: String,
    ahead_by: usize,
    behind_by: usize,
    status: ComparisonStatus,
    base_target: GitObject,
    head_target: GitObject,
}

fn compare_ref(env: &Env, parent: &Map, args: &Map) -> Result<Output, Error> {
    let parent: RefParent = read(parent)?;
    let CompareArgs { head_ref } = read(args)?;
    let name = parent
        .repository
        .ok_or_else(|| not_found("ref has no repository".to_owned()))?;
    let (owner, repo_name) = name
        .name_with_owner
        .split_once('/')
        .ok_or_else(|| not_found("ref has no repository".to_owned()))?;
    let state = lock(&env.state);
    let git_dir = state
        .account(owner)
        .and_then(|account| account.repo(repo_name))
        .and_then(|repo| repo.git_dir.clone())
        .ok_or_else(|| not_found("repository does not exist".to_owned()))?;
    let base =
        tip(&git_dir, &parent.name).ok_or_else(|| not_found("ref does not exist".to_owned()))?;
    let head =
        tip(&git_dir, &head_ref).ok_or_else(|| not_found("head ref does not exist".to_owned()))?;
    let ahead_by = git::new_commits(&git_dir, &head, &[&base])
        .map_err(|error| Error::new(error.to_string()))?
        .len();
    let behind_by = git::new_commits(&git_dir, &base, &[&head])
        .map_err(|error| Error::new(error.to_string()))?
        .len();
    let status = match (ahead_by, behind_by) {
        (0, 0) => ComparisonStatus::Identical,
        (_, 0) => ComparisonStatus::Ahead,
        (0, _) => ComparisonStatus::Behind,
        _ => ComparisonStatus::Diverged,
    };
    Output::of(Comparison {
        typename: "Comparison",
        id: node_id("Comparison", ahead_by as u64 + behind_by as u64),
        ahead_by,
        behind_by,
        status,
        base_target: GitObject {
            typename: "Commit",
            oid: base,
        },
        head_target: GitObject {
            typename: "Commit",
            oid: head,
        },
    })
}

fn tip(git_dir: &std::path::Path, name: &str) -> Option<String> {
    let bare = name.strip_prefix("refs/heads/").unwrap_or(name);
    git::resolve_commit(git_dir, &format!("refs/heads/{bare}"))
        .ok()
        .flatten()
        .or_else(|| git::resolve_commit(git_dir, name).ok().flatten())
}

fn is_required(_: &Env, _: &Map, _: &Map) -> Result<Output, Error> {
    Output::of(false)
}

fn user_id(env: &Env, parent: &Map, _: &Map) -> Result<Output, Error> {
    if let Some(id) = parent.get("id").and_then(|value| value.as_str())
        && !id.is_empty()
    {
        return Output::of(id.to_owned());
    }
    let login = parent
        .get("login")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    let state = lock(&env.state);
    let id = state.account(login).map(|account| account.id).unwrap_or(0);
    Output::of(node_id("User", id))
}

fn user_database_id(env: &Env, parent: &Map, _: &Map) -> Result<Output, Error> {
    if let Some(id) = parent.get("databaseId").and_then(|value| value.as_u64()) {
        return Output::of(id);
    }
    let login = parent
        .get("login")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    let state = lock(&env.state);
    Output::of(state.account(login).map(|account| account.id).unwrap_or(0))
}
