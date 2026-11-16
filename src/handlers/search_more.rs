//! User, label and topic search, and the query parsing every search shares.
//!
//! Terms without a qualifier are keywords every result must contain, ignoring case.
//! Source: <https://docs.github.com/en/rest/search/search>

use std::sync::Mutex;

use super::Handler;
use crate::api::{ApiRequest, ApiResponse, ApiResult, PER_PAGE_MAX, paginate_with};
use crate::handlers::repos::Found;
use crate::operation::ops;
use crate::render;
use crate::state::{Account, AccountKind, State, lock};
use crate::types;

/// A search request's `q`, split into keywords and `qualifier:value` pairs, and its `order`.
pub(crate) struct Query {
    pub keywords: Vec<String>,
    pub qualifiers: Vec<(String, String)>,
    pub ascending: bool,
}

impl Query {
    /// Parses `q`, required by every search, and `order`.
    pub fn from_request(request: &ApiRequest) -> Result<Self, ApiResponse> {
        let q = request
            .query("q")
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| ApiResponse::validation_failed("Search", "q", "missing"))?;
        let ascending = match request.query("order") {
            None | Some("desc") => false,
            Some("asc") => true,
            Some(_) => return Err(ApiResponse::validation_failed("Search", "order", "invalid")),
        };
        if let Some(per_page) = request.query("per_page")
            && per_page
                .parse::<usize>()
                .map_or(true, |value| value == 0 || value > PER_PAGE_MAX)
        {
            return Err(ApiResponse::validation_failed(
                "Search", "per_page", "invalid",
            ));
        }
        let mut keywords = Vec::new();
        let mut qualifiers = Vec::new();
        for term in q.split_whitespace() {
            match term.split_once(':') {
                Some((name, value)) if !name.is_empty() && !value.is_empty() => {
                    qualifiers.push((name.to_ascii_lowercase(), value.to_owned()));
                }
                _ => keywords.push(term.to_lowercase()),
            }
        }
        Ok(Self {
            keywords,
            qualifiers,
            ascending,
        })
    }

    /// The values of every `name:` qualifier.
    pub fn values<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a str> {
        self.qualifiers
            .iter()
            .filter(move |(qualifier, _)| qualifier == name)
            .map(|(_, value)| value.as_str())
    }

    /// Whether every keyword occurs in one of `fields`, ignoring case.
    pub fn keywords_in(&self, fields: &[Option<&str>]) -> bool {
        let haystack = fields
            .iter()
            .flatten()
            .map(|field| field.to_lowercase())
            .collect::<Vec<_>>();
        self.keywords
            .iter()
            .all(|keyword| haystack.iter().any(|field| field.contains(keyword)))
    }

    /// Whether repeated `name:` qualifiers, which GitHub ORs, allow `value`.
    pub fn allows(&self, name: &str, value: &str) -> bool {
        let mut wanted = self.values(name).peekable();
        wanted.peek().is_none() || wanted.any(|wanted| wanted.eq_ignore_ascii_case(value))
    }
}

/// Orders `items` by `key`, descending unless the query asked for ascending.
fn sort_by<T, K: Ord>(items: &mut [T], query: &Query, key: impl Fn(&T) -> K) {
    items.sort_by(|left, right| {
        let ordered = key(left).cmp(&key(right));
        if query.ascending {
            ordered
        } else {
            ordered.reverse()
        }
    });
}

fn count(items: usize) -> i64 {
    i64::try_from(items).unwrap_or(i64::MAX)
}

fn user_item(request: &ApiRequest, account: &Account) -> types::UserSearchResultItem {
    let user = render::public_user(request, account);
    types::UserSearchResultItem {
        avatar_url: user.avatar_url,
        bio: user.bio,
        blog: user.blog,
        company: user.company,
        created_at: Some(user.created_at),
        email: user.email,
        events_url: user.events_url,
        followers: Some(user.followers),
        followers_url: user.followers_url,
        following: Some(user.following),
        following_url: user.following_url,
        gists_url: user.gists_url,
        gravatar_id: user.gravatar_id,
        hireable: user.hireable,
        html_url: user.html_url,
        id: user.id,
        location: user.location,
        login: user.login,
        name: user.name,
        node_id: user.node_id,
        organizations_url: user.organizations_url,
        public_gists: Some(user.public_gists),
        public_repos: Some(user.public_repos),
        received_events_url: user.received_events_url,
        repos_url: user.repos_url,
        score: 1.0,
        site_admin: user.site_admin,
        starred_url: user.starred_url,
        subscriptions_url: user.subscriptions_url,
        suspended_at: None,
        text_matches: None,
        type_: user.type_,
        updated_at: Some(user.updated_at),
        url: user.url,
        user_view_type: user.user_view_type,
    }
}

