//! Pull request reviews and reviewer requests.

use std::sync::Mutex;

use super::super::Handler;
use super::super::repos::find_repo;
use super::caller_user;
use crate::api::{ApiRequest, ApiResponse, ApiResult};
use crate::operation::ops;
use crate::state::{NewTimelineEvent, State, TimelineEventKind, lock};
use crate::types;
use crate::webhooks::events::{self, Ctx};

impl Handler for ops::PullsCreateReview {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = request.require_scope(&["repo", "public_repo"])?;
        let body: types::PullsCreateReviewRequest = request.json_as()?;
        // A blank event creates a pending review; `body` is required for REQUEST_CHANGES and COMMENT.
        // Source: <https://docs.github.com/en/rest/pulls/reviews#create-a-review-for-a-pull-request>
        let event = body.event.map_or("PENDING", |event| match event {
            types::PullsCreateReviewRequestEvent::Approve => "APPROVE",
            types::PullsCreateReviewRequestEvent::RequestChanges => "REQUEST_CHANGES",
            types::PullsCreateReviewRequestEvent::Comment => "COMMENT",
        });
        if !matches!(event, "APPROVE" | "PENDING")
            && body.body.as_deref().unwrap_or_default().is_empty()
        {
            return Err(ApiResponse::unprocessable(
                "body is required for this review event",
            ));
        }

        let mut state = lock(state);
        let found = find_repo(&state, request)?;
        let author = state
            .account(&caller.login)
            .ok_or_else(ApiResponse::not_found)?;
        let author = crate::render::simple_user(request, author);
        let number = request
            .param("pull_number")
            .parse::<u64>()
            .map_err(|_| ApiResponse::not_found())?;
        let id = state.next_id();
        let comment_ids = body
            .comments
            .iter()
            .map(|_| state.next_id())
            .collect::<Vec<_>>();
        let repo = state
            .account_mut(&found.owner)
            .and_then(|account| account.repo_mut(&found.name))
            .ok_or_else(ApiResponse::not_found)?;
        let pull = repo
            .pull_requests
            .get_mut(&number)
            .ok_or_else(ApiResponse::not_found)?;
        let data = pull
            .data
            .as_mut()
            .ok_or_else(|| ApiResponse::internal("Pull request REST model is not seeded"))?;
        if matches!(event, "APPROVE" | "REQUEST_CHANGES")
            && data.user.login.eq_ignore_ascii_case(&caller.login)
        {
            return Err(ApiResponse::unprocessable(
                "A pull request author cannot approve or request changes on their own pull request",
            ));
        }
        let review_state = match event {
            "APPROVE" => "APPROVED",
            "REQUEST_CHANGES" => "CHANGES_REQUESTED",
            "COMMENT" => "COMMENTED",
            _ => "PENDING",
        };
        let submitted_at = (event != "PENDING").then(crate::state::now);
        if let Some(time) = &submitted_at {
            data.updated_at = crate::render::timestamp(time);
        }
        let urls = request.urls().repo(&found.owner, &found.name).pull(number);
        let pull_request_url = urls.api.clone();
        let html_url = urls.review_html(id);
        let items = comment_ids
            .iter()
            .copied()
            .zip(&body.comments)
            .collect::<Vec<_>>();
        let inline = super::super::pull_reviews::attach_comments(
            request,
            &found,
            number,
            &author,
            crate::render::id(id),
            &data.head.sha,
            &items,
        )?;
        let review = types::PullRequestReview {
            author_association: types::AuthorAssociation::Owner,
            body: body.body.unwrap_or_default(),
            body_html: None,
            body_text: None,
            commit_id: Some(body.commit_id.unwrap_or_else(|| data.head.sha.clone())),
            html_url: html_url.clone(),
            id: crate::render::id(id),
            links: types::PullRequestReviewLinks {
                html: types::PullRequestReviewLinksHtml { href: html_url },
                pull_request: types::PullRequestReviewLinksPullRequest {
                    href: pull_request_url.clone(),
                },
            },
            node_id: crate::render::node_id("PullRequestReview", id),
            pull_request_url,
            state: review_state.to_owned(),
            submitted_at: submitted_at.map(|time| crate::render::timestamp(&time)),
            user: Some(author.into()),
        };
        // Source: <https://docs.github.com/en/rest/pulls/review-requests#get-all-requested-reviewers-for-a-pull-request>
        if event != "PENDING" {
            data.requested_reviewers
                .retain(|reviewer| !reviewer.login.eq_ignore_ascii_case(&caller.login));
        }
        pull.reviews.push(review.clone());
        pull.review_comments.extend(inline.iter().cloned());
        if let Some(data) = &mut pull.data {
            data.review_comments = pull.review_comments.len() as i64;
        }
        let pull_data = pull.data.clone();
        if let Some(pull_data) = pull_data {
            let mut ctx = Ctx::new(&mut state, request, &found.owner, &found.name);
            events::pull_request_review::submitted(&mut ctx, &pull_data, &review)?;
            if event != "PENDING" {
                for comment in &inline {
                    events::pull_request_review_comment::created(&mut ctx, &pull_data, comment)?;
                }
            }
        }
        Ok(ApiResponse::typed(200, &review))
    }
}

