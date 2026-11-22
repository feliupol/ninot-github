//! Query root fields and repository objects.

use async_graphql::Error;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use winnow::Parser;

use super::objects::{self, Connection, GitObject, PageInfo, Ref};
use super::types::{
    RepositoryOrderField, RepositoryPermission, RepositoryPrivacy, RepositoryVisibility,
};
use super::{
    AccountKey, Env, IdArgs, LoginArgs, Map, MutationArgs, NameArgs, Output, RepositoriesArgs,
    RepositoryArgs, RepositoryKey, database_id,
};
use crate::handlers::repos::init_storage;
use crate::render::node_id;
use crate::state::{Account, AccountKind, Repo, State, lock};
use ninot_common::git;

pub(super) const RESOLVERS: &[(&str, super::Resolver)] = &[
    ("Query.viewer", viewer),
    ("Query.node", query_node),
    ("Query.repository", query_repository),
    ("Query.repositoryOwner", repository_owner),
    ("Query.user", user),
    ("Query.organization", organization),
    ("Mutation.createRepository", create_repository),
    ("User.repositories", repositories),
    ("Organization.repositories", repositories),
    ("User.repository", owner_repository),
    ("Organization.repository", owner_repository),
    ("Repository.isEmpty", is_empty),
    ("Repository.defaultBranchRef", default_branch_ref),
    ("Repository.viewerPermission", viewer_permission),
    ("Repository.autoMergeAllowed", auto_merge_allowed),
];

pub(super) fn account_object(env: &Env, account: &Account) -> objects::Account {
    let typename = match account.kind {
        AccountKind::User => "User",
        AccountKind::Organization => "Organization",
    };
    objects::Account {
        typename,
        id: node_id(typename, account.id),
        database_id: account.id,
        login: account.login.clone(),
        name: None,
        url: env.urls().user(&account.login).html,
        created_at: super::read::account_created_at(account),
    }
}

/// A `Repository` object; fields that need git or permissions have resolvers.
pub(super) fn repository_object(env: &Env, account: &Account, repo: &Repo) -> objects::Repository {
    let urls = env.urls().repo(&account.login, &repo.name);
    let (created_at, updated_at, pushed_at) = super::read::repo_times(repo);
    objects::Repository {
        typename: "Repository",
        id: node_id("Repository", repo.id),
        database_id: repo.id,
        name: repo.name.clone(),
        name_with_owner: format!("{}/{}", account.login, repo.name),
        owner: account_object(env, account),
        description: repo.description.clone(),
        ssh_url: urls.ssh_url(),
        url: urls.html,
        is_private: repo.private,
        visibility: if repo.private {
            RepositoryVisibility::Private
        } else {
            RepositoryVisibility::Public
        },
        is_fork: false,
        is_archived: repo.settings.archived,
        is_template: repo.settings.is_template,
        is_mirror: false,
        parent: None,
        created_at,
        updated_at,
        pushed_at,
        has_issues_enabled: repo.settings.has_issues,
        has_wiki_enabled: repo.settings.has_wiki,
        merge_commit_allowed: repo.settings.allow_merge_commit,
        rebase_merge_allowed: repo.settings.allow_rebase_merge,
        squash_merge_allowed: repo.settings.allow_squash_merge,
    }
}

/// The repository object if the caller can see it.
pub(super) fn visible_repository(
    env: &Env,
    state: &State,
    owner: &str,
    name: &str,
) -> Option<objects::Repository> {
    state.permission(Some(&env.caller), owner, name)?;
    let account = state.account(owner)?;
    Some(repository_object(env, account, account.repo(name)?))
}

/// The account object with `login`, if it is of `kind`.
pub(super) fn account_of_kind(
    env: &Env,
    login: &str,
    kind: AccountKind,
) -> Option<objects::Account> {
    let state = lock(&env.state);
    state
        .account(login)
        .filter(|account| account.kind == kind)
        .map(|account| account_object(env, account))
}

