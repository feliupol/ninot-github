//! Issues: updates, labels, assignees, milestones, comments, locks, events and lists.
//!
//! Pull requests share the issue routes GitHub uses for labels, assignees, comments and locks.
//! Updating an issue edits stored issues only; pull-request edits stay on the pull-request
//! handler, which already serves `pulls/update`.

#![expect(
    clippy::disallowed_types,
    reason = "untyped JSON; remove when this module is typed"
)]

mod assignees;
mod comments;
mod create;
mod events;
mod label_repo;
mod labels;
mod lists;
mod locks;
mod milestones;
pub(crate) mod record;
mod update;

pub(crate) use lists::{for_repository, next_subject_number};

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::api::{ApiRequest, ApiResponse};
use crate::handlers::repos::Found;
use crate::render;
use crate::state::{Account, AccountKind, Issue, LabelRecord, Milestone, Name, Repo, State};
use crate::types;

/// Every label a repository currently has: stored labels, then labels that exist only as copies
/// on an issue or pull request. Stored labels win when both have the same name.
pub(crate) fn repository_labels(
    state: &State,
    request: &ApiRequest,
    found: &Found,
) -> Vec<types::Label> {
    catalog(state, request, found).into_values().collect()
}

/// Labels keyed by lowercase name. Copies keep the fields they were copied with.
fn catalog(state: &State, request: &ApiRequest, found: &Found) -> BTreeMap<String, types::Label> {
    let repo = found.repo(state);
    let mut labels = BTreeMap::new();
    for label in repo
        .pull_requests
        .values()
        .filter_map(|pull| pull.data.as_ref())
        .flat_map(|pull| pull.labels.iter().map(label_from_pull))
    {
        labels.entry(label_key(&label.name)).or_insert(label);
    }
    for label in repo.labels.values() {
        labels.insert(
            label_key(&label.name),
            render_label(state, request, found, label),
        );
    }
    labels
}

/// A stored label as the label endpoints return it.
pub(crate) fn render_label(
    state: &State,
    request: &ApiRequest,
    found: &Found,
    label: &LabelRecord,
) -> types::Label {
    render::label(state, request, &found.owner, &found.name, label)
}

pub(super) fn label_from_issue_item(label: &types::IssueLabelsItem) -> Option<types::Label> {
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

pub(super) fn label_from_pull(label: &types::PullRequestLabelsItem) -> types::Label {
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

fn label_key(name: &str) -> String {
    name.to_lowercase()
}

/// Stores `label` in the repository's label store unless a label with its name is stored, and
/// returns the stored spelling an issue references it by.
pub(crate) fn ensure_label(repo: &mut Repo, label: &types::Label) -> Name {
    let stored = repo
        .labels
        .entry(Name::new(label.name.clone()))
        .or_insert_with(|| label.clone().into());
    Name::new(stored.name.clone())
}

/// The repository label named `name`, ignoring case, storing it when it exists only as a copy
/// on a pull request; otherwise `fresh()`, stored as a new label. GitHub creates a label an issue
/// is given that the repository lacks.
pub(crate) fn resolve_issue_label(
    state: &mut State,
    request: &ApiRequest,
    found: &Found,
    name: &str,
    fresh: impl FnOnce() -> types::Label,
) -> types::Label {
    let label = catalog(state, request, found)
        .remove(&label_key(name))
        .unwrap_or_else(fresh);
    let stored = ensure_label(found.repo_mut(state), &label);
    render_label(
        state,
        request,
        found,
        &found.repo(state).labels[stored.folded()],
    )
}

/// The names an issue references `labels` by, storing each label that is not stored yet.
///
/// Empty, storing nothing, when `number` is a pull request, which keeps copies of its labels.
pub(crate) fn issue_label_names(
    repo: &mut Repo,
    number: u64,
    labels: &[types::Label],
) -> Vec<Name> {
    if repo.pull_requests.contains_key(&number) || !repo.issues.contains_key(&number) {
        return Vec::new();
    }
    labels
        .iter()
        .map(|label| ensure_label(repo, label))
        .collect()
}

/// What rendering an issue in the repository `find_repo` resolved reads.
pub(crate) fn context<'a>(
    state: &'a State,
    request: &'a ApiRequest,
    found: &'a Found,
) -> render::IssueContext<'a> {
    render::IssueContext {
        state,
        request,
        owner: &found.owner,
        repo: found.repo(state),
    }
}

