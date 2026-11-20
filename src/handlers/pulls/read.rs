//! Pull request and issue read routes that share this module.

use std::sync::Mutex;

use super::super::Handler;
use super::super::issues::{label_from_issue_item, label_from_pull};
use super::super::repos::find_repo;
use super::{full_team, pull_collection};
use crate::api::{ApiRequest, ApiResponse, ApiResult, paginate};
use crate::operation::ops;
use crate::state::{State, lock};
use crate::types;

impl Handler for ops::PullsGet {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let found = find_repo(&state, request)?;
        let repo = state
            .account(&found.owner)
            .and_then(|account| account.repo(&found.name))
            .expect("find_repo found the repository");
        let number = request
            .param("pull_number")
            .parse::<u64>()
            .map_err(|_| ApiResponse::not_found())?;
        let pull = repo
            .pull_requests
            .get(&number)
            .ok_or_else(ApiResponse::not_found)?;
        let data = pull
            .data
            .as_ref()
            .ok_or_else(|| ApiResponse::internal("Pull request REST model is not seeded"))?;
        if let Some(response) =
            super::super::diff_media::pull(request, repo.git_dir.as_deref(), data)?
        {
            return Ok(response);
        }
        let commits = super::super::timeline::pull_commits(repo.git_dir.as_deref(), data)
            .map_err(ApiResponse::internal)?;
        if commits.is_empty() {
            return Ok(ApiResponse::typed(200, data));
        }
        // The count agrees with `/pulls/{number}/commits` whenever git knows the head.
        let mut data = data.clone();
        data.commits = i64::try_from(commits.len()).expect("commit counts fit in i64");
        Ok(ApiResponse::typed(200, &data))
    }
}
impl Handler for ops::PullsList {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let found = find_repo(&state, request)?;
        let repo = state
            .account(&found.owner)
            .and_then(|account| account.repo(&found.name))
            .expect("find_repo found the repository");
        let state_filter = request.query("state").unwrap_or("open");
        let base = request.query("base");
        let head = request.query("head");
        let mut rows = repo
            .pull_requests
            .values()
            .filter_map(|pull| {
                let data = pull.data.as_ref()?;
                if state_filter != "all" && data.state.to_string() != state_filter {
                    return None;
                }
                if base.is_some_and(|value| data.base.ref_ != value) {
                    return None;
                }
                if let Some(value) = head {
                    let (owner, branch) = value
                        .split_once(':')
                        .unwrap_or((found.owner.as_str(), value));
                    if data.head.ref_ != branch || !data.head.user.login.eq_ignore_ascii_case(owner)
                    {
                        return None;
                    }
                }
                Some(data.clone())
            })
            .collect::<Vec<_>>();
        let sort = request.query("sort").unwrap_or("created");
        // Default direction, and `popularity` meaning comment count.
        // Source: <https://docs.github.com/en/rest/pulls/pulls#list-pull-requests>
        let ascending = match request.query("direction") {
            Some("asc") => true,
            Some("desc") => false,
            Some(_) => {
                return Err(ApiResponse::validation_failed(
                    "PullRequest",
                    "direction",
                    "invalid",
                ));
            }
            None => sort != "created",
        };
        match sort {
            "created" => rows.sort_by_key(|data| (data.created_at, data.number)),
            "updated" => rows.sort_by_key(|data| (data.updated_at, data.number)),
            "popularity" => rows.sort_by_key(|data| (data.comments, data.number)),
            "long-running" => {
                return Err(ApiResponse::error(
                    501,
                    "sort=long-running is not implemented by ninot-github",
                ));
            }
            _ => {
                return Err(ApiResponse::validation_failed(
                    "PullRequest",
                    "sort",
                    "invalid",
                ));
            }
        }
        if !ascending {
            rows.reverse();
        }
        let rows = rows
            .iter()
            .map(crate::render::pull_request_simple)
            .collect::<Vec<_>>();
        Ok(paginate(request, &request.path, rows))
    }
}
impl Handler for ops::IssuesListForRepo {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let found = find_repo(&state, request)?;
        let context = super::super::issues::context(&state, request, &found);
        let rows = super::super::issues::for_repository(context)?;
        Ok(paginate(request, &request.path, rows))
    }
}