/// `Query.viewer`.
pub(super) fn viewer(env: &Env, _: &Map, _: &Map) -> Result<Output, Error> {
    let viewer = account_of_kind(env, &env.caller.login, AccountKind::User)
        .ok_or_else(super::errors::viewer_missing)?;
    Output::of(viewer)
}

/// `Query.repository`, following renames and transfers.
pub(super) fn query_repository(env: &Env, _: &Map, args: &Map) -> Result<Output, Error> {
    let RepositoryArgs { owner, name } = super::read(args)?;
    let state = lock(&env.state);
    let (owner, name) = match state.redirect(&owner, &name) {
        Some((to_owner, to_name)) if super::read::repo(&state, &owner, &name).is_none() => {
            (to_owner.clone(), to_name.clone())
        }
        _ => (owner, name),
    };
    // Source: cli/cli v2.101.0 api/queries_repo.go:309
    let repository = visible_repository(env, &state, &owner, &name)
        .ok_or_else(|| super::errors::resolve_repository_name(&owner, &name))?;
    Output::of(repository)
}

/// `Query.repositoryOwner`.
pub(super) fn repository_owner(env: &Env, _: &Map, args: &Map) -> Result<Output, Error> {
    let LoginArgs { login } = super::read(args)?;
    let state = lock(&env.state);
    Output::of(
        state
            .account(&login)
            .map(|account| account_object(env, account)),
    )
}

/// `Query.user`.
pub(super) fn user(env: &Env, _: &Map, args: &Map) -> Result<Output, Error> {
    let LoginArgs { login } = super::read(args)?;
    // Source: cli/cli v2.101.0 pkg/cmd/project/shared/queries/queries.go:1270
    let user = account_of_kind(env, &login, AccountKind::User)
        .ok_or_else(|| super::errors::resolve_user_login(&login))?;
    Output::of(user)
}

/// `Query.organization`.
pub(super) fn organization(env: &Env, _: &Map, args: &Map) -> Result<Output, Error> {
    let LoginArgs { login } = super::read(args)?;
    // Source: cli/cli v2.101.0 api/queries_repo.go:25
    // Source: cli/cli v2.101.0 pkg/cmd/project/shared/queries/queries.go:1270
    let organization = account_of_kind(env, &login, AccountKind::Organization)
        .ok_or_else(|| super::errors::resolve_org_login(&login))?;
    Output::of(organization)
}

/// `User.repository` and `Organization.repository`.
pub(super) fn owner_repository(env: &Env, parent: &Map, args: &Map) -> Result<Output, Error> {
    let AccountKey { login } = super::read(parent)?;
    let NameArgs { name } = super::read(args)?;
    let state = lock(&env.state);
    Output::of(visible_repository(env, &state, &login, &name))
}

