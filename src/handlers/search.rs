//! Issue and pull-request search over persisted repository state.

use std::sync::Mutex;

use super::Handler;
use crate::api::{ApiRequest, ApiResponse, ApiResult};
use crate::operation::ops;
use crate::state::{Account, Issue, PullRequest, State, lock};

/// A user-valued qualifier's login, where `@me` stands for the authenticated user.
fn login<'a>(value: &'a str, caller: &'a str) -> &'a str {
    if value.eq_ignore_ascii_case("@me") {
        caller
    } else {
        value
    }
}

/// Whether `login` is asked to review `pull` directly, as `user-review-requested:` matches.
fn user_review_requested(pull: &crate::types::PullRequest, login: &str) -> bool {
    pull.requested_reviewers
        .iter()
        .any(|reviewer| reviewer.login.eq_ignore_ascii_case(login))
}

/// Whether `login` is asked to review `pull` directly or through a team of the repository's
/// organization `owner`, as `review-requested:` matches.
pub(crate) fn review_requested(
    owner: &Account,
    pull: &crate::types::PullRequest,
    login: &str,
) -> bool {
    user_review_requested(pull, login)
        || pull
            .requested_teams
            .iter()
            .any(|team| owner.team_has_member(&team.slug, login))
}

/// Whether the team `org/slug` is asked to review `pull`, as `team-review-requested:` matches.
fn team_review_requested(owner: &Account, pull: &crate::types::PullRequest, team: &str) -> bool {
    team.split_once('/').is_some_and(|(org, slug)| {
        org.eq_ignore_ascii_case(&owner.login)
            && pull
                .requested_teams
                .iter()
                .any(|requested| requested.slug.eq_ignore_ascii_case(slug))
    })
}

/// Whether `full_name` matches the query's `repo:` qualifiers; GitHub ORs repeated ones.
fn in_searched_repo(query: &str, full_name: &str) -> bool {
    let mut wanted = query
        .split_whitespace()
        .filter_map(|term| term.strip_prefix("repo:"))
        .peekable();
    wanted.peek().is_none() || wanted.any(|repo| repo.eq_ignore_ascii_case(full_name))
}

fn matches_pull(
    query: &str,
    caller: &str,
    owner: &Account,
    repo: &str,
    pull: &PullRequest,
) -> bool {
    let Some(data) = pull.data.as_ref() else {
        return false;
    };
    if !in_searched_repo(query, &format!("{}/{repo}", owner.login)) {
        return false;
    }
    for term in query.split_whitespace() {
        if let Some(wanted) = term.strip_prefix("author:")
            && !data.user.login.eq_ignore_ascii_case(login(wanted, caller))
        {
            return false;
        }
        if let Some(wanted) = term.strip_prefix("review-requested:")
            && !review_requested(owner, data, login(wanted, caller))
        {
            return false;
        }
        if let Some(wanted) = term.strip_prefix("user-review-requested:")
            && !user_review_requested(data, login(wanted, caller))
        {
            return false;
        }
        if let Some(team) = term.strip_prefix("team-review-requested:")
            && !team_review_requested(owner, data, team)
        {
            return false;
        }
        if term == "is:open" && data.state != crate::types::PullRequestState::Open {
            return false;
        }
        if term == "is:closed" && data.state != crate::types::PullRequestState::Closed {
            return false;
        }
        if term == "is:merged" && !data.merged {
            return false;
        }
        if term == "is:unmerged" && data.merged {
            return false;
        }
        if let Some(number) = term.strip_prefix('#')
            && number
                .parse::<u64>()
                .is_ok_and(|number| u64::try_from(data.number).ok() != Some(number))
        {
            return false;
        }
    }
    true
}

