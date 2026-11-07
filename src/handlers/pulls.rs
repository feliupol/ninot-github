//! Pull requests and their review/comment read models.

use std::sync::Mutex;

use super::Handler;
use super::repos::find_repo;
use crate::api::{ApiRequest, ApiResponse, ApiResult, paginate};
use crate::operation::ops;
use crate::state::{State, TimelineEvent, TimelineEventKind, lock};
use crate::types;
use crate::webhooks::events::{self, Ctx};

/// The authenticated caller as events and comments embed them.
fn caller_user(
    state: &State,
    request: &ApiRequest,
    login: &str,
) -> Result<types::SimpleUser, ApiResponse> {
    state
        .account(login)
        .map(|account| crate::render::simple_user(request, account))
        .ok_or_else(ApiResponse::not_found)
}

fn issue_label(label: &types::IssueLabelsItem) -> Option<types::Label> {
    match label {
        types::IssueLabelsItem::String(name) => Some(types::Label {
            archived_at: None,
            archived_by: None,
            color: "ededed".to_owned(),
            default: false,
            description: Some(String::new()),
            id: 0,
            name: name.clone(),
            node_id: String::new(),
            url: String::new(),
        }),
        types::IssueLabelsItem::Object {
            archived_by,
            color,
            default,
            description,
            id,
            name,
            node_id,
            url,
        } => Some(types::Label {
            archived_at: None,
            archived_by: archived_by.clone(),
            color: color.clone().unwrap_or_else(|| "ededed".to_owned()),
            default: default.unwrap_or(false),
            description: description.clone(),
            id: id.unwrap_or_default(),
            name: name.clone().unwrap_or_default(),
            node_id: node_id.clone().unwrap_or_default(),
            url: url.clone().unwrap_or_default(),
        }),
    }
}

fn pull_label(label: &types::PullRequestLabelsItem) -> types::Label {
    types::Label {
        archived_at: None,
        archived_by: label.archived_by.clone(),
        color: label.color.clone(),
        default: label.default,
        description: label.description.clone(),
        id: label.id,
        name: label.name.clone(),
        node_id: label.node_id.clone(),
        url: label.url.clone(),
    }
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
        return Ok(paginate(request, &request.path, issue.comments.clone()));
    }
    let pull = repo
        .pull_requests
        .get(&number)
        .ok_or_else(ApiResponse::not_found)?;
    match collection {
        "reviews" => Ok(paginate(request, &request.path, pull.reviews.clone())),
        "issue_comments" => Ok(paginate(
            request,
            &request.path,
            pull.issue_comments.clone(),
        )),
        "review_comments" => Ok(paginate(
            request,
            &request.path,
            pull.review_comments.clone(),
        )),
        _ => unreachable!("only known pull request collections are requested"),
    }
}

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
            .ok_or_else(|| ApiResponse::error(500, "Pull request REST model is not seeded"))?;
        let commits = super::timeline::pull_commits(repo.git_dir.as_deref(), data)
            .map_err(|error| ApiResponse::error(500, &error.to_string()))?;
        if commits.is_empty() {
            return Ok(ApiResponse::typed(200, data));
        }
        // The count agrees with `/pulls/{number}/commits` whenever git knows the head.
        let mut data = data.clone();
        data.commits = i64::try_from(commits.len()).expect("commit counts fit in i64");
        Ok(ApiResponse::typed(200, &data))
    }
}

