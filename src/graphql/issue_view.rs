//! GraphQL `Issue` objects built from stored REST issues.

use super::Env;
use super::objects::{
    Actor, Connection, Issue, IssueComment, Label, LinkedBranch, ReactionGroup, Ref,
    RepositoryName, SubIssuesSummary, TotalCount,
};
use super::query::repository_object;
use super::read;
use super::types::{
    CommentAuthorAssociation, IssueState, IssueStateReason, LockReason as GqlLock, ReactionContent,
};
use crate::handlers::issues::repository_labels;
use crate::handlers::repos::Found;
use crate::render::{self, node_id};
use crate::state::{Account, Issue as Stored, LockReason, Repo, State, StateReason};

/// The GraphQL `Issue` of a stored issue.
pub(super) fn issue_from_stored(
    env: &Env,
    state: &State,
    account: &Account,
    repo: &Repo,
    issue: &Stored,
) -> Issue {
    let request = env.request();
    let rendered = render::issue(
        render::IssueContext {
            state,
            request: &request,
            owner: &account.login,
            repo,
        },
        issue,
    );
    let labels = repo_labels(state, &request, account, repo)
        .into_iter()
        .filter(|label| issue.has_label(&label.name))
        .collect();
    let comments: Vec<IssueComment> = issue
        .comments
        .iter()
        .map(|comment| comment_object(env, state, account, repo, issue, comment))
        .collect();
    let assignees: Vec<Actor> = issue
        .assignees
        .iter()
        .filter_map(|login| actor_for(env, state, login.as_str()))
        .collect();
    let linked = issue
        .linked_branches
        .iter()
        .map(|branch| linked_branch_object(env, account, repo, branch))
        .collect();
    Issue {
        typename: "Issue",
        id: issue.node_id.clone(),
        database_id: Some(issue.number),
        number: issue.number,
        title: issue.title.clone(),
        body: issue.body.clone().unwrap_or_default(),
        state: if issue.is_open() {
            IssueState::Open
        } else {
            IssueState::Closed
        },
        state_reason: issue.state_reason.map(gql_state_reason),
        url: rendered.html_url.clone(),
        closed: !issue.is_open(),
        locked: issue.locked,
        active_lock_reason: issue.lock_reason.map(gql_lock_reason),
        is_pinned: issue.is_pinned,
        created_at: crate::time::format(&issue.created_at),
        updated_at: crate::time::format(&issue.updated_at),
        closed_at: issue.closed_at.as_ref().map(crate::time::format),
        author: actor_for(env, state, issue.author.as_str()),
        author_association: association(account, issue.author.as_str()),
        milestone: read::issue_milestone(&env.urls(), &account.login, repo, issue),
        repository: repository_object(env, account, repo),
        labels: Connection::complete(labels),
        comments: Connection::complete(comments),
        assignees: Connection::complete(assignees.clone()),
        assigned_actors: Connection::complete(assignees),
        reaction_groups: empty_reaction_groups(),
        project_cards: Connection::complete(Vec::new()),
        project_items: Connection::complete(Vec::new()),
        issue_type: None,
        parent: None,
        sub_issues: Connection::complete(Vec::new()),
        sub_issues_summary: SubIssuesSummary::empty(),
        blocked_by: Connection::complete(Vec::new()),
        blocking: Connection::complete(Vec::new()),
        closed_by_pull_requests_references: Connection::complete(Vec::new()),
        linked_branches: Connection::complete(linked),
    }
}

pub(super) fn gql_state_reason(reason: StateReason) -> IssueStateReason {
    match reason {
        StateReason::Completed => IssueStateReason::Completed,
        StateReason::NotPlanned => IssueStateReason::NotPlanned,
        StateReason::Duplicate => IssueStateReason::Duplicate,
        StateReason::Reopened => IssueStateReason::Reopened,
    }
}

pub(super) fn gql_lock_reason(reason: LockReason) -> GqlLock {
    match reason {
        LockReason::OffTopic => GqlLock::OffTopic,
        LockReason::TooHeated => GqlLock::TooHeated,
        LockReason::Resolved => GqlLock::Resolved,
        LockReason::Spam => GqlLock::Spam,
    }
}