/// The issue `number` as the REST API returns it, when it is stored.
pub(crate) fn rendered_issue(
    state: &State,
    request: &ApiRequest,
    found: &Found,
    number: u64,
) -> Option<types::Issue> {
    let context = context(state, request, found);
    context
        .repo
        .issues
        .get(&number)
        .map(|issue| render::issue(context, issue))
}

/// Percent-decodes a path parameter. Invalid `%` sequences are left in place.
fn decode_param(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let Ok(byte) = u8::from_str_radix(
                std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or(""),
                16,
            )
        {
            decoded.push(byte);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

fn issue_number(request: &ApiRequest) -> Result<u64, ApiResponse> {
    request
        .param("issue_number")
        .parse()
        .map_err(|_| ApiResponse::not_found())
}

fn milestone_number(request: &ApiRequest) -> Result<u64, ApiResponse> {
    request
        .param("milestone_number")
        .parse()
        .map_err(|_| ApiResponse::not_found())
}

fn comment_id(request: &ApiRequest) -> Result<i64, ApiResponse> {
    request
        .param("comment_id")
        .parse()
        .map_err(|_| ApiResponse::not_found())
}

fn require_push(found: &Found) -> Result<(), ApiResponse> {
    if found.permission.push {
        Ok(())
    } else {
        Err(ApiResponse::error(
            403,
            "Must have push access to this repository",
        ))
    }
}

fn caller_user(
    state: &State,
    request: &ApiRequest,
    login: &str,
) -> Result<types::SimpleUser, ApiResponse> {
    state
        .account(login)
        .filter(|account| account.kind == AccountKind::User)
        .map(|account| render::simple_user(request, account))
        .ok_or_else(ApiResponse::not_found)
}

/// Users who can be assigned issues in `owner`'s repository: the owning user, or the
/// organization's members. Public read access is not enough, matching GitHub's collaborator rule.
fn assignable<'a>(state: &'a State, owner: &str) -> Vec<&'a Account> {
    let Some(owner_account) = state.account(owner) else {
        return Vec::new();
    };
    let kind = owner_account.kind;
    let logins = match kind {
        AccountKind::User => vec![owner_account.login.clone()],
        AccountKind::Organization => owner_account.members.iter().cloned().collect(),
    };
    let mut users: Vec<&Account> = logins
        .iter()
        .filter_map(|login| {
            state
                .account(login)
                .filter(|account| account.kind == AccountKind::User)
        })
        .collect();
    users.sort_by(|left, right| {
        left.login
            .to_lowercase()
            .cmp(&right.login.to_lowercase())
            .then(left.login.cmp(&right.login))
    });
    users.dedup_by(|left, right| left.login.eq_ignore_ascii_case(&right.login));
    users
}

fn can_be_assigned(state: &State, owner: &str, login: &str) -> bool {
    assignable(state, owner)
        .iter()
        .any(|account| account.login.eq_ignore_ascii_case(login))
}

fn json_body(request: &ApiRequest) -> Result<Value, ApiResponse> {
    if request.body.is_empty() {
        return Ok(Value::Object(serde_json::Map::new()));
    }
    // Source: <https://docs.github.com/en/rest/using-the-rest-api/troubleshooting-the-rest-api#problems-parsing-json>
    serde_json::from_slice(&request.body)
        .map_err(|_| ApiResponse::error(400, "Problems parsing JSON"))
}

fn object_body(request: &ApiRequest) -> Result<serde_json::Map<String, Value>, ApiResponse> {
    match json_body(request)? {
        Value::Object(object) => Ok(object),
        _ => Err(ApiResponse::unprocessable("Invalid request body")),
    }
}

/// Rewrites every milestone embedded in a pull request from the repository store and refreshes
/// its counts. Issues reference milestones by number; one whose milestone is no longer stored
/// loses it.
fn sync_milestones(repo: &mut Repo, request: &ApiRequest, owner: &str) {
    let rendered: BTreeMap<i64, types::NullableMilestone> = repo
        .milestones
        .values()
        .map(|milestone| {
            (
                render::id(milestone.number),
                render::stored_milestone(request, owner, repo, milestone),
            )
        })
        .collect();
    for issue in repo.issues.values_mut() {
        if issue
            .milestone
            .is_some_and(|number| !repo.milestones.contains_key(&number))
        {
            issue.milestone = None;
        }
    }
    for pull in repo
        .pull_requests
        .values_mut()
        .filter_map(|pull| pull.data.as_mut())
    {
        let number = pull.milestone.as_ref().map(|milestone| milestone.number);
        if let Some(number) = number {
            pull.milestone = rendered.get(&number).cloned();
        }
    }
}