/// Updates metadata and open/closed state without changing the GraphQL-owned draft state.
impl Handler for ops::PullsUpdate {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = request.require_scope(&["repo", "public_repo"])?;
        let body: types::PullsUpdateRequest = request.json_as()?;
        if body
            .title
            .as_ref()
            .is_some_and(|title| title.trim().is_empty())
        {
            return Err(ApiResponse::validation_failed(
                "PullRequest",
                "title",
                "missing_field",
            ));
        }
        let number = request
            .param("pull_number")
            .parse::<u64>()
            .map_err(|_| ApiResponse::not_found())?;
        let mut state = lock(state);
        let found = find_repo(&state, request)?;
        let repo = state
            .account(&found.owner)
            .and_then(|account| account.repo(&found.name))
            .ok_or_else(ApiResponse::not_found)?;
        let data = repo
            .pull_requests
            .get(&number)
            .and_then(|pull| pull.data.as_ref())
            .ok_or_else(ApiResponse::not_found)?;
        if !found.permission.push && !data.user.login.eq_ignore_ascii_case(&caller.login) {
            return Err(ApiResponse::error(
                403,
                "Must have write access or be the pull request author",
            ));
        }
        if data.merged && matches!(body.state, Some(types::PullsUpdateRequestState::Open)) {
            return Err(ApiResponse::validation_failed(
                "PullRequest",
                "state",
                "invalid",
            ));
        }
        let old_title = data.title.clone();
        let old_body = data.body.clone();
        let old_base_ref = data.base.ref_.clone();
        let old_base_sha = data.base.sha.clone();
        let base_sha = body
            .base
            .as_ref()
            .map(|branch| {
                let git_dir = repo
                    .git_dir
                    .as_ref()
                    .ok_or_else(|| ApiResponse::error(500, "repository has no git storage"))?;
                ninot_common::git::resolve_commit(git_dir, &format!("refs/heads/{branch}"))
                    .map_err(|error| ApiResponse::error(500, &error.to_string()))?
                    .ok_or_else(|| ApiResponse::validation_failed("PullRequest", "base", "invalid"))
            })
            .transpose()?;
        let transition = match (&body.state, &data.state) {
            (Some(types::PullsUpdateRequestState::Open), types::PullRequestState::Closed) => {
                Some(TimelineEventKind::Reopened)
            }
            (Some(types::PullsUpdateRequestState::Closed), types::PullRequestState::Open) => {
                Some(TimelineEventKind::Closed)
            }
            _ => None,
        };
        let actor = caller_user(&state, request, &caller.login)?;
        let event_id = state.next_id();
        let pull = state
            .account_mut(&found.owner)
            .and_then(|account| account.repo_mut(&found.name))
            .and_then(|repo| repo.pull_requests.get_mut(&number))
            .ok_or_else(ApiResponse::not_found)?;
        let data = pull.data.as_mut().ok_or_else(ApiResponse::not_found)?;
        if let Some(title) = body.title {
            data.title = title;
        }
        if let Some(body) = body.body {
            data.body = Some(body);
        }
        if let Some(branch) = body.base {
            data.base.label = format!("{}:{branch}", found.owner);
            data.base.ref_ = branch;
            if let Some(sha) = base_sha {
                data.base.sha = sha;
            }
        }
        if let Some(allowed) = body.maintainer_can_modify {
            data.maintainer_can_modify = allowed;
        }
        let now = crate::render::timestamp(&crate::state::now());
        data.updated_at = now;
        if let Some(kind) = transition {
            pull.apply_event(TimelineEvent {
                id: event_id,
                kind,
                actor,
                created_at: now,
                commit_id: None,
                requested_reviewer: None,
                requested_team: None,
            })
            .map_err(|message| ApiResponse::error(422, &message))?;
        }
        let data = pull.data.clone().expect("checked above");
        let mut changes = serde_json::Map::new();
        if data.title != old_title {
            changes.insert("title".to_owned(), serde_json::json!({ "from": old_title }));
        }
        if data.body != old_body {
            changes.insert(
                "body".to_owned(),
                serde_json::json!({ "from": old_body.unwrap_or_default() }),
            );
        }
        if data.base.ref_ != old_base_ref || data.base.sha != old_base_sha {
            changes.insert(
                "base".to_owned(),
                serde_json::json!({
                    "ref": { "from": old_base_ref },
                    "sha": { "from": old_base_sha },
                }),
            );
        }
        let mut ctx = Ctx::new(&mut state, request, &found.owner, &found.name);
        match transition {
            Some(TimelineEventKind::Closed) => events::pull_request::closed(&mut ctx, &data),
            Some(TimelineEventKind::Reopened) => events::pull_request::reopened(&mut ctx, &data),
            _ => {}
        }
        events::pull_request::edited(&mut ctx, &data, changes);
        Ok(ApiResponse::typed(200, &types::PullsUpdateResponse(data)))
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

impl Handler for ops::IssuesCreate {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = request.require_scope(&["repo", "public_repo"])?;
        let body: types::IssuesCreateRequest = request.json_as()?;
        let title = body.title.to_string();
        if title.is_empty() {
            return Err(ApiResponse::error(422, "title is required"));
        }
        let mut state = lock(state);
        let found = find_repo(&state, request)?;
        if !found.permission.push {
            return Err(ApiResponse::error(
                403,
                "Resource not accessible by personal access token",
            ));
        }
        let number = state
            .account(&found.owner)
            .and_then(|account| account.repo(&found.name))
            .and_then(|repo| {
                repo.issues
                    .keys()
                    .chain(repo.pull_requests.keys())
                    .max()
                    .copied()
            })
            .unwrap_or(0)
            + 1;
        let user = state
            .account(&caller.login)
            .ok_or_else(ApiResponse::not_found)?;
        let user = crate::render::simple_user(request, user);
        let api = format!("{}/repos/{}/{}", request.api_url, found.owner, found.name);
        let web = format!("{}/{}/{}", request.web_url, found.owner, found.name);
        let created = crate::state::now();
        let labels = body
            .labels
            .iter()
            .enumerate()
            .map(|(index, label)| {
                let name = match label {
                    types::IssuesCreateRequestLabelsItem::String(name) => name.as_str(),
                    types::IssuesCreateRequestLabelsItem::Object { name, .. } => {
                        name.as_deref().unwrap_or("label")
                    }
                };
                types::IssueLabelsItem::Object {
                    archived_by: None,
                    color: Some("ededed".to_owned()),
                    default: Some(false),
                    description: Some(String::new()),
                    id: Some(crate::render::id(
                        number.saturating_mul(100).saturating_add(index as u64 + 1),
                    )),
                    name: Some(name.to_owned()),
                    node_id: Some(format!(
                        "LA_{}_{}_{}_{index}",
                        found.owner, found.name, number
                    )),
                    url: Some(format!("{api}/labels/{name}")),
                }
            })
            .collect::<Vec<_>>();
        let issue = types::Issue {
            active_lock_reason: None,
            assignee: None,
            assignees: Vec::new(),
            author_association: Some(types::AuthorAssociation::Owner),
            body: body.body,
            body_html: None,
            body_text: None,
            closed_at: None,
            closed_by: None,
            comments: 0,
            comments_url: format!("{api}/issues/{number}/comments"),
            created_at: crate::render::timestamp(&created),
            draft: None,
            events_url: format!("{api}/issues/{number}/events"),
            html_url: format!("{web}/issues/{number}"),
            id: crate::render::id(number),
            issue_dependencies_summary: None,
            issue_field_values: Vec::new(),
            labels,
            labels_url: format!("{api}/issues/{number}/labels{{/name}}"),
            locked: false,
            milestone: None,
            node_id: format!("I_{}_{}_{}", found.owner, found.name, number),
            number: crate::render::id(number),
            parent_issue_url: None,
            performed_via_github_app: None,
            pinned_comment: None,
            pull_request: None,
            reactions: None,
            repository: None,
            repository_url: api.clone(),
            state: "open".to_owned(),
            state_reason: None,
            sub_issues_summary: None,
            timeline_url: None,
            title,
            type_: None,
            updated_at: crate::render::timestamp(&created),
            url: format!("{api}/issues/{number}"),
            user: Some(user.clone().into()),
        };
        state.add_issue(&found.owner, &found.name, number, issue.clone());
        for label in issue.labels.iter().filter_map(issue_label) {
            super::issues::record::record_labeled(&mut state, &found, number, user.clone(), &label);
        }
        let mut ctx = Ctx::new(&mut state, request, &found.owner, &found.name);
        events::issues::opened(&mut ctx, &issue);
        for label in issue.labels.iter().filter_map(issue_label) {
            events::issues::labeled(&mut ctx, &issue, &label);
        }
        Ok(ApiResponse::typed(201, &issue))
    }
}