fn matches_issue(query: &str, caller: &str, owner: &str, repo: &str, issue: &Issue) -> bool {
    if !in_searched_repo(query, &format!("{owner}/{repo}")) {
        return false;
    }
    for term in query.split_whitespace() {
        if let Some(wanted) = term.strip_prefix("author:")
            && !issue
                .data
                .user
                .as_ref()
                .map(|user| user.login.as_str())
                .is_some_and(|author| author.eq_ignore_ascii_case(login(wanted, caller)))
        {
            return false;
        }
        if term == "is:open" && issue.data.state != "open" {
            return false;
        }
        if term == "is:closed" && issue.data.state != "closed" {
            return false;
        }
        if term == "is:merged"
            || [
                "review-requested:",
                "user-review-requested:",
                "team-review-requested:",
            ]
            .iter()
            .any(|qualifier| term.starts_with(qualifier))
        {
            return false;
        }
        if let Some(number) = term.strip_prefix('#')
            && number
                .parse::<u64>()
                .is_ok_and(|number| u64::try_from(issue.data.number).ok() != Some(number))
        {
            return false;
        }
    }
    true
}

fn visible(state: &State, request: &ApiRequest, owner: &str, repo: &str) -> bool {
    state
        .permission(request.caller.as_ref(), owner, repo)
        .is_some()
}

#[derive(Clone, Copy)]
enum RepoSearchSort {
    Stars,
    Forks,
    Updated,
}

#[derive(Clone, Copy)]
enum RepoSearchOrder {
    Ascending,
    Descending,
}

/// The generated OpenAPI model contains the search response but does not generate query models.
/// Decode the operation's documented query parameters once into this typed request shape.
struct SearchReposQuery {
    q: String,
    sort: RepoSearchSort,
    order: RepoSearchOrder,
    page: usize,
    per_page: usize,
}

impl SearchReposQuery {
    fn from_request(request: &ApiRequest) -> Result<Self, ApiResponse> {
        let q = request
            .query("q")
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| ApiResponse::validation_failed("Search", "q", "missing"))?
            .to_owned();
        let sort = match request.query("sort") {
            None | Some("") => RepoSearchSort::Updated,
            Some("stars") => RepoSearchSort::Stars,
            Some("forks") => RepoSearchSort::Forks,
            Some("updated") => RepoSearchSort::Updated,
            Some(_) => return Err(ApiResponse::validation_failed("Search", "sort", "invalid")),
        };
        let order = match request.query("order") {
            None | Some("desc") => RepoSearchOrder::Descending,
            Some("asc") => RepoSearchOrder::Ascending,
            Some(_) => return Err(ApiResponse::validation_failed("Search", "order", "invalid")),
        };
        let page = parse_search_number(request, "page", 1, 1, None)?;
        let per_page = parse_search_number(request, "per_page", 30, 1, Some(100))?;
        Ok(Self {
            q,
            sort,
            order,
            page,
            per_page,
        })
    }
}

fn parse_search_number(
    request: &ApiRequest,
    name: &'static str,
    default: usize,
    minimum: usize,
    maximum: Option<usize>,
) -> Result<usize, ApiResponse> {
    let Some(value) = request.query(name) else {
        return Ok(default);
    };
    let parsed = value
        .parse::<usize>()
        .map_err(|_| ApiResponse::validation_failed("Search", name, "invalid"))?;
    if parsed < minimum || maximum.is_some_and(|maximum| parsed > maximum) {
        return Err(ApiResponse::validation_failed("Search", name, "invalid"));
    }
    Ok(parsed)
}

