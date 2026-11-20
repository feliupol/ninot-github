//! Pull requests and their review/comment read models.

mod open;
mod read;
mod review;
mod update;

use std::sync::Mutex;

use super::repos::find_repo;
use crate::api::{ApiRequest, ApiResponse, ApiResult, paginate};
use crate::state::{State, lock};
use crate::types;

/// The authenticated caller as events and comments embed them.
pub(super) fn caller_user(
    state: &State,
    request: &ApiRequest,
    login: &str,
) -> Result<types::SimpleUser, ApiResponse> {
    state
        .account(login)
        .map(|account| crate::render::simple_user(request, account))
        .ok_or_else(ApiResponse::not_found)
}

/// Expands a team reference into the full team model events embed.
pub(crate) fn full_team(team: &types::TeamSimple) -> types::Team {
    types::Team {
        access_source: None,
        description: team.description.clone(),
        enterprise_id: team.enterprise_id,
        html_url: team.html_url.clone(),
        id: team.id,
        members_url: team.members_url.clone(),
        name: team.name.clone(),
        node_id: team.node_id.clone(),
        notification_setting: team.notification_setting.clone(),
        organization_id: team.organization_id,
        parent: None,
        permission: team.permission.clone(),
        permissions: None,
        privacy: team.privacy.clone(),
        repositories_url: team.repositories_url.clone(),
        slug: team.slug.clone(),
        type_: match team.type_ {
            types::TeamSimpleType::Enterprise => types::TeamType::Enterprise,
            types::TeamSimpleType::Organization => types::TeamType::Organization,
        },
        url: team.url.clone(),
    }
}

fn pull_collection(
    state: &Mutex<State>,
    request: &ApiRequest,
    collection: &str,
    number_parameter: &str,
) -> ApiResult {
    let state = lock(state);
    let found = find_repo(&state, request)?;
    let repo = state
        .account(&found.owner)
        .and_then(|account| account.repo(&found.name))
        .expect("find_repo found the repository");
    let number = request
        .param(number_parameter)
        .parse::<u64>()
        .map_err(|_| ApiResponse::not_found())?;
    if collection == "issue_comments"
        && let Some(issue) = repo.issues.get(&number)
    {
        let context = super::issues::context(&state, request, &found);
        let comments = issue
            .comments
            .iter()
            .map(|comment| crate::render::issue_comment(context, number, comment))
            .collect::<Vec<_>>();
        return Ok(paginate(request, &request.path, comments));
    }
    let pull = repo
        .pull_requests
        .get(&number)
        .ok_or_else(ApiResponse::not_found)?;
    match collection {
        // Source: <https://docs.github.com/en/rest/pulls/reviews#list-reviews-for-a-pull-request>
        "reviews" => Ok(paginate(request, &request.path, pull.reviews.clone())),
        // Source: <https://docs.github.com/en/rest/issues/comments#list-issue-comments>
        "issue_comments" => Ok(paginate(
            request,
            &request.path,
            pull.issue_comments.clone(),
        )),
        // Source: <https://docs.github.com/en/rest/pulls/comments#list-review-comments-on-a-pull-request>
        "review_comments" => Ok(paginate(
            request,
            &request.path,
            pull.review_comments.clone(),
        )),
        _ => unreachable!("only known pull request collections are requested"),
    }
}
