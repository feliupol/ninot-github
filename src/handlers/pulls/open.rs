//! Opening, labeling, commenting on and merging pull requests, plus issue comments.

use std::sync::Mutex;

use super::super::Handler;
use super::super::issues::{label_from_issue_item, label_from_pull};
use super::super::repos::find_repo;
use super::caller_user;
use crate::api::{ApiRequest, ApiResponse, ApiResult};
use crate::operation::ops;
use crate::state::{Name, State, lock};
use crate::types;
use crate::webhooks::events::{self, Ctx};

impl Handler for ops::PullsCreate {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = request.require_scope(&["repo", "public_repo"])?;
        let body: types::PullsCreateRequest = request.json_as()?;
        let title = body.title.as_deref().unwrap_or_default();
        let head_ref = body.head.as_str();
        let base_ref = body.base.as_str();
        if title.is_empty() || head_ref.is_empty() || base_ref.is_empty() {
            return Err(crate::errors::GithubError::TitleHeadBaseRequired.to_response());
        }
        let mut state = lock(state);
        let found = find_repo(&state, request)?;
        let account = state
            .account(&found.owner)
            .expect("repository owner exists");
        let repository = account.repo(&found.name).expect("repository exists");
        let number = super::super::issues::next_subject_number(repository);
        let repository_id = repository.id;
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
            .ok_or_else(|| ApiResponse::internal("repository has no git storage"))?;
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
                .ok_or_else(|| crate::errors::GithubError::HeadNotInForkNetwork.to_response())?
        };
        if head_git_dir != *git_dir {
            crate::fixture_git::copy_refs(git_dir, &head_git_dir).map_err(ApiResponse::internal)?;
        }
        // A missing branch is a validation failure; the status only. Source: OpenAPI pulls/create 422
        let base_sha = ninot_common::git::resolve_commit(git_dir, base_ref)
            .map_err(ApiResponse::internal)?
            .ok_or_else(|| crate::errors::GithubError::BaseBranchMissing.to_response())?;
        let head_sha = ninot_common::git::resolve_commit(git_dir, head_name)
            .map_err(ApiResponse::internal)?
            .ok_or_else(|| crate::errors::GithubError::HeadBranchMissing.to_response())?;
        let repo_urls = request.urls().repo(&found.owner, &found.name);
        let urls = repo_urls.pull(number);
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
        let now = crate::time::now_utc();
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
            comments_url: urls.issue.comments(),
            commits: 1,
            commits_url: urls.commits(),
            created_at: now,
            deletions: 0,
            diff_url: urls.diff(),
            draft: Some(body.draft.unwrap_or(false)),
            head,
            html_url: urls.html.clone(),
            id: crate::render::id(number),
            issue_url: urls.issue.api.clone(),
            labels: Vec::new(),
            links: types::PullRequestLinks {
                comments: link(urls.issue.comments()),
                commits: link(urls.commits()),
                html: link(urls.html.clone()),
                issue: link(urls.issue.api.clone()),
                review_comment: link(repo_urls.review_comment_template()),
                review_comments: link(urls.review_comments()),
                self_: link(urls.api.clone()),
                statuses: link(repo_urls.statuses(&head_sha)),
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
            node_id: crate::ids::pull_request_node_id(repository_id, number),
            number: crate::render::id(number),
            patch_url: urls.patch(),
            rebaseable: Some(true),
            requested_reviewers: Vec::new(),
            requested_teams: Vec::new(),
            review_comment_url: repo_urls.review_comment_template(),
            review_comments: 0,
            review_comments_url: urls.review_comments(),
            stack: None,
            state: types::PullRequestState::Open,
            statuses_url: repo_urls.statuses(&head_sha),
            title: title.to_owned(),
            updated_at: now,
            url: urls.api.clone(),
            user,
        };
        state.add_pull_request(&found.owner, &found.name, number, data.clone());
        let mut ctx = Ctx::new(&mut state, request, &found.owner, &found.name);
        events::pull_request::opened(&mut ctx, &data)?;
        Ok(ApiResponse::created(&data))
    }
}