fn search_repo_item(full: crate::types::FullRepository) -> crate::types::RepoSearchResultItem {
    use crate::types as t;
    t::RepoSearchResultItem {
        allow_auto_merge: full.allow_auto_merge,
        allow_forking: full.allow_forking,
        allow_merge_commit: full.allow_merge_commit,
        allow_rebase_merge: full.allow_rebase_merge,
        allow_squash_merge: full.allow_squash_merge,
        archive_url: full.archive_url,
        archived: full.archived,
        assignees_url: full.assignees_url,
        blobs_url: full.blobs_url,
        branches_url: full.branches_url,
        clone_url: full.clone_url,
        collaborators_url: full.collaborators_url,
        comments_url: full.comments_url,
        commits_url: full.commits_url,
        compare_url: full.compare_url,
        contents_url: full.contents_url,
        contributors_url: full.contributors_url,
        created_at: full.created_at,
        default_branch: full.default_branch,
        delete_branch_on_merge: full.delete_branch_on_merge,
        deployments_url: full.deployments_url,
        description: full.description,
        disabled: full.disabled,
        downloads_url: full.downloads_url,
        events_url: full.events_url,
        fork: full.fork,
        forks: full.forks,
        forks_count: full.forks_count,
        forks_url: full.forks_url,
        full_name: full.full_name,
        git_commits_url: full.git_commits_url,
        git_refs_url: full.git_refs_url,
        git_tags_url: full.git_tags_url,
        git_url: full.git_url,
        has_discussions: Some(full.has_discussions),
        has_downloads: full.has_downloads.unwrap_or(true),
        has_issues: full.has_issues,
        has_pages: full.has_pages,
        has_projects: full.has_projects,
        has_pull_requests: full.has_pull_requests,
        has_wiki: full.has_wiki,
        homepage: full.homepage,
        hooks_url: full.hooks_url,
        html_url: full.html_url,
        id: full.id,
        is_template: full.is_template,
        issue_comment_url: full.issue_comment_url,
        issue_events_url: full.issue_events_url,
        issues_url: full.issues_url,
        keys_url: full.keys_url,
        labels_url: full.labels_url,
        language: full.language,
        languages_url: full.languages_url,
        license: full.license,
        master_branch: full.master_branch,
        merges_url: full.merges_url,
        milestones_url: full.milestones_url,
        mirror_url: full.mirror_url,
        name: full.name,
        node_id: full.node_id,
        notifications_url: full.notifications_url,
        open_issues: full.open_issues,
        open_issues_count: full.open_issues_count,
        owner: Some(nullable_simple_user(full.owner)),
        permissions: full
            .permissions
            .map(|permissions| t::RepoSearchResultItemPermissions {
                admin: permissions.admin,
                maintain: permissions.maintain,
                pull: permissions.pull,
                push: permissions.push,
                triage: permissions.triage,
            }),
        private: full.private,
        pull_request_creation_policy: full.pull_request_creation_policy.map(
            |policy| match policy {
                t::FullRepositoryPullRequestCreationPolicy::All => {
                    t::RepoSearchResultItemPullRequestCreationPolicy::All
                }
                t::FullRepositoryPullRequestCreationPolicy::CollaboratorsOnly => {
                    t::RepoSearchResultItemPullRequestCreationPolicy::CollaboratorsOnly
                }
            },
        ),
        pulls_url: full.pulls_url,
        pushed_at: full.pushed_at,
        releases_url: full.releases_url,
        score: 1.0,
        size: full.size,
        ssh_url: full.ssh_url,
        stargazers_count: full.stargazers_count,
        stargazers_url: full.stargazers_url,
        statuses_url: full.statuses_url,
        subscribers_url: full.subscribers_url,
        subscription_url: full.subscription_url,
        svn_url: full.svn_url,
        tags_url: full.tags_url,
        teams_url: full.teams_url,
        temp_clone_token: full.temp_clone_token,
        text_matches: None,
        topics: full.topics,
        trees_url: full.trees_url,
        updated_at: full.updated_at,
        url: full.url,
        visibility: full.visibility,
        watchers: full.watchers,
        watchers_count: full.watchers_count,
        web_commit_signoff_required: full.web_commit_signoff_required,
    }
}