impl Handler for ops::IssuesListForRepo {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let found = find_repo(&state, request)?;
        let repo = state
            .account(&found.owner)
            .and_then(|account| account.repo(&found.name))
            .expect("repository exists");
        let rows = repo
            .issues
            .values()
            .map(|issue| issue.data.clone())
            .collect::<Vec<_>>();
        Ok(paginate(request, &request.path, rows))
    }
}

impl Handler for ops::IssuesListLabelsForRepo {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let found = find_repo(&state, request)?;
        let repo = state
            .account(&found.owner)
            .and_then(|account| account.repo(&found.name))
            .expect("find_repo found the repository");
        let labels = super::issues::repository_labels(repo);
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
                .ok_or_else(|| ApiResponse::error(500, "Pull request REST model is not seeded"))?
                .labels
                .iter()
                .map(pull_label)
                .collect::<Vec<_>>()
        } else if let Some(issue) = repo.issues.get(&number) {
            issue.data.labels.iter().filter_map(issue_label).collect()
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
        let repo = state
            .account(&found.owner)
            .and_then(|account| account.repo(&found.name))
            .expect("repository exists");
        let number = request
            .param("issue_number")
            .parse::<u64>()
            .map_err(|_| ApiResponse::not_found())?;
        let issue = repo
            .issues
            .get(&number)
            .ok_or_else(ApiResponse::not_found)?;
        Ok(ApiResponse::typed(200, &issue.data))
    }
}

