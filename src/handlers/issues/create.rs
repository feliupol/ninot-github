//! `POST /repos/{owner}/{repo}/issues`.

use std::sync::Mutex;

use super::super::Handler;
use super::record::{self, Snapshot};
use crate::api::{ApiRequest, ApiResponse, ApiResult};
use crate::handlers::repos::find_repo;
use crate::operation::ops;
use crate::state::{Issue, IssueState, Name, State, lock};
use crate::types;
use crate::webhooks::events::{self, Ctx};

impl Handler for ops::IssuesCreate {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = request.require_scope(&["repo", "public_repo"])?;
        let body: types::IssuesCreateRequest = request.json_as()?;
        let title = body.title.to_string();
        if title.is_empty() {
            return Err(ApiResponse::unprocessable("title is required"));
        }
        let mut state = lock(state);
        let found = find_repo(&state, request)?;
        if !found.permission.push {
            return Err(ApiResponse::error(
                403,
                "Resource not accessible by personal access token",
            ));
        }
        let milestone = milestone_number(&state, &found, body.milestone.as_ref())?;
        let assignee_users = super::assignees::resolve_assignees(
            &state,
            request,
            &found.owner,
            &assignee_logins(&body),
        )?;
        let number = super::next_subject_number(found.repo(&state));
        let account = state
            .account(&caller.login)
            .ok_or_else(ApiResponse::not_found)?;
        let author = Name::new(account.login.clone());
        let user = crate::render::simple_user(request, account);
        let repo_urls = request.urls().repo(&found.owner, &found.name);
        let mut labels: Vec<types::Label> = Vec::new();
        for (index, label) in body.labels.iter().enumerate() {
            let name = match label {
                types::IssuesCreateRequestLabelsItem::String(name) => name.as_str(),
                types::IssuesCreateRequestLabelsItem::Object { name, .. } => {
                    name.as_deref().unwrap_or("label")
                }
            };
            let label =
                super::resolve_issue_label(&mut state, request, &found, name, || types::Label {
                    archived_at: None,
                    archived_by: None,
                    color: "ededed".to_owned(),
                    default: false,
                    description: Some(String::new()),
                    id: crate::render::id(
                        number.saturating_mul(100).saturating_add(index as u64 + 1),
                    ),
                    name: name.to_owned(),
                    node_id: format!("LA_{}_{}_{}_{index}", found.owner, found.name, number),
                    url: repo_urls.label(name),
                });
            if !labels
                .iter()
                .any(|existing| existing.name.eq_ignore_ascii_case(&label.name))
            {
                labels.push(label);
            }
        }
        let created = crate::time::now_utc();
        let milestone_title = milestone.and_then(|number| {
            found
                .repo(&state)
                .milestones
                .get(&number)
                .map(|milestone| milestone.title.clone())
        });
        state.add_issue(
            &found.owner,
            &found.name,
            Issue {
                number,
                node_id: format!("I_{}_{}_{}", found.owner, found.name, number),
                title,
                body: body.body,
                author,
                state: IssueState::Open,
                state_reason: None,
                closed_at: None,
                closed_by: None,
                labels: labels
                    .iter()
                    .map(|label| Name::new(label.name.clone()))
                    .collect(),
                assignees: assignee_users
                    .iter()
                    .map(|assignee| Name::new(assignee.login.clone()))
                    .collect(),
                milestone,
                locked: false,
                lock_reason: None,
                created_at: created,
                updated_at: created,
                comments: Vec::new(),
                events: Vec::new(),
                is_pinned: false,
                linked_branches: Vec::new(),
            },
        );
        let before = Snapshot {
            labels: Vec::new(),
            assignees: Vec::new(),
            milestone: None,
            state: "open".to_owned(),
            locked: false,
        };
        let after = Snapshot {
            labels: labels.clone(),
            assignees: assignee_users,
            milestone: milestone_title,
            state: "open".to_owned(),
            locked: false,
        };
        record::record_changes(&mut state, &found, number, &user, &before, &after);
        let issue = super::rendered_issue(&state, request, &found, number)
            .expect("the created issue is stored");
        let mut ctx = Ctx::new(&mut state, request, &found.owner, &found.name);
        events::issues::opened(&mut ctx, &issue)?;
        for label in &labels {
            events::issues::labeled(&mut ctx, &issue, label)?;
        }
        for assignee in &issue.assignees {
            events::issues::assigned(&mut ctx, &issue, assignee)?;
        }
        if let Some(milestone) = &issue.milestone {
            events::issues::milestoned(&mut ctx, &issue, milestone)?;
        }
        Ok(ApiResponse::typed(201, &issue))
    }
}

/// `assignee` plus `assignees`, with the singular login first when it is not already listed.
fn assignee_logins(body: &types::IssuesCreateRequest) -> Vec<String> {
    let mut logins = body.assignees.clone();
    if let Some(Some(login)) = &body.assignee
        && !logins
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(login))
    {
        logins.insert(0, login.clone());
    }
    logins
}

/// Milestone number from the create body. Absent and JSON null both mean no milestone.
fn milestone_number(
    state: &State,
    found: &crate::handlers::repos::Found,
    milestone: Option<&Option<types::IssuesCreateRequestMilestone>>,
) -> Result<Option<u64>, ApiResponse> {
    let Some(Some(value)) = milestone else {
        return Ok(None);
    };
    let number = match value {
        types::IssuesCreateRequestMilestone::Integer(number) => u64::try_from(*number).ok(),
        types::IssuesCreateRequestMilestone::String(text) => text.trim().parse().ok(),
    };
    let Some(number) = number else {
        return Err(ApiResponse::validation_failed(
            "Issue",
            "milestone",
            "invalid",
        ));
    };
    if !found.repo(state).milestones.contains_key(&number) {
        return Err(ApiResponse::validation_failed(
            "Issue",
            "milestone",
            "invalid",
        ));
    }
    Ok(Some(number))
}