fn render_stored(
    repo: &Repo,
    request: &ApiRequest,
    owner: &str,
    milestone: &Milestone,
) -> types::NullableMilestone {
    render::stored_milestone(request, owner, repo, milestone)
}

fn author_login(issue: &types::Issue) -> Option<&str> {
    issue.user.as_ref().map(|user| user.login.as_str())
}

fn is_assigned(issue: &types::Issue, login: &str) -> bool {
    issue
        .assignees
        .iter()
        .any(|user| user.login.eq_ignore_ascii_case(login))
        || issue
            .assignee
            .as_ref()
            .is_some_and(|user| user.login.eq_ignore_ascii_case(login))
}

fn label_names(issue: &types::Issue) -> Vec<String> {
    issue
        .labels
        .iter()
        .filter_map(label_from_issue_item)
        .map(|label| label.name)
        .collect()
}

/// Whether `issue` mentions `@login` as a whole token in its title or body.
fn mentions(issue: &types::Issue, login: &str) -> bool {
    let needle = format!("@{}", login.to_ascii_lowercase());
    let text =
        format!("{} {}", issue.title, issue.body.as_deref().unwrap_or("")).to_ascii_lowercase();
    let mut rest = text.as_str();
    while let Some(index) = rest.find(&needle) {
        let after = index + needle.len();
        let before_ok = index == 0
            || rest[..index]
                .chars()
                .next_back()
                .is_some_and(|character| !character.is_ascii_alphanumeric());
        let after_ok = rest[after..]
            .chars()
            .next()
            .is_none_or(|character| !character.is_ascii_alphanumeric() && character != '-');
        if before_ok && after_ok {
            return true;
        }
        rest = &rest[after..];
    }
    false
}

fn parse_timestamp(value: &str, resource: &str, field: &str) -> Result<DateTime<Utc>, ApiResponse> {
    crate::time::parse(value)
        .map_err(|_| ApiResponse::validation_failed(resource, field, "invalid"))
}

fn query_choice<'a>(
    request: &'a ApiRequest,
    resource: &str,
    name: &str,
    allowed: &[&str],
    default: &'a str,
) -> Result<&'a str, ApiResponse> {
    match request.query(name) {
        None => Ok(default),
        Some(value) if allowed.contains(&value) => Ok(value),
        Some(_) => Err(ApiResponse::validation_failed(resource, name, "invalid")),
    }
}

/// Runs `on_issue` or `on_pull` for `number`, preferring a pull request when both exist.
///
/// A pull request whose REST model was never seeded is a 500. A missing number is a 404.
fn with_subject<T>(
    repo: &mut Repo,
    number: u64,
    on_issue: impl FnOnce(&mut Issue) -> Result<T, ApiResponse>,
    on_pull: impl FnOnce(&mut types::PullRequest) -> Result<T, ApiResponse>,
) -> Result<T, ApiResponse> {
    if repo.pull_requests.contains_key(&number) {
        let pull = repo
            .pull_requests
            .get_mut(&number)
            .and_then(|pull| pull.data.as_mut())
            .ok_or_else(|| ApiResponse::internal("Pull request REST model is not seeded"))?;
        return on_pull(pull);
    }
    if let Some(issue) = repo.issues.get_mut(&number) {
        return on_issue(issue);
    }
    Err(ApiResponse::not_found())
}

fn webhook_ctx<'a>(
    state: &'a mut State,
    request: &'a ApiRequest,
    found: &'a Found,
) -> crate::webhooks::events::Ctx<'a> {
    crate::webhooks::events::Ctx::new(state, request, &found.owner, &found.name)
}

fn stored_pull(repo: &Repo, number: u64) -> Option<types::PullRequest> {
    repo.pull_requests
        .get(&number)
        .and_then(|pull| pull.data.clone())
}

fn query_bool(request: &ApiRequest, name: &str) -> Result<Option<bool>, ApiResponse> {
    match request.query(name) {
        None => Ok(None),
        Some(value) if value.eq_ignore_ascii_case("true") || value == "1" => Ok(Some(true)),
        Some(value) if value.eq_ignore_ascii_case("false") || value == "0" => Ok(Some(false)),
        Some(_) => Err(ApiResponse::validation_failed("Issue", name, "invalid")),
    }
}