impl Handler for ops::PullsCreate {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = request.require_scope(&["repo", "public_repo"])?;
        let body: types::PullsCreateRequest = request.json_as()?;
        let title = body.title.as_deref().unwrap_or_default();
        let head_ref = body.head.as_str();
        let base_ref = body.base.as_str();
        if title.is_empty() || head_ref.is_empty() || base_ref.is_empty() {
            return Err(ApiResponse::error(422, "title, head and base are required"));
        }
        let mut state = lock(state);
        let found = find_repo(&state, request)?;
        let account = state
            .account(&found.owner)
            .expect("repository owner exists");
        let repository = account.repo(&found.name).expect("repository exists");
        let number = repository
            .pull_requests
            .keys()
            .next_back()
            .copied()
            .unwrap_or(0)
            + 1;
        let user = crate::render::simple_user(
            request,
            state
                .account(&caller.login)
                .ok_or_else(ApiResponse::not_found)?,
        );
        let (head_owner, head_name) = head_ref.split_once(':').unwrap_or((&found.owner, head_ref));
        let head_user = state
            .account(head_owner)
            .map(|account| crate::render::simple_user(request, account))
            .unwrap_or_else(|| user.clone());
        let pull_repository =
            crate::render::repository_for_pull(request, account, repository, found.permission);
        let git_dir = repository
            .git_dir
            .as_ref()
            .ok_or_else(|| ApiResponse::error(500, "repository has no git storage"))?;
        let target_lineage = repository
            .fork_source
            .clone()
            .unwrap_or_else(|| (found.owner.clone(), found.name.clone()));
        let (head_git_dir, head_repository) = if head_owner.eq_ignore_ascii_case(&found.owner) {
            (git_dir.clone(), pull_repository.clone())
        } else {
            state
                .accounts()
                .find(|account| account.login.eq_ignore_ascii_case(head_owner))
                .and_then(|account| {
                    account.repos().find_map(|candidate| {
                        let same_network =
                            candidate.belongs_to_fork_network(&account.login, &target_lineage);
                        let source = candidate.git_dir.as_ref()?;
                        if !same_network
                            || ninot_common::git::resolve_commit(source, head_name)
                                .ok()
                                .flatten()
                                .is_none()
                        {
                            return None;
                        }
                        let permission = state.permission(
                            request.caller.as_ref(),
                            &account.login,
                            &candidate.name,
                        )?;
                        Some((
                            source.clone(),
                            crate::render::repository_for_pull(
                                request, account, candidate, permission,
                            ),
                        ))
                    })
                })
                .ok_or_else(|| {
                    ApiResponse::error(422, "head branch does not exist in this fork network")
                })?
        };
        if head_git_dir != *git_dir {
            crate::fixture_git::copy_refs(git_dir, &head_git_dir)
                .map_err(|error| ApiResponse::error(500, &error.to_string()))?;
        }
        let base_sha = ninot_common::git::resolve_commit(git_dir, base_ref)
            .map_err(|error| ApiResponse::error(500, &error.to_string()))?
            .ok_or_else(|| ApiResponse::error(422, "base branch does not exist"))?;
        let head_sha = ninot_common::git::resolve_commit(git_dir, head_name)
            .map_err(|error| ApiResponse::error(500, &error.to_string()))?
            .ok_or_else(|| ApiResponse::error(422, "head branch does not exist"))?;
        let api = format!("{}/repos/{}/{}", request.api_url, found.owner, found.name);
        let web = format!("{}/{}/{}", request.web_url, found.owner, found.name);
        let base = types::PullRequestBase {
            label: format!("{}:{base_ref}", found.owner),
            ref_: base_ref.to_owned(),
            repo: pull_repository,
            sha: base_sha,
            user: crate::render::simple_user(request, account),
        };
        let head = types::PullRequestHead {
            label: format!("{head_owner}:{head_name}"),
            ref_: head_name.to_owned(),
            repo: head_repository,
            sha: head_sha.clone(),
            user: head_user,
        };
        let now = crate::render::timestamp(&crate::state::now());
        let link = |href: String| types::Link { href };
        let data = types::PullRequest {
            active_lock_reason: None,
            additions: 0,
            assignee: None,
            assignees: Vec::new(),
            author_association: types::AuthorAssociation::Owner,
            auto_merge: None,
            base,
            body: body.body,
            changed_files: 0,
            closed_at: None,
            comments: 0,
            comments_url: format!("{api}/issues/{number}/comments"),
            commits: 1,
            commits_url: format!("{api}/pulls/{number}/commits"),
            created_at: now,
            deletions: 0,
            diff_url: format!("{web}/pull/{number}.diff"),
            draft: Some(body.draft.unwrap_or(false)),
            head,
            html_url: format!("{web}/pull/{number}"),
            id: crate::render::id(number),
            issue_url: format!("{api}/issues/{number}"),
            labels: Vec::new(),
            links: types::PullRequestLinks {
                comments: link(format!("{api}/issues/{number}/comments")),
                commits: link(format!("{api}/pulls/{number}/commits")),
                html: link(format!("{web}/pull/{number}")),
                issue: link(format!("{api}/issues/{number}")),
                review_comment: link(format!("{api}/pulls/comments{{/number}}")),
                review_comments: link(format!("{api}/pulls/{number}/comments")),
                self_: link(format!("{api}/pulls/{number}")),
                statuses: link(format!("{api}/statuses/{head_sha}")),
            },
            locked: false,
            maintainer_can_modify: body.maintainer_can_modify.unwrap_or(true),
            merge_commit_sha: None,
            mergeable: Some(true),
            mergeable_state: "clean".to_owned(),
            merged: false,
            merged_at: None,
            merged_by: None,
            milestone: None,
            node_id: format!("PR_{}_{}_{}", found.owner, found.name, number),
            number: crate::render::id(number),
            patch_url: format!("{web}/pull/{number}.patch"),
            rebaseable: Some(true),
            requested_reviewers: Vec::new(),
            requested_teams: Vec::new(),
            review_comment_url: format!("{api}/pulls/comments{{/number}}"),
            review_comments: 0,
            review_comments_url: format!("{api}/pulls/{number}/comments"),
            stack: None,
            state: types::PullRequestState::Open,
            statuses_url: format!("{api}/statuses/{head_sha}"),
            title: title.to_owned(),
            updated_at: now,
            url: format!("{api}/pulls/{number}"),
            user,
        };
        state.add_pull_request(&found.owner, &found.name, number, data.clone());
        let mut ctx = Ctx::new(&mut state, request, &found.owner, &found.name);
        events::pull_request::opened(&mut ctx, &data);
        Ok(ApiResponse::typed(201, &data))
    }
}

