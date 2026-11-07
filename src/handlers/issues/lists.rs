//! Cross-repository issue lists for the caller, an organization, and the caller's account.
//!
//! Every pull request is an issue. `subscribed` matches nothing: this fake stores no
//! subscriptions. `collab` is every visible repository the caller neither owns nor administers
//! through organization membership.

use std::sync::Mutex;

use chrono::{DateTime, Utc};

use super::super::Handler;
use crate::api::{ApiRequest, ApiResponse, ApiResult, Caller, paginate};
use crate::operation::ops;
use crate::render;
use crate::state::{AccountKind, State, lock};
use crate::types;

const FILTERS: [&str; 6] = [
    "assigned",
    "created",
    "mentioned",
    "subscribed",
    "repos",
    "all",
];

enum Scope {
    Visible {
        owned: Option<bool>,
        orgs: Option<bool>,
        collab: Option<bool>,
    },
    Administered,
    Organization(String),
}

struct Selection {
    owner: String,
    name: String,
    kind: AccountKind,
    administered: bool,
}

#[derive(Clone, Copy)]
enum Participation {
    Assigned,
    Created,
    Mentioned,
    Subscribed,
    All,
}

struct Criteria {
    participation: Participation,
    state: String,
    labels: Vec<String>,
    sort: String,
    ascending: bool,
    since: Option<DateTime<Utc>>,
    include_pulls: bool,
    type_name: Option<String>,
}

enum Pulls {
    FromQuery,
    Included,
}

enum TypeQuery {
    FromQuery,
    Ignored,
}

impl Handler for ops::IssuesList {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = request.require_caller()?;
        let scope = visible_scope(request)?;
        let criteria = criteria(request, Pulls::FromQuery, TypeQuery::Ignored)?;
        let rows = {
            let state = lock(state);
            listed(&state, caller, scope, &criteria)
        };
        Ok(paginate(request, &request.path, rows))
    }
}

impl Handler for ops::IssuesListForOrg {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = request.require_caller()?;
        let org = request.param("org").to_owned();
        let state = lock(state);
        if !is_organization(&state, &org) {
            return Err(ApiResponse::not_found());
        }
        let criteria = criteria(request, Pulls::Included, TypeQuery::FromQuery)?;
        let rows = listed(&state, caller, Scope::Organization(org), &criteria);
        Ok(paginate(request, &request.path, rows))
    }
}

impl Handler for ops::IssuesListForAuthenticatedUser {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = request.require_caller()?;
        let criteria = criteria(request, Pulls::Included, TypeQuery::Ignored)?;
        let rows = {
            let state = lock(state);
            listed(&state, caller, Scope::Administered, &criteria)
        };
        Ok(paginate(request, &request.path, rows))
    }
}

fn visible_scope(request: &ApiRequest) -> Result<Scope, ApiResponse> {
    Ok(Scope::Visible {
        owned: super::query_bool(request, "owned")?,
        orgs: super::query_bool(request, "orgs")?,
        collab: super::query_bool(request, "collab")?,
    })
}

fn criteria(
    request: &ApiRequest,
    pulls: Pulls,
    type_query: TypeQuery,
) -> Result<Criteria, ApiResponse> {
    let participation = match super::query_choice(request, "Issue", "filter", &FILTERS, "assigned")?
    {
        "created" => Participation::Created,
        "mentioned" => Participation::Mentioned,
        "subscribed" => Participation::Subscribed,
        "repos" | "all" => Participation::All,
        _ => Participation::Assigned,
    };
    let state = super::query_choice(
        request,
        "Issue",
        "state",
        &["open", "closed", "all"],
        "open",
    )?
    .to_owned();
    let sort = super::query_choice(
        request,
        "Issue",
        "sort",
        &["created", "updated", "comments"],
        "created",
    )?
    .to_owned();
    let ascending =
        super::query_choice(request, "Issue", "direction", &["asc", "desc"], "desc")? == "asc";
    let since = match request.query("since") {
        Some(value) => Some(super::parse_timestamp(value, "Issue", "since")?),
        None => None,
    };
    let labels = request
        .query("labels")
        .map(split_labels)
        .unwrap_or_default();
    let include_pulls = match pulls {
        Pulls::Included => true,
        Pulls::FromQuery => super::query_bool(request, "pulls")?.unwrap_or(true),
    };
    let type_name = match type_query {
        TypeQuery::Ignored => None,
        TypeQuery::FromQuery => request.query("type").map(str::to_owned),
    };
    Ok(Criteria {
        participation,
        state,
        labels,
        sort,
        ascending,
        since,
        include_pulls,
        type_name,
    })
}