impl Handler for ops::PullsRequestReviewers {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = request.require_scope(&["repo", "public_repo"])?;
        let body: types::PullsRequestReviewersRequest = request.json_as()?;
        let (reviewers, teams) = (body.reviewers, body.team_reviewers);
        if reviewers.is_empty() && teams.is_empty() {
            return Err(ApiResponse::unprocessable(
                "reviewers or team_reviewers is required",
            ));
        }
        let mut state = lock(state);
        let found = find_repo(&state, request)?;
        let number = request
            .param("pull_number")
            .parse::<u64>()
            .map_err(|_| ApiResponse::not_found())?;
        let mut users = Vec::new();
        for login in &reviewers {
            let account = state.account(login).ok_or_else(|| {
                ApiResponse::validation_failed("User", "reviewers", "does not exist")
            })?;
            users.push(crate::render::simple_user(request, account));
        }
        let owner = state
            .account(&found.owner)
            .ok_or_else(ApiResponse::not_found)?;
        let resolved_teams = teams
            .iter()
            .map(|slug| {
                owner
                    .team(slug)
                    .map(|team| crate::render::team_simple(request, owner, team))
                    .ok_or_else(|| {
                        ApiResponse::validation_failed("Team", "team_reviewers", "does not exist")
                    })
            })
            .collect::<Result<Vec<_>, ApiResponse>>()?;
        let actor = caller_user(&state, request, &caller.login)?;
        let requests = users
            .into_iter()
            .map(|user| (Some(user), None))
            .chain(resolved_teams.into_iter().map(|team| (None, Some(team))))
            .map(|(user, team)| (state.next_id(), user, team))
            .collect::<Vec<_>>();
        let now = crate::time::now_utc();
        let (data, added_users, added_teams) = {
            let repo = state
                .account_mut(&found.owner)
                .and_then(|account| account.repo_mut(&found.name))
                .ok_or_else(ApiResponse::not_found)?;
            let pull = repo
                .pull_requests
                .get_mut(&number)
                .ok_or_else(ApiResponse::not_found)?;
            let mut added_users = Vec::new();
            let mut added_teams = Vec::new();
            for (id, user, team) in requests {
                let data = pull.data.as_ref().ok_or_else(|| {
                    ApiResponse::internal("Pull request REST model is not seeded")
                })?;
                let requested = match (&user, &team) {
                    (Some(user), _) => data
                        .requested_reviewers
                        .iter()
                        .any(|existing| existing.login.eq_ignore_ascii_case(&user.login)),
                    (_, Some(team)) => data
                        .requested_teams
                        .iter()
                        .any(|existing| existing.slug == team.slug),
                    (None, None) => unreachable!("each request names a user or a team"),
                };
                if !requested {
                    if let Some(user) = &user {
                        added_users.push(user.clone());
                    }
                    if let Some(team) = &team {
                        added_teams.push(team.clone());
                    }
                    pull.apply_event(NewTimelineEvent {
                        id,
                        kind: TimelineEventKind::ReviewRequested,
                        actor: actor.clone(),
                        created_at: now,
                        commit_id: None,
                        requested_reviewer: user,
                        requested_team: team,
                    })
                    .map_err(|message| ApiResponse::unprocessable(&message))?;
                }
            }
            let data = pull
                .data
                .clone()
                .ok_or_else(|| ApiResponse::internal("Pull request REST model is not seeded"))?;
            (data, added_users, added_teams)
        };
        let mut ctx = Ctx::new(&mut state, request, &found.owner, &found.name);
        for user in &added_users {
            events::pull_request::review_requested_user(&mut ctx, &data, user)?;
        }
        for team in &added_teams {
            events::pull_request::review_requested_team(&mut ctx, &data, team)?;
        }
        Ok(ApiResponse::typed(
            201,
            &crate::render::pull_request_simple(&data),
        ))
    }
}

impl Handler for ops::PullsRemoveRequestedReviewers {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = request.require_scope(&["repo", "public_repo"])?;
        let body: types::PullsRemoveRequestedReviewersRequest = request.json_as()?;
        let number = request
            .param("pull_number")
            .parse::<u64>()
            .map_err(|_| ApiResponse::not_found())?;
        let mut state = lock(state);
        let found = find_repo(&state, request)?;
        if !found.permission.push {
            return Err(ApiResponse::error(
                403,
                "Resource not accessible by personal access token",
            ));
        }
        let actor = caller_user(&state, request, &caller.login)?;
        let ids = body
            .reviewers
            .iter()
            .chain(&body.team_reviewers)
            .map(|_| state.next_id())
            .collect::<Vec<_>>();
        let pull = state
            .account_mut(&found.owner)
            .and_then(|account| account.repo_mut(&found.name))
            .and_then(|repo| repo.pull_requests.get_mut(&number))
            .ok_or_else(ApiResponse::not_found)?;
        let data = pull
            .data
            .as_ref()
            .ok_or_else(|| ApiResponse::internal("Pull request REST model is not seeded"))?;
        let users = body
            .reviewers
            .iter()
            .filter_map(|login| {
                data.requested_reviewers
                    .iter()
                    .find(|user| user.login.eq_ignore_ascii_case(login))
                    .cloned()
            })
            .map(|user| (Some(user), None));
        let teams = body
            .team_reviewers
            .iter()
            .filter_map(|slug| {
                data.requested_teams
                    .iter()
                    .find(|team| team.slug == *slug)
                    .cloned()
            })
            .map(|team| (None, Some(team)));
        let removals = users.chain(teams).collect::<Vec<_>>();
        let now = crate::time::now_utc();
        for ((user, team), id) in removals.into_iter().zip(ids) {
            pull.apply_event(NewTimelineEvent {
                id,
                kind: TimelineEventKind::ReviewRequestRemoved,
                actor: actor.clone(),
                created_at: now,
                commit_id: None,
                requested_reviewer: user,
                requested_team: team,
            })
            .map_err(|message| ApiResponse::unprocessable(&message))?;
        }
        let data = pull.data.as_ref().expect("checked above");
        Ok(ApiResponse::typed(
            200,
            &crate::render::pull_request_simple(data),
        ))
    }
}