impl Handler for ops::IssuesAddLabels {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = request.require_scope(&["repo", "public_repo"])?;
        let body: types::IssuesAddLabelsRequest = request.json_as()?;
        let requested_labels = match body {
            types::IssuesAddLabelsRequest::Variant0 { labels } => {
                if labels.is_empty() {
                    return Err(ApiResponse::error(
                        422,
                        "labels must contain at least one label",
                    ));
                }
                labels
                    .into_iter()
                    .map(|label| match label {
                        types::IssuesAddLabelsRequestVariant0LabelsItem::String(name) => name,
                        types::IssuesAddLabelsRequestVariant0LabelsItem::Object {
                            name, ..
                        } => name,
                    })
                    .collect::<Vec<_>>()
            }
            types::IssuesAddLabelsRequest::Variant1(names) => names,
            types::IssuesAddLabelsRequest::Variant2(labels) => {
                labels.into_iter().map(|label| label.name).collect()
            }
        };
        let mut state = lock(state);
        let found = find_repo(&state, request)?;
        let number = request
            .param("issue_number")
            .parse::<u64>()
            .map_err(|_| ApiResponse::not_found())?;
        let api = format!("{}/repos/{}/{}", request.api_url, found.owner, found.name);
        let (labels, pull, issue, added) = {
            let repo = state
                .account_mut(&found.owner)
                .and_then(|account| account.repo_mut(&found.name))
                .ok_or_else(ApiResponse::not_found)?;
            let mut added = Vec::new();
            if let Some(pull) = repo.pull_requests.get_mut(&number) {
                let pull = pull.data.as_mut().ok_or_else(|| {
                    ApiResponse::error(500, "Pull request REST model is not seeded")
                })?;
                for name in requested_labels {
                    if !pull.labels.iter().any(|label| label.name == name) {
                        let id = number
                            .saturating_mul(100)
                            .saturating_add(pull.labels.len() as u64 + 1);
                        let item = types::PullRequestLabelsItem {
                            archived_at: serde_json::Value::Null,
                            archived_by: None,
                            color: "ededed".to_owned(),
                            default: false,
                            description: Some(String::new()),
                            id: crate::render::id(id),
                            name: name.clone(),
                            node_id: format!("LA_{}_{}_{}", found.owner, found.name, id),
                            url: format!("{api}/labels/{name}"),
                        };
                        added.push(pull_label(&item));
                        pull.labels.push(item);
                    }
                }
                let labels = pull.labels.iter().map(pull_label).collect::<Vec<_>>();
                (labels, Some(pull.clone()), None, added)
            } else if let Some(issue) = repo.issues.get_mut(&number) {
                for name in requested_labels {
                    let found_label = issue
                        .data
                        .labels
                        .iter()
                        .filter_map(issue_label)
                        .any(|label| label.name == name);
                    if !found_label {
                        let id = number
                            .saturating_mul(100)
                            .saturating_add(issue.data.labels.len() as u64 + 1);
                        let item = types::IssueLabelsItem::Object {
                            archived_by: None,
                            color: Some("ededed".to_owned()),
                            default: Some(false),
                            description: Some(String::new()),
                            id: Some(crate::render::id(id)),
                            name: Some(name.clone()),
                            node_id: Some(format!("LA_{}_{}_{}", found.owner, found.name, id)),
                            url: Some(format!("{api}/labels/{name}")),
                        };
                        if let Some(label) = issue_label(&item) {
                            added.push(label);
                        }
                        issue.data.labels.push(item);
                    }
                }
                let labels = issue
                    .data
                    .labels
                    .iter()
                    .filter_map(issue_label)
                    .collect::<Vec<_>>();
                (labels, None, Some(issue.data.clone()), added)
            } else {
                return Err(ApiResponse::not_found());
            }
        };
        let actor = caller_user(&state, request, &caller.login)?;
        for label in &added {
            super::issues::record::record_labeled(&mut state, &found, number, actor.clone(), label);
        }
        let mut ctx = Ctx::new(&mut state, request, &found.owner, &found.name);
        if let Some(pull) = &pull {
            for label in &added {
                events::pull_request::labeled(&mut ctx, pull, label);
            }
        }
        if let Some(issue) = &issue {
            for label in &added {
                events::issues::labeled(&mut ctx, issue, label);
            }
        }
        Ok(ApiResponse::typed(200, &labels))
    }
}