fn nullable_simple_user(user: crate::types::SimpleUser) -> crate::types::NullableSimpleUser {
    crate::types::NullableSimpleUser {
        avatar_url: user.avatar_url,
        email: user.email,
        events_url: user.events_url,
        followers_url: user.followers_url,
        following_url: user.following_url,
        gists_url: user.gists_url,
        gravatar_id: user.gravatar_id,
        html_url: user.html_url,
        id: user.id,
        login: user.login,
        name: user.name,
        node_id: user.node_id,
        organizations_url: user.organizations_url,
        received_events_url: user.received_events_url,
        repos_url: user.repos_url,
        site_admin: user.site_admin,
        starred_at: user.starred_at,
        starred_url: user.starred_url,
        subscriptions_url: user.subscriptions_url,
        type_: user.type_,
        url: user.url,
        user_view_type: user.user_view_type,
    }
}

fn search_issue_labels(
    labels: &[crate::types::IssueLabelsItem],
) -> Vec<crate::types::IssueSearchResultItemLabelsItem> {
    labels
        .iter()
        .map(|label| match label {
            crate::types::IssueLabelsItem::String(name) => {
                crate::types::IssueSearchResultItemLabelsItem {
                    name: Some(name.clone()),
                    ..Default::default()
                }
            }
            crate::types::IssueLabelsItem::Object {
                archived_by,
                color,
                default,
                description,
                id,
                name,
                node_id,
                url,
            } => crate::types::IssueSearchResultItemLabelsItem {
                archived_by: archived_by.clone(),
                color: color.clone(),
                default: *default,
                description: description.clone(),
                id: *id,
                name: name.clone(),
                node_id: node_id.clone(),
                url: url.clone(),
                ..Default::default()
            },
        })
        .collect()
}

fn search_pull_labels(
    labels: &[crate::types::PullRequestLabelsItem],
) -> Vec<crate::types::IssueSearchResultItemLabelsItem> {
    labels
        .iter()
        .map(|label| crate::types::IssueSearchResultItemLabelsItem {
            archived_by: label.archived_by.clone(),
            color: Some(label.color.clone()),
            default: Some(label.default),
            description: label.description.clone(),
            id: Some(label.id),
            name: Some(label.name.clone()),
            node_id: Some(label.node_id.clone()),
            url: Some(label.url.clone()),
            ..Default::default()
        })
        .collect()
}

fn search_issue_item(
    issue: &crate::types::Issue,
    repository_url: String,
) -> crate::types::IssueSearchResultItem {
    crate::types::IssueSearchResultItem {
        active_lock_reason: issue.active_lock_reason.clone(),
        assignee: issue.assignee.clone(),
        assignees: Some(issue.assignees.clone()),
        author_association: issue
            .author_association
            .unwrap_or(crate::types::AuthorAssociation::None),
        body: issue.body.clone(),
        body_html: issue.body_html.clone(),
        body_text: issue.body_text.clone(),
        closed_at: issue.closed_at,
        comments: issue.comments,
        comments_url: issue.comments_url.clone(),
        created_at: issue.created_at,
        draft: issue.draft,
        events_url: issue.events_url.clone(),
        html_url: issue.html_url.clone(),
        id: issue.id,
        issue_dependencies_summary: issue.issue_dependencies_summary.clone(),
        issue_field_values: issue.issue_field_values.clone(),
        labels: search_issue_labels(&issue.labels),
        labels_url: issue.labels_url.clone(),
        locked: issue.locked,
        milestone: issue.milestone.clone(),
        node_id: issue.node_id.clone(),
        number: issue.number,
        performed_via_github_app: issue.performed_via_github_app.clone(),
        pinned_comment: issue.pinned_comment.clone(),
        pull_request: None,
        reactions: issue.reactions.clone(),
        repository: issue.repository.clone(),
        repository_url,
        score: 1.0,
        state: issue.state.clone(),
        state_reason: issue.state_reason.map(|reason| reason.to_string()),
        sub_issues_summary: issue.sub_issues_summary.clone(),
        text_matches: None,
        timeline_url: issue.timeline_url.clone(),
        title: issue.title.clone(),
        type_: issue.type_.clone(),
        updated_at: issue.updated_at,
        url: issue.url.clone(),
        user: issue.user.clone(),
    }
}