impl Handler for ops::IssuesListLabelsForRepo {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let found = find_repo(&state, request)?;
        let labels = super::super::issues::repository_labels(&state, request, &found);
        Ok(paginate(request, &request.path, labels))
    }
}

impl Handler for ops::IssuesListLabelsOnIssue {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let found = find_repo(&state, request)?;
        let repo = state
            .account(&found.owner)
            .and_then(|account| account.repo(&found.name))
            .expect("find_repo found the repository");
        let number = request
            .param("issue_number")
            .parse::<u64>()
            .map_err(|_| ApiResponse::not_found())?;
        let labels = if let Some(pull) = repo.pull_requests.get(&number) {
            pull.data
                .as_ref()
                .ok_or_else(|| ApiResponse::internal("Pull request REST model is not seeded"))?
                .labels
                .iter()
                .map(label_from_pull)
                .collect::<Vec<_>>()
        } else if let Some(issue) =
            super::super::issues::rendered_issue(&state, request, &found, number)
        {
            issue
                .labels
                .iter()
                .filter_map(label_from_issue_item)
                .collect()
        } else {
            return Err(ApiResponse::not_found());
        };
        Ok(paginate(request, &request.path, labels))
    }
}

impl Handler for ops::IssuesGet {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let found = find_repo(&state, request)?;
        let number = request
            .param("issue_number")
            .parse::<u64>()
            .map_err(|_| ApiResponse::not_found())?;
        // A stored issue wins when a pull request was seeded with the same number.
        if let Some(issue) = super::super::issues::rendered_issue(&state, request, &found, number) {
            return Ok(ApiResponse::typed(200, &issue));
        }
        let pull = found
            .repo(&state)
            .pull_requests
            .get(&number)
            .ok_or_else(ApiResponse::not_found)?;
        let data = pull
            .data
            .as_ref()
            .ok_or_else(|| ApiResponse::internal("Pull request REST model is not seeded"))?;
        Ok(ApiResponse::typed(
            200,
            &crate::render::issue_from_pull(data),
        ))
    }
}

impl Handler for ops::PullsListReviews {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        pull_collection(state, request, "reviews", "pull_number")
    }
}

impl Handler for ops::PullsListRequestedReviewers {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let found = find_repo(&state, request)?;
        let repo = state
            .account(&found.owner)
            .and_then(|account| account.repo(&found.name))
            .expect("find_repo found the repository");
        let number = request
            .param("pull_number")
            .parse::<u64>()
            .map_err(|_| ApiResponse::not_found())?;
        let pull = repo
            .pull_requests
            .get(&number)
            .ok_or_else(ApiResponse::not_found)?;

        let data = pull
            .data
            .as_ref()
            .ok_or_else(|| ApiResponse::internal("Pull request REST model is not seeded"))?;
        let users = data
            .requested_reviewers
            .iter()
            .map(|reviewer| {
                state
                    .account(&reviewer.login)
                    .map(|account| crate::render::simple_user(request, account))
                    .unwrap_or_else(|| reviewer.clone())
            })
            .collect::<Vec<_>>();
        let teams = data
            .requested_teams
            .iter()
            .map(full_team)
            .collect::<Vec<_>>();

        Ok(ApiResponse::typed(
            200,
            &types::PullRequestReviewRequest { teams, users },
        ))
    }
}

impl Handler for ops::IssuesListComments {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        // GitHub uses an issue route for comments on pull requests. The state stores
        // the comment list alongside the matching pull request number.
        pull_collection(state, request, "issue_comments", "issue_number")
    }
}

impl Handler for ops::PullsListReviewComments {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        pull_collection(state, request, "review_comments", "pull_number")
    }
}