impl Handler for ops::IssuesCreateComment {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = request.require_scope(&["repo", "public_repo"])?;
        let body: types::IssuesCreateCommentRequest = request.json_as()?;
        let text = body.body.as_str();
        if text.is_empty() {
            return Err(ApiResponse::error(422, "body is required"));
        }
        let mut state = lock(state);
        let found = find_repo(&state, request)?;
        let number = request
            .param("issue_number")
            .parse::<u64>()
            .map_err(|_| ApiResponse::not_found())?;
        let account = state
            .account(&caller.login)
            .ok_or_else(ApiResponse::not_found)?;
        let user = crate::render::simple_user(request, account);
        let id = state.next_id();
        let api = format!("{}/repos/{}/{}", request.api_url, found.owner, found.name);
        let exists = state
            .account(&found.owner)
            .and_then(|account| account.repo(&found.name))
            .is_some_and(|repo| {
                repo.issues.contains_key(&number) || repo.pull_requests.contains_key(&number)
            });
        if !exists {
            return Err(ApiResponse::not_found());
        }
        let created_at = crate::render::timestamp(&crate::state::now());
        let comment = types::IssueComment {
            author_association: Some(types::AuthorAssociation::Owner),
            body: Some(text.to_owned()),
            body_html: None,
            body_text: None,
            created_at,
            html_url: format!("{}/issues/{number}#issuecomment-{id}", request.web_url),
            id: crate::render::id(id),
            issue_url: format!("{api}/issues/{number}"),
            minimized: None,
            node_id: crate::render::node_id("IssueComment", id),
            performed_via_github_app: None,
            pin: None,
            reactions: None,
            updated_at: created_at,
            url: format!("{api}/issues/comments/{id}"),
            user: Some(user.into()),
        };
        state.add_issue_comment(&found.owner, &found.name, number, comment.clone());
        let mut ctx = Ctx::new(&mut state, request, &found.owner, &found.name);
        events::issue_comment::created(&mut ctx, number, &comment);
        Ok(ApiResponse::typed(201, &comment))
    }
}