fn search_pull_item(
    pull: &crate::types::PullRequest,
    repository_url: String,
) -> crate::types::IssueSearchResultItem {
    let issue_url = pull.issue_url.clone();
    crate::types::IssueSearchResultItem {
        active_lock_reason: pull.active_lock_reason.clone(),
        assignee: pull.assignee.clone(),
        assignees: Some(pull.assignees.clone()),
        author_association: pull.author_association,
        body: pull.body.clone(),
        body_html: None,
        body_text: None,
        closed_at: pull.closed_at,
        comments: pull.comments,
        comments_url: pull.comments_url.clone(),
        created_at: pull.created_at,
        draft: pull.draft,
        events_url: format!("{issue_url}/events"),
        html_url: pull.html_url.clone(),
        id: pull.id,
        issue_dependencies_summary: None,
        issue_field_values: Vec::new(),
        labels: search_pull_labels(&pull.labels),
        labels_url: format!("{issue_url}/labels{{/name}}"),
        locked: pull.locked,
        milestone: pull.milestone.clone(),
        node_id: pull.node_id.clone(),
        number: pull.number,
        performed_via_github_app: None,
        pinned_comment: None,
        pull_request: Some(crate::types::IssueSearchResultItemPullRequest {
            diff_url: Some(pull.diff_url.clone()),
            html_url: Some(pull.html_url.clone()),
            merged_at: pull.merged_at,
            patch_url: Some(pull.patch_url.clone()),
            url: Some(pull.url.clone()),
        }),
        reactions: None,
        repository: None,
        repository_url,
        score: 1.0,
        state: pull.state.to_string(),
        state_reason: None,
        sub_issues_summary: None,
        text_matches: None,
        timeline_url: Some(format!("{issue_url}/timeline")),
        title: pull.title.clone(),
        type_: None,
        updated_at: pull.updated_at,
        url: pull.url.clone(),
        user: Some(pull.user.clone().into()),
    }
}

impl Handler for ops::SearchIssuesAndPullRequests {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let query = request.query("q").unwrap_or_default();
        let caller = request
            .caller
            .as_ref()
            .map_or("", |caller| caller.login.as_str());
        let state = lock(state);
        let mut items = Vec::new();
        for account in state.accounts() {
            for repo in account.repos() {
                if !visible(&state, request, &account.login, &repo.name) {
                    continue;
                }
                let api_repo = format!("{}/repos/{}/{}", request.api_url, account.login, repo.name);
                for pull in repo.pull_requests.values() {
                    let Some(data) = pull.data.as_ref() else {
                        continue;
                    };
                    if !matches_pull(query, caller, account, &repo.name, pull) {
                        continue;
                    }
                    items.push(search_pull_item(data, api_repo.clone()));
                }
                for issue in repo.issues.values() {
                    if !matches_issue(query, caller, &account.login, &repo.name, issue) {
                        continue;
                    }
                    items.push(search_issue_item(&issue.data, api_repo.clone()));
                }
            }
        }
        let total_count = items.len();
        let per_page = request
            .query("per_page")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(30)
            .clamp(1, 100);
        let page = request
            .query("page")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(1)
            .max(1);
        items = items
            .into_iter()
            .skip((page - 1).saturating_mul(per_page))
            .take(per_page)
            .collect();
        Ok(ApiResponse::typed(
            200,
            &crate::types::SearchIssuesAndPullRequestsResponse {
                incomplete_results: false,
                items,
                lexical_fallback_reason: Vec::new(),
                search_type: crate::types::SearchIssuesAndPullRequestsResponseSearchType::Lexical,
                total_count: i64::try_from(total_count).unwrap_or(i64::MAX),
            },
        ))
    }
}