pub(super) fn association(account: &Account, login: &str) -> CommentAuthorAssociation {
    if account.login.eq_ignore_ascii_case(login) {
        CommentAuthorAssociation::Owner
    } else if account.kind == crate::state::AccountKind::Organization
        && account.members.contains(crate::state::fold(login).as_ref())
    {
        CommentAuthorAssociation::Member
    } else {
        CommentAuthorAssociation::None
    }
}

pub(super) fn actor_for(_env: &Env, state: &State, login: &str) -> Option<Actor> {
    let account = state.account(login)?;
    Some(Actor {
        typename: if account.kind == crate::state::AccountKind::User {
            "User"
        } else {
            "Bot"
        },
        login: account.login.clone(),
        id: Some(node_id(
            if account.kind == crate::state::AccountKind::User {
                "User"
            } else {
                "Bot"
            },
            account.id,
        )),
        name: account.name.clone(),
        database_id: Some(account.id),
    })
}

pub(super) fn empty_reaction_groups() -> Vec<ReactionGroup> {
    [
        ReactionContent::ThumbsUp,
        ReactionContent::ThumbsDown,
        ReactionContent::Laugh,
        ReactionContent::Hooray,
        ReactionContent::Confused,
        ReactionContent::Heart,
        ReactionContent::Rocket,
        ReactionContent::Eyes,
    ]
    .into_iter()
    .map(|content| ReactionGroup {
        content,
        users: TotalCount { total_count: 0 },
    })
    .collect()
}

pub(super) fn comment_object(
    env: &Env,
    state: &State,
    account: &Account,
    repo: &Repo,
    issue: &Stored,
    comment: &crate::state::Comment,
) -> IssueComment {
    let request = env.request();
    let rendered = render::issue_comment(
        render::IssueContext {
            state,
            request: &request,
            owner: &account.login,
            repo,
        },
        issue.number,
        comment,
    );
    let author_login = comment.author.as_ref().map(|name| name.as_str());
    IssueComment {
        typename: "IssueComment",
        body: Some(comment.body.clone()),
        id: Some(rendered.node_id.clone()),
        url: Some(rendered.html_url.clone()),
        created_at: Some(crate::time::format(&comment.created_at)),
        author: author_login.and_then(|login| actor_for(env, state, login)),
        author_association: Some(
            author_login.map_or(CommentAuthorAssociation::None, |login| {
                association(account, login)
            }),
        ),
        includes_created_edit: Some(false),
        is_minimized: Some(false),
        minimized_reason: None,
        viewer_did_author: Some(
            author_login.is_some_and(|login| login.eq_ignore_ascii_case(&env.caller.login)),
        ),
        reaction_groups: Some(empty_reaction_groups()),
    }
}

pub(super) fn linked_branch_object(
    env: &Env,
    account: &Account,
    repo: &Repo,
    branch: &crate::state::LinkedBranch,
) -> LinkedBranch {
    LinkedBranch {
        typename: "LinkedBranch",
        id: node_id("LinkedBranch", branch.id),
        ref_: Ref {
            typename: "Ref",
            name: branch.name.clone(),
            prefix: "refs/heads/",
            target: None,
            repository: Some(RepositoryName {
                typename: "Repository",
                name_with_owner: format!("{}/{}", account.login, repo.name),
                url: Some(env.urls().repo(&account.login, &repo.name).html),
            }),
        },
    }
}

pub(super) fn repo_labels(
    state: &State,
    request: &crate::api::ApiRequest,
    account: &Account,
    repo: &Repo,
) -> Vec<Label> {
    let found = Found {
        owner: account.login.clone(),
        name: repo.name.clone(),
        permission: crate::state::Permission::ADMIN,
    };
    repository_labels(state, request, &found)
        .into_iter()
        .map(gql_label)
        .collect()
}

pub(super) fn gql_label(label: crate::types::Label) -> Label {
    Label {
        name: label.name,
        id: Some(label.node_id),
        color: Some(label.color),
        description: label.description,
        is_default: Some(label.default),
    }
}