impl Handler for ops::PullsMerge {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = request.require_scope(&["repo", "public_repo"])?;
        let body: Option<types::PullsMergeRequestBody> = request.json_as()?;
        let body = body.unwrap_or_default();
        let mut state = lock(state);
        let found = find_repo(&state, request)?;
        if !found.permission.push {
            return Err(ApiResponse::error(
                403,
                "Resource not accessible by personal access token",
            ));
        }
        let number = request
            .param("pull_number")
            .parse::<u64>()
            .map_err(|_| ApiResponse::not_found())?;
        let actor = caller_user(&state, request, &caller.login)?;
        let (merged_id, closed_id) = (state.next_id(), state.next_id());
        let pull = state
            .account_mut(&found.owner)
            .and_then(|account| account.repo_mut(&found.name))
            .and_then(|repo| repo.pull_requests.get_mut(&number))
            .ok_or_else(ApiResponse::not_found)?;
        let data = pull
            .data
            .as_ref()
            .ok_or_else(|| ApiResponse::error(500, "Pull request REST model is not seeded"))?;
        let expected = body.sha.as_deref();
        if expected.is_some_and(|sha| data.head.sha != sha) {
            return Err(ApiResponse::error(409, "head SHA does not match"));
        }
        if data.state != types::PullRequestState::Open
            || data.mergeable == Some(false)
            || data.draft == Some(true)
        {
            return Err(ApiResponse::error(405, "pull request is not mergeable"));
        }
        let sha = data.head.sha.clone();
        let now = crate::render::timestamp(&crate::state::now());
        for (id, kind, commit_id) in [
            (merged_id, TimelineEventKind::Merged, Some(sha.clone())),
            (closed_id, TimelineEventKind::Closed, None),
        ] {
            pull.apply_event(TimelineEvent {
                id,
                kind,
                actor: actor.clone(),
                created_at: now,
                commit_id,
                requested_reviewer: None,
                requested_team: None,
            })
            .map_err(|message| ApiResponse::error(405, &message))?;
        }
        let data = pull
            .data
            .clone()
            .ok_or_else(|| ApiResponse::error(500, "Pull request REST model is not seeded"))?;
        let mut ctx = Ctx::new(&mut state, request, &found.owner, &found.name);
        events::pull_request::closed(&mut ctx, &data);
        Ok(ApiResponse::typed(
            200,
            &types::PullRequestMergeResult {
                merged: true,
                message: "Pull Request successfully merged".to_owned(),
                sha,
            },
        ))
    }
}