fn split_labels(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|label| !label.is_empty())
        .map(str::to_owned)
        .collect()
}

fn is_organization(state: &State, org: &str) -> bool {
    state
        .account(org)
        .is_some_and(|account| account.kind == AccountKind::Organization)
}

fn listed(state: &State, caller: &Caller, scope: Scope, criteria: &Criteria) -> Vec<types::Issue> {
    let repos = selected(state, caller, &scope);
    let mut rows = issues_from(state, &repos, criteria, &caller.login);
    sort_rows(&mut rows, &criteria.sort, criteria.ascending);
    rows
}

fn selected(state: &State, caller: &Caller, scope: &Scope) -> Vec<Selection> {
    let mut rows = Vec::new();
    for account in state.accounts() {
        let administered = account.is_administered_by(&caller.login);
        let kind = account.kind;
        let owner = account.login.clone();
        for repo in account.repos() {
            rows.push(Selection {
                owner: owner.clone(),
                name: repo.name.clone(),
                kind,
                administered,
            });
        }
    }
    rows.retain(|repo| {
        state
            .permission(Some(caller), &repo.owner, &repo.name)
            .is_some()
    });
    rows.into_iter()
        .filter(|repo| in_scope(repo, scope))
        .collect()
}

fn in_scope(repo: &Selection, scope: &Scope) -> bool {
    match scope {
        Scope::Administered => repo.administered,
        Scope::Organization(org) => repo.owner.eq_ignore_ascii_case(org),
        Scope::Visible {
            owned,
            orgs,
            collab,
        } => {
            let restricted = owned.is_some() || orgs.is_some() || collab.is_some();
            if !restricted {
                return true;
            }
            let user_owned = repo.kind == AccountKind::User && repo.administered;
            let org_member = repo.kind == AccountKind::Organization && repo.administered;
            (*owned == Some(true) && user_owned)
                || (*orgs == Some(true) && org_member)
                || (*collab == Some(true) && !user_owned && !org_member)
        }
    }
}

fn issues_from(
    state: &State,
    repos: &[Selection],
    criteria: &Criteria,
    login: &str,
) -> Vec<types::Issue> {
    let mut rows = Vec::new();
    for repo in repos {
        let Some(repository) = state
            .account(&repo.owner)
            .and_then(|account| account.repo(&repo.name))
        else {
            continue;
        };
        for issue in repository.issues.values() {
            if accepts(&issue.data, criteria, login) {
                rows.push(issue.data.clone());
            }
        }
        if !criteria.include_pulls {
            continue;
        }
        for pull in repository
            .pull_requests
            .values()
            .filter_map(|pull| pull.data.as_ref())
        {
            let issue = render::issue_from_pull(pull);
            if accepts(&issue, criteria, login) {
                rows.push(issue);
            }
        }
    }
    rows
}

fn accepts(issue: &types::Issue, criteria: &Criteria, login: &str) -> bool {
    if criteria.type_name.as_ref().is_some_and(|name| {
        issue.type_.as_ref().map(|kind| kind.name.as_str()) != Some(name.as_str())
    }) {
        return false;
    }
    if criteria.state != "all" && issue.state != criteria.state {
        return false;
    }
    if criteria
        .since
        .is_some_and(|since| issue.updated_at <= since)
    {
        return false;
    }
    if !has_labels(issue, &criteria.labels) {
        return false;
    }
    participates(issue, criteria.participation, login)
}

fn has_labels(issue: &types::Issue, wanted: &[String]) -> bool {
    let names = super::label_names(issue);
    wanted
        .iter()
        .all(|label| names.iter().any(|name| name.eq_ignore_ascii_case(label)))
}

fn participates(issue: &types::Issue, participation: Participation, login: &str) -> bool {
    match participation {
        Participation::Assigned => super::is_assigned(issue, login),
        Participation::Created => {
            super::author_login(issue).is_some_and(|author| author.eq_ignore_ascii_case(login))
        }
        Participation::Mentioned => super::mentions(issue, login),
        Participation::Subscribed => false,
        Participation::All => true,
    }
}

fn sort_rows(rows: &mut [types::Issue], sort: &str, ascending: bool) {
    rows.sort_by(|left, right| {
        let order = match sort {
            "updated" => left.updated_at.cmp(&right.updated_at),
            "comments" => left.comments.cmp(&right.comments),
            _ => left.created_at.cmp(&right.created_at),
        }
        .then(left.number.cmp(&right.number));
        if ascending { order } else { order.reverse() }
    });
}