/// Whether `account` matches the keywords in the fields `in:` names: login, name and public
/// email by default.
/// Source: <https://docs.github.com/en/search-github/searching-on-github/searching-users>
fn user_matches(query: &Query, account: &Account) -> bool {
    let kind_ok = query
        .values("type")
        .all(|kind| match kind.to_ascii_lowercase().as_str() {
            "user" => account.kind == AccountKind::User,
            "org" => account.kind == AccountKind::Organization,
            _ => false,
        });
    let fields = query
        .values("in")
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>();
    let wants = |field: &str| fields.is_empty() || fields.iter().any(|wanted| wanted == field);
    let email = match account.kind {
        AccountKind::User => account.user_profile.email.as_deref(),
        AccountKind::Organization => account.profile.email.as_deref(),
    };
    kind_ok
        && query.allows("user", &account.login)
        && query.keywords_in(&[
            wants("login").then_some(account.login.as_str()),
            wants("name").then_some(account.name.as_deref()).flatten(),
            wants("email").then_some(email).flatten(),
        ])
}

impl Handler for ops::SearchUsers {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let query = Query::from_request(request)?;
        let state = lock(state);
        let mut accounts: Vec<&Account> = state
            .accounts()
            .filter(|account| user_matches(&query, account))
            .collect();
        match request.query("sort") {
            // Best match: the fake scores every result alike, so keep signup order.
            None => accounts.sort_by_key(|account| account.id),
            Some("followers") => sort_by(&mut accounts, &query, |a| a.followers.len()),
            Some("repositories") => sort_by(&mut accounts, &query, |a| a.repos().count()),
            Some("joined") => sort_by(&mut accounts, &query, |a| a.id),
            Some(_) => return Err(ApiResponse::validation_failed("Search", "sort", "invalid")),
        }
        let items = accounts
            .into_iter()
            .map(|account| user_item(request, account))
            .collect::<Vec<_>>();
        Ok(paginate_with(
            request,
            &request.path,
            items,
            |total, items| types::SearchUsersResponse {
                incomplete_results: false,
                items,
                total_count: total,
            },
        ))
    }
}

impl Handler for ops::SearchLabels {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let query = Query::from_request(request)?;
        let id: u64 = request
            .query("repository_id")
            .ok_or_else(|| ApiResponse::validation_failed("Search", "repository_id", "missing"))?
            .parse()
            .map_err(|_| ApiResponse::validation_failed("Search", "repository_id", "invalid"))?;
        if !matches!(request.query("sort"), None | Some("created" | "updated")) {
            return Err(ApiResponse::validation_failed("Search", "sort", "invalid"));
        }
        let state = lock(state);
        let found = state
            .accounts()
            .flat_map(|account| account.repos().map(move |repo| (account, repo)))
            .find(|(_, repo)| repo.id == id)
            .and_then(|(account, repo)| {
                let permission =
                    state.permission(request.caller.as_ref(), &account.login, &repo.name)?;
                Some(Found {
                    owner: account.login.clone(),
                    name: repo.name.clone(),
                    permission,
                })
            })
            .ok_or_else(ApiResponse::not_found)?;
        // Labels carry no creation time here, so `sort` keeps them by id.
        let mut labels = crate::handlers::issues::repository_labels(&state, request, &found)
            .into_iter()
            .filter(|label| query.keywords_in(&[Some(&label.name), label.description.as_deref()]))
            .collect::<Vec<_>>();
        sort_by(&mut labels, &query, |label| label.id);
        let items = labels
            .into_iter()
            .map(|label| types::LabelSearchResultItem {
                archived_at: label.archived_at,
                archived_by: label.archived_by,
                color: label.color,
                default: label.default,
                description: label.description,
                id: label.id,
                name: label.name,
                node_id: label.node_id,
                score: 1.0,
                text_matches: None,
                url: label.url,
            })
            .collect::<Vec<_>>();
        Ok(paginate_with(
            request,
            &request.path,
            items,
            |total, items| types::SearchLabelsResponse {
                incomplete_results: false,
                items,
                total_count: total,
            },
        ))
    }
}

/// GitHub searches its curated topic catalog, which this fake does not have: every search
/// is valid and finds nothing.
/// Source: <https://docs.github.com/en/rest/search/search#search-topics>
impl Handler for ops::SearchTopics {
    fn handle(_state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        Query::from_request(request)?;
        let items: Vec<types::TopicSearchResultItem> = Vec::new();
        Ok(ApiResponse::typed(
            200,
            &types::SearchTopicsResponse {
                incomplete_results: false,
                total_count: count(items.len()),
                items,
            },
        ))
    }
}