impl Handler for ops::SearchRepos {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let query = SearchReposQuery::from_request(request)?;
        let caller = request
            .caller
            .as_ref()
            .map_or("", |caller| caller.login.as_str());
        let terms = query.q.split_whitespace().collect::<Vec<_>>();
        let user = terms
            .iter()
            .find_map(|term| term.strip_prefix("user:"))
            .map(|user| login(user, caller));
        let organization = terms.iter().find_map(|term| term.strip_prefix("org:"));
        let text_terms = terms
            .iter()
            .copied()
            .filter(|term| !term.contains(':'))
            .collect::<Vec<_>>();
        let state = lock(state);
        let mut items = Vec::new();
        for account in state.accounts() {
            if user.is_some_and(|login| {
                account.kind != crate::state::AccountKind::User
                    || !account.login.eq_ignore_ascii_case(login)
            }) || organization.is_some_and(|login| {
                account.kind != crate::state::AccountKind::Organization
                    || !account.login.eq_ignore_ascii_case(login)
            }) {
                continue;
            }
            for repo in account.repos() {
                if !visible(&state, request, &account.login, &repo.name) {
                    continue;
                }
                let haystack = format!(
                    "{} {} {}",
                    account.login,
                    repo.name,
                    repo.description.as_deref().unwrap_or_default()
                )
                .to_lowercase();
                if !text_terms
                    .iter()
                    .all(|term| haystack.contains(&term.to_lowercase()))
                {
                    continue;
                }
                let permission = state
                    .permission(request.caller.as_ref(), &account.login, &repo.name)
                    .expect("visible repository has permission");
                items.push(search_repo_item(crate::render::full_repository(
                    request, account, repo, permission,
                )));
            }
        }
        items.sort_by(|left, right| {
            let compared = match query.sort {
                RepoSearchSort::Stars => left.stargazers_count.cmp(&right.stargazers_count),
                RepoSearchSort::Forks => left.forks_count.cmp(&right.forks_count),
                RepoSearchSort::Updated => left.updated_at.cmp(&right.updated_at),
            };
            let compared = if matches!(query.order, RepoSearchOrder::Descending) {
                compared.reverse()
            } else {
                compared
            };
            compared.then_with(|| left.full_name.cmp(&right.full_name))
        });
        let total_count = i64::try_from(items.len()).unwrap_or(i64::MAX);
        let per_page = query.per_page;
        let page = query.page;
        let last_page = items.len().div_ceil(per_page).max(1);
        let items = items
            .into_iter()
            .skip((page - 1).saturating_mul(per_page))
            .take(per_page)
            .collect();
        let response = crate::types::SearchReposResponse {
            incomplete_results: false,
            items,
            total_count,
        };
        let mut api_response = ApiResponse::typed(200, &response);
        let mut links = Vec::new();
        for (target, relation) in [
            (page.saturating_add(1), "next"),
            (last_page, "last"),
            (1, "first"),
            (page.saturating_sub(1), "prev"),
        ] {
            let include = match relation {
                "next" | "last" => page < last_page,
                "first" | "prev" => page > 1,
                _ => false,
            };
            if !include {
                continue;
            }
            let mut query = url::form_urlencoded::Serializer::new(String::new());
            for (key, value) in &request.query {
                if key != "page" && key != "per_page" {
                    query.append_pair(key, value);
                }
            }
            query
                .append_pair("per_page", &per_page.to_string())
                .append_pair("page", &target.to_string());
            links.push(format!(
                "<{}{}?{}>; rel=\"{}\"",
                request.api_url,
                request.path,
                query.finish(),
                relation
            ));
        }
        if !links.is_empty() {
            api_response
                .headers
                .push(("link".to_owned(), links.join(", ")));
        }
        Ok(api_response)
    }
}