impl Handler for ops::IssuesAddLabels {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = request.require_scope(&["repo", "public_repo"])?;
        let body: types::IssuesAddLabelsRequest = request.json_as()?;
        let requested_labels = match body {
            types::IssuesAddLabelsRequest::Variant0 { labels } => {
                if labels.is_empty() {
                    return Err(crate::errors::GithubError::LabelsRequired.to_response());
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
        let repo_urls = request.urls().repo(&found.owner, &found.name);
        let (labels, pull, is_issue, added) = {
            let repo = state
                .account_mut(&found.owner)
                .and_then(|account| account.repo_mut(&found.name))
                .ok_or_else(ApiResponse::not_found)?;
            let mut added = Vec::new();
            if let Some(pull) = repo.pull_requests.get_mut(&number) {
                let pull = pull.data.as_mut().ok_or_else(|| {
                    ApiResponse::internal("Pull request REST model is not seeded")
                })?;
                for name in requested_labels.clone() {
                    if !pull.labels.iter().any(|label| label.name == name) {
                        let id = number
                            .saturating_mul(100)
                            .saturating_add(pull.labels.len() as u64 + 1);
                        let item = types::PullRequestLabelsItem {
                            archived_at: None,
                            archived_by: None,
                            color: "ededed".to_owned(),
                            default: false,
                            description: Some(String::new()),
                            id: crate::render::id(id),
                            name: name.clone(),
                            node_id: format!("LA_{}_{}_{}", found.owner, found.name, id),
                            url: repo_urls.label(&name),
                        };
                        added.push(label_from_pull(&item));
                        pull.labels.push(item);
                    }
                }
                let labels = pull.labels.iter().map(label_from_pull).collect::<Vec<_>>();
                (labels, Some(pull.clone()), false, added)
            } else if repo.issues.contains_key(&number) {
                (Vec::new(), None, true, added)
            } else {
                return Err(ApiResponse::not_found());
            }
        };
        let mut added = added;
        if is_issue {
            for name in requested_labels {
                let issue = &found.repo(&state).issues[&number];
                if issue.has_label(&name) {
                    continue;
                }
                let id = number
                    .saturating_mul(100)
                    .saturating_add(issue.labels.len() as u64 + 1);
                let label = super::super::issues::resolve_issue_label(
                    &mut state,
                    request,
                    &found,
                    &name,
                    || types::Label {
                        archived_at: None,
                        archived_by: None,
                        color: "ededed".to_owned(),
                        default: false,
                        description: Some(String::new()),
                        id: crate::render::id(id),
                        name: name.clone(),
                        node_id: format!("LA_{}_{}_{}", found.owner, found.name, id),
                        url: repo_urls.label(&name),
                    },
                );
                let issue = found
                    .repo_mut(&mut state)
                    .issues
                    .get_mut(&number)
                    .expect("issue exists");
                if !issue.has_label(&label.name) {
                    issue.labels.push(Name::new(label.name.clone()));
                    added.push(label);
                }
            }
        }
        let actor = caller_user(&state, request, &caller.login)?;
        for label in &added {
            super::super::issues::record::record_labeled(
                &mut state,
                &found,
                number,
                actor.clone(),
                label,
            );
        }
        let mut ctx = Ctx::new(&mut state, request, &found.owner, &found.name);
        if let Some(pull) = &pull {
            for label in &added {
                events::pull_request::labeled(&mut ctx, pull, label)?;
            }
        }
        let labels = match super::super::issues::rendered_issue(ctx.state, request, &found, number)
        {
            Some(issue) if is_issue => {
                for label in &added {
                    events::issues::labeled(&mut ctx, &issue, label)?;
                }
                issue
                    .labels
                    .iter()
                    .filter_map(label_from_issue_item)
                    .collect()
            }
            _ => labels,
        };
        Ok(ApiResponse::ok_typed(&labels))
    }
}

impl Handler for ops::IssuesCreateComment {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = request.require_scope(&["repo", "public_repo"])?;
        let body: types::IssuesCreateCommentRequest = request.json_as()?;
        let text = body.body.as_str();
        if text.is_empty() {
            return Err(crate::errors::GithubError::BodyRequired.to_response());
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
        let author = Name::new(account.login.clone());
        let user = crate::render::simple_user(request, account);
        let id = state.next_id();
        let repo_urls = request.urls().repo(&found.owner, &found.name);
        let issue_urls = repo_urls.issue(number);
        let (is_pull, is_issue) = state
            .account(&found.owner)
            .and_then(|account| account.repo(&found.name))
            .map_or((false, false), |repo| {
                (
                    repo.pull_requests.contains_key(&number),
                    repo.issues.contains_key(&number),
                )
            });
        if !is_pull && !is_issue {
            return Err(ApiResponse::not_found());
        }
        let created_at = crate::time::now_utc();
        let comment = if is_pull {
            let comment = types::IssueComment {
                author_association: Some(types::AuthorAssociation::Owner),
                body: Some(text.to_owned()),
                body_html: None,
                body_text: None,
                created_at,
                html_url: repo_urls.pull(number).comment_html(id),
                id: crate::render::id(id),
                issue_url: issue_urls.api,
                minimized: None,
                node_id: crate::render::node_id("IssueComment", id),
                performed_via_github_app: None,
                pin: None,
                reactions: None,
                updated_at: created_at,
                url: repo_urls.issue_comment(id),
                user: Some(user.into()),
            };
            state.add_issue_comment(&found.owner, &found.name, number, comment.clone());
            comment
        } else {
            let record = crate::state::Comment {
                id,
                author: Some(author),
                body: text.to_owned(),
                created_at,
                updated_at: created_at,
            };
            let issue = found
                .repo_mut(&mut state)
                .issues
                .get_mut(&number)
                .expect("issue exists");
            issue.comments.push(record.clone());
            crate::render::issue_comment(
                super::super::issues::context(&state, request, &found),
                number,
                &record,
            )
        };
        let mut ctx = Ctx::new(&mut state, request, &found.owner, &found.name);
        events::issue_comment::created(&mut ctx, number, &comment)?;
        Ok(ApiResponse::created(&comment))
    }
}