impl Handler for ops::PullsListReviews {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        pull_collection(state, request, "reviews", "pull_number")
    }
}

impl Handler for ops::PullsCreateReview {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = request.require_scope(&["repo", "public_repo"])?;
        let body: types::PullsCreateReviewRequest = request.json_as()?;
        let event = body.event.map_or("PENDING", |event| match event {
            types::PullsCreateReviewRequestEvent::Approve => "APPROVE",
            types::PullsCreateReviewRequestEvent::RequestChanges => "REQUEST_CHANGES",
            types::PullsCreateReviewRequestEvent::Comment => "COMMENT",
        });
        if event != "APPROVE" && body.body.as_deref().unwrap_or_default().is_empty() {
            return Err(ApiResponse::error(
                422,
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
            .ok_or_else(|| ApiResponse::error(500, "Pull request REST model is not seeded"))?;
        if matches!(event, "APPROVE" | "REQUEST_CHANGES")
            && data.user.login.eq_ignore_ascii_case(&caller.login)
        {
            return Err(ApiResponse::error(
                422,
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
        let pull_request_url = format!(
            "{}/repos/{}/{}/pulls/{number}",
            request.api_url, found.owner, found.name
        );
        let html_url = format!(
            "{}/{}/{}/pull/{}#pullrequestreview-{id}",
            request.web_url, found.owner, found.name, number
        );
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
        if event != "PENDING" {
            data.requested_reviewers
                .retain(|reviewer| !reviewer.login.eq_ignore_ascii_case(&caller.login));
        }
        pull.reviews.push(review.clone());
        let pull_data = pull.data.clone();
        if let Some(pull_data) = pull_data {
            let mut ctx = Ctx::new(&mut state, request, &found.owner, &found.name);
            events::pull_request_review::submitted(&mut ctx, &pull_data, &review);
        }
        Ok(ApiResponse::typed(200, &review))
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
            .ok_or_else(|| ApiResponse::error(500, "Pull request REST model is not seeded"))?;
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

impl Handler for ops::PullsRequestReviewers {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = request.require_scope(&["repo", "public_repo"])?;
        let body: types::PullsRequestReviewersRequest = request.json_as()?;
        let (reviewers, teams) = (body.reviewers, body.team_reviewers);
        if reviewers.is_empty() && teams.is_empty() {
            return Err(ApiResponse::error(
                422,
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
        let now = crate::render::timestamp(&crate::state::now());
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
                    ApiResponse::error(500, "Pull request REST model is not seeded")
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
                    pull.apply_event(TimelineEvent {
                        id,
                        kind: TimelineEventKind::ReviewRequested,
                        actor: actor.clone(),
                        created_at: now,
                        commit_id: None,
                        requested_reviewer: user,
                        requested_team: team,
                    })
                    .map_err(|message| ApiResponse::error(422, &message))?;
                }
            }
            let data = pull
                .data
                .clone()
                .ok_or_else(|| ApiResponse::error(500, "Pull request REST model is not seeded"))?;
            (data, added_users, added_teams)
        };
        let mut ctx = Ctx::new(&mut state, request, &found.owner, &found.name);
        for user in &added_users {
            events::pull_request::review_requested_user(&mut ctx, &data, user);
        }
        for team in &added_teams {
            events::pull_request::review_requested_team(&mut ctx, &data, team);
        }
        Ok(ApiResponse::typed(
            201,
            &crate::render::pull_request_simple(&data),
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
            .ok_or_else(|| ApiResponse::error(500, "Pull request REST model is not seeded"))?;
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
        let now = crate::render::timestamp(&crate::state::now());
        for ((user, team), id) in removals.into_iter().zip(ids) {
            pull.apply_event(TimelineEvent {
                id,
                kind: TimelineEventKind::ReviewRequestRemoved,
                actor: actor.clone(),
                created_at: now,
                commit_id: None,
                requested_reviewer: user,
                requested_team: team,
            })
            .map_err(|message| ApiResponse::error(422, &message))?;
        }
        let data = pull.data.as_ref().expect("checked above");
        Ok(ApiResponse::typed(
            200,
            &crate::render::pull_request_simple(data),
        ))
    }
}