/// `User.repositories` and `Organization.repositories`: the owner's repositories visible to the
/// caller, filtered by `privacy` and `isFork`, ordered by `orderBy` and paginated with opaque
/// cursors.
pub(super) fn repositories(env: &Env, parent: &Map, args: &Map) -> Result<Output, Error> {
    let AccountKey { login } = super::read(parent)?;
    let args: RepositoriesArgs = super::read(args)?;
    let state = lock(&env.state);
    let mut repos: Vec<&Repo> = state
        .account(&login)
        .map(|account| {
            account
                .repos()
                .filter(|repo| {
                    state
                        .permission(Some(&env.caller), &login, &repo.name)
                        .is_some()
                })
                .collect()
        })
        .unwrap_or_default();
    match args.privacy {
        Some(RepositoryPrivacy::Public) => repos.retain(|repo| !repo.private),
        Some(RepositoryPrivacy::Private) => repos.retain(|repo| repo.private),
        None => {}
    }
    if args.is_fork == Some(true) {
        repos.clear();
    }
    if let Some(order) = args.order_by {
        match order.field {
            RepositoryOrderField::Name => repos.sort_by_key(|repo| repo.name.to_lowercase()),
            RepositoryOrderField::CreatedAt => {
                repos.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)))
            }
            RepositoryOrderField::UpdatedAt => {
                repos.sort_by(|a, b| a.updated_at.cmp(&b.updated_at).then(a.id.cmp(&b.id)))
            }
            _ => repos.sort_by(|a, b| {
                super::read::repo_pushed_or_created(a)
                    .cmp(&super::read::repo_pushed_or_created(b))
                    .then(a.id.cmp(&b.id))
            }),
        }
        if order.direction == super::types::OrderDirection::Desc {
            repos.reverse();
        }
    }
    let total = repos.len();
    let start = args
        .after
        .and_then(|cursor| STANDARD.decode(cursor).ok())
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .and_then(|text| super::grammar::cursor.parse(text.as_str()).ok())
        .map_or(0, |index| index + 1);
    let count = args.first.unwrap_or(30).clamp(0, 100) as usize;
    let end = (start + count).min(total);
    let cursor = |index: usize| STANDARD.encode(format!("cursor:{index}"));
    let account = state.account(&login);
    let nodes = repos
        .get(start..end)
        .unwrap_or_default()
        .iter()
        .filter_map(|repo| Some(repository_object(env, account?, repo)))
        .collect();
    Output::of(Connection {
        nodes,
        total_count: total,
        page_info: PageInfo {
            has_next_page: end < total,
            has_previous_page: start > 0,
            end_cursor: (end > start).then(|| cursor(end - 1)),
            start_cursor: (end > start).then(|| cursor(start)),
        },
    })
}

/// The stored repository behind a repository object.
pub(super) fn stored_repo<'a>(
    state: &'a State,
    repository: &Map,
) -> Result<Option<&'a Repo>, Error> {
    let RepositoryKey { owner, name } = super::read(repository)?;
    Ok(super::read::repo(state, &owner.login, &name))
}

/// `Repository.isEmpty`.
pub(super) fn is_empty(env: &Env, parent: &Map, _: &Map) -> Result<Output, Error> {
    let git_dir = stored_repo(&lock(&env.state), parent)?.and_then(|repo| repo.git_dir.clone());
    Output::of(git_dir.is_none_or(|git_dir| git::is_empty(&git_dir).unwrap_or(true)))
}

/// `Repository.defaultBranchRef`: null until the default branch exists.
pub(super) fn default_branch_ref(env: &Env, parent: &Map, _: &Map) -> Result<Output, Error> {
    let Some((git_dir, default_branch)) = stored_repo(&lock(&env.state), parent)?
        .and_then(|repo| Some((repo.git_dir.clone()?, repo.default_branch.clone())))
    else {
        return Output::of(None::<Ref>);
    };
    let name = format!("refs/heads/{default_branch}");
    let exists = git::refs(&git_dir, &name)
        .is_ok_and(|refs| refs.into_iter().any(|found| found.name == name));
    let oid = git::refs(&git_dir, &name)
        .ok()
        .and_then(|refs| refs.into_iter().find(|found| found.name == name))
        .map(|found| found.sha);
    Output::of(exists.then_some(Ref {
        typename: "Ref",
        name: default_branch,
        prefix: "refs/heads/",
        target: oid.map(|oid| GitObject {
            typename: "Commit",
            oid,
        }),
        repository: None,
        branch_protection_rule: None,
    }))
}

/// `Repository.viewerPermission`.
pub(super) fn viewer_permission(env: &Env, parent: &Map, _: &Map) -> Result<Output, Error> {
    let RepositoryKey { owner, name } = super::read(parent)?;
    let state = lock(&env.state);
    let permission = state.permission(Some(&env.caller), &owner.login, &name);
    Output::of(permission.map(|permission| {
        if permission.admin {
            RepositoryPermission::Admin
        } else if permission.push {
            RepositoryPermission::Write
        } else {
            RepositoryPermission::Read
        }
    }))
}

/// `Repository.autoMergeAllowed`.
pub(super) fn auto_merge_allowed(env: &Env, parent: &Map, _: &Map) -> Result<Output, Error> {
    let allowed = stored_repo(&lock(&env.state), parent)?
        .ok_or_else(super::errors::repository_missing)?
        .settings
        .allow_auto_merge;
    Output::of(allowed)
}

