//! `Query.search`.

use async_graphql::Error;

use super::issue_view::issue_from_stored;
use super::objects::{SearchConnection, SearchResultItem};
use super::page::page;
use super::{Env, Map, Output, SearchArgs};
use crate::state::lock;

pub(super) const RESOLVERS: &[(&str, super::Resolver)] = &[("Query.search", search)];

/// `Query.search`, over pull requests and issues seeded in the real repository state.
fn search(env: &Env, _: &Map, args: &Map) -> Result<Output, Error> {
    let SearchArgs {
        query,
        first,
        last,
        after,
    } = super::read(args)?;
    let tokens = search_tokens(&query);
    let qualifiers: Vec<&str> = tokens.iter().map(String::as_str).collect();
    let repositories: Vec<&str> = qualifiers
        .iter()
        .filter_map(|part| part.strip_prefix("repo:"))
        .collect();
    let head = qualifiers
        .iter()
        .find_map(|part| part.strip_prefix("head:"));
    let labels: Vec<&str> = qualifiers
        .iter()
        .filter_map(|part| part.strip_prefix("label:"))
        .map(|label| label.trim_matches('"'))
        .collect();
    let assignee = qualifiers
        .iter()
        .find_map(|part| part.strip_prefix("assignee:"));
    let author = qualifiers
        .iter()
        .find_map(|part| part.strip_prefix("author:"))
        .filter(|value| *value != "@me");
    let author_me = qualifiers.contains(&"author:@me");
    let requested = qualifiers
        .iter()
        .find_map(|part| part.strip_prefix("review-requested:"));
    let milestone = qualifiers
        .iter()
        .find_map(|part| part.strip_prefix("milestone:"))
        .map(|value| value.trim_matches('"'));
    let want_open = qualifiers.contains(&"is:open") || qualifiers.contains(&"state:open");
    let want_closed = qualifiers.contains(&"is:closed") || qualifiers.contains(&"state:closed");
    let want_pr = qualifiers.contains(&"is:pr") || qualifiers.contains(&"type:pr");
    let want_issue = qualifiers.contains(&"is:issue") || qualifiers.contains(&"type:issue");
    let text_terms: Vec<&str> = qualifiers
        .iter()
        .copied()
        .filter(|term| !term.contains(':') && *term != "OR" && *term != "(" && *term != ")")
        .collect();
    let state = lock(&env.state);
    let mut nodes = Vec::new();
    for account in state.accounts() {
        for repo in account.repos() {
            let full_name = format!("{}/{}", account.login, repo.name);
            if !repositories.is_empty()
                && !repositories
                    .iter()
                    .any(|wanted| wanted.eq_ignore_ascii_case(&full_name))
                || state
                    .permission(Some(&env.caller), &account.login, &repo.name)
                    .is_none()
            {
                continue;
            }
            if !want_issue {
                for (number, pull) in &repo.pull_requests {
                    let Some(data) = pull.data.as_ref() else {
                        continue;
                    };
                    if want_open && data.state != crate::types::PullRequestState::Open {
                        continue;
                    }
                    if want_closed && data.state != crate::types::PullRequestState::Closed {
                        continue;
                    }
                    if head.is_some_and(|branch| {
                        !data.head.ref_.eq_ignore_ascii_case(branch)
                            && !data.head.label.eq_ignore_ascii_case(branch)
                    }) {
                        continue;
                    }
                    if !text_terms.iter().all(|term| {
                        data.title.to_lowercase().contains(&term.to_lowercase())
                            || data.body.as_deref().is_some_and(|body| {
                                body.to_lowercase().contains(&term.to_lowercase())
                            })
                    }) {
                        continue;
                    }
                    if author_me && !data.user.login.eq_ignore_ascii_case(&env.caller.login) {
                        continue;
                    }
                    if let Some(login) = requested {
                        let login = if login == "@me" {
                            env.caller.login.as_str()
                        } else {
                            login
                        };
                        if !crate::handlers::search::review_requested(account, data, login) {
                            continue;
                        }
                    }
                    if author.is_some_and(|login| !data.user.login.eq_ignore_ascii_case(login)) {
                        continue;
                    }
                    if !labels.iter().all(|name| {
                        data.labels
                            .iter()
                            .any(|label| label.name.eq_ignore_ascii_case(name))
                    }) {
                        continue;
                    }
                    let draft = data.draft.unwrap_or(false);
                    if (qualifiers.contains(&"draft:true") || qualifiers.contains(&"is:draft"))
                        && !draft
                    {
                        continue;
                    }
                    if qualifiers.contains(&"draft:false") && draft {
                        continue;
                    }
                    if qualifiers.iter().any(|part| part == &"is:merged") && !data.merged {
                        continue;
                    }
                    if let Some(base) = qualifiers
                        .iter()
                        .find_map(|part| part.strip_prefix("base:"))
                        && !data.base.ref_.eq_ignore_ascii_case(base)
                    {
                        continue;
                    }
                    if let Some(pull) =
                        super::pulls::pull_object(env, account, repo, *number, pull)?
                    {
                        nodes.push(SearchResultItem::PullRequest(Box::new(pull)));
                    }
                }
            }
            if !want_pr && requested.is_none() {
                for issue in repo.issues.values() {
                    if want_open && !issue.is_open() {
                        continue;
                    }
                    if want_closed && issue.is_open() {
                        continue;
                    }
                    if !text_terms.iter().all(|term| {
                        issue.title.to_lowercase().contains(&term.to_lowercase())
                            || issue.body.as_deref().is_some_and(|body| {
                                body.to_lowercase().contains(&term.to_lowercase())
                            })
                    }) {
                        continue;
                    }
                    if author_me && !issue.authored_by(&env.caller.login) {
                        continue;
                    }
                    if author.is_some_and(|login| !issue.authored_by(login)) {
                        continue;
                    }
                    if assignee.is_some_and(|login| !issue.is_assigned(login)) {
                        continue;
                    }
                    if !labels.iter().all(|name| issue.has_label(name)) {
                        continue;
                    }
                    if let Some(title) = milestone {
                        let matches = issue.milestone.and_then(|number| {
                            repo.milestones.get(&number).map(|m| m.title.as_str())
                        });
                        if !matches.is_some_and(|found| found.eq_ignore_ascii_case(title)) {
                            continue;
                        }
                    }
                    nodes.push(SearchResultItem::Issue(Box::new(issue_from_stored(
                        env, &state, account, repo, issue,
                    ))));
                }
            }
        }
    }
    let paged = page(nodes, first, last, after);
    Output::of(SearchConnection {
        issue_count: paged.total_count,
        nodes: paged.nodes,
        page_info: paged.page_info,
    })
}

/// Splits GitHub search syntax into words while retaining quoted exact phrases as one token.
fn search_tokens(query: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut token = String::new();
    let mut quoted = false;
    for character in query.chars() {
        match character {
            '"' => quoted = !quoted,
            character if character.is_whitespace() && !quoted => {
                if !token.is_empty() {
                    tokens.push(std::mem::take(&mut token));
                }
            }
            character => token.push(character),
        }
    }
    if !token.is_empty() {
        tokens.push(token);
    }
    tokens
}