/// The payload of `Mutation.createRepository`.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateRepositoryPayload {
    #[serde(rename = "__typename")]
    typename: &'static str,
    repository: objects::Repository,
    client_mutation_id: Option<String>,
}

/// `Mutation.createRepository`, with GitHub's scope and permission checks.
pub(super) fn create_repository(env: &Env, _: &Map, args: &Map) -> Result<Output, Error> {
    let MutationArgs { input } =
        super::read::<MutationArgs<super::types::CreateRepositoryInput>>(args)?;
    let name = input.name.as_str();
    let private = input.visibility != RepositoryVisibility::Public;
    let scopes: &[&str] = if private {
        &["repo"]
    } else {
        &["repo", "public_repo"]
    };
    if !scopes.iter().any(|scope| env.caller.has_scope(scope)) {
        return Err(super::errors::insufficient_scopes());
    }
    let mut state = lock(&env.state);
    let owner = match &input.owner_id {
        None => env.caller.login.clone(),
        Some(owner_id) => {
            let unresolved = || super::errors::resolve_node(owner_id);
            let id = database_id(owner_id).ok_or_else(unresolved)?;
            state
                .accounts()
                .find(|account| account.id == id)
                .map(|account| account.login.clone())
                .ok_or_else(unresolved)?
        }
    };
    if !state
        .account(&owner)
        .is_some_and(|account| account.is_administered_by(&env.caller.login))
    {
        return Err(super::errors::create_repository_permission(
            &env.caller.login,
        ));
    }
    let repo = state
        .create_repo(&owner, name, private, input.description.clone())
        .map_err(|_| super::errors::name_exists())?;
    let storage = repo
        .git_dir
        .clone()
        .map(|git_dir| (git_dir, repo.default_branch.clone()));
    drop(state);
    if let Some((git_dir, branch)) = &storage {
        init_storage(&env.state, &owner, name, git_dir, branch, false)
            .map_err(|error| Error::new(error.to_string()))?;
    }
    let state = lock(&env.state);
    let repository = visible_repository(env, &state, &owner, name)
        .expect("the creator can see the repository it created");
    Output::of(CreateRepositoryPayload {
        typename: "CreateRepositoryPayload",
        repository,
        client_mutation_id: input.client_mutation_id,
    })
}

#[derive(serde::Serialize)]
#[serde(untagged)]
enum Node {
    PullRequest(Box<objects::PullRequest>),
    ProjectV2(Box<super::projects::ProjectV2>),
    Issue(Box<objects::Issue>),
}

/// Resolves a node id against persisted pull requests, issues and Projects v2 boards.
pub(super) fn query_node(env: &Env, _: &Map, args: &Map) -> Result<Output, Error> {
    let IdArgs { id } = super::read(args)?;
    let state = lock(&env.state);
    if let Some((owner, name, number)) = super::issues::issue_location(&state, &id)
        && let Some(account) = state.account(&owner)
        && let Some(repo) = account.repo(&name)
        && let Some(issue) = repo.issues.get(&number)
    {
        return Output::of(Node::Issue(Box::new(super::issue_view::issue_from_stored(
            env, &state, account, repo, issue,
        ))));
    }
    for account in state.accounts() {
        for repo in account.repos() {
            for (number, pull) in &repo.pull_requests {
                let expected = node_id("PullRequest", super::pulls::pull_id(repo.id, *number));
                if id == expected || pull.data.as_ref().is_some_and(|data| data.node_id == id) {
                    let pull = super::pulls::pull_object(env, account, repo, *number, pull)?;
                    return Output::of(pull.map(|pull| Node::PullRequest(Box::new(pull))));
                }
            }
        }
    }
    if let Some(id) = database_id(&id)
        && let Some(project) = state.projects.get(&id)
    {
        return Output::of(Node::ProjectV2(Box::new(super::projects::project_object(
            env, &state, project,
        ))));
    }
    Output::of(None::<Node>)
}
