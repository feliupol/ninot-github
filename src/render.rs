//! Builds the spec's response types from state objects.

#![expect(
    clippy::disallowed_types,
    reason = "untyped JSON; remove when this module is typed"
)]

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use chrono::{DateTime, Utc};
use url::Url;

use crate::api::ApiRequest;
use crate::state::{
    Account, AccountKind, Comment, Issue, LabelRecord, Milestone, Permission, Repo, State,
    StateReason, fold,
};
use crate::types;
use crate::webhook_types;

/// GitHub's opaque global node id.
pub fn node_id(kind: &str, id: u64) -> String {
    STANDARD.encode(format!("0{}:{kind}{id}", kind.len()))
}

/// The host of a web URL, with its port when it has one, for a repository's `git@host:` clone
/// URL. A `web_url` that is not a URL at all stands in for itself.
pub fn web_host(web_url: &str) -> String {
    let Ok(url) = Url::parse(web_url) else {
        return web_url.to_owned();
    };
    match (url.host_str(), url.port()) {
        (Some(host), Some(port)) => format!("{host}:{port}"),
        (Some(host), None) => host.to_owned(),
        (None, _) => web_url.to_owned(),
    }
}

/// Parses a timestamp the fake recorded, such as `2026-09-29T20:38:27Z`, or one git reported
/// with an offset.
pub fn timestamp(text: &str) -> DateTime<Utc> {
    crate::time::parse(text).unwrap_or_else(|error| panic!("invalid timestamp {text}: {error}"))
}

/// An id as the spec types it.
pub fn id(id: u64) -> i64 {
    i64::try_from(id).expect("ids fit in i64")
}

/// Projects `FullRepository` into the fields of `MinimalRepository`.
impl From<types::FullRepository> for types::MinimalRepository {
    fn from(value: types::FullRepository) -> Self {
        Self {
            allow_forking: value.allow_forking,
            archive_url: value.archive_url,
            archived: Some(value.archived),
            assignees_url: value.assignees_url,
            blobs_url: value.blobs_url,
            branches_url: value.branches_url,
            clone_url: Some(value.clone_url),
            code_of_conduct: value.code_of_conduct.map(|value| value.into()),
            collaborators_url: value.collaborators_url,
            comments_url: value.comments_url,
            commits_url: value.commits_url,
            compare_url: value.compare_url,
            contents_url: value.contents_url,
            contributors_url: value.contributors_url,
            created_at: Some(value.created_at),
            custom_properties: value.custom_properties,
            default_branch: Some(value.default_branch),
            delete_branch_on_merge: value.delete_branch_on_merge,
            deployments_url: value.deployments_url,
            description: value.description,
            disabled: Some(value.disabled),
            downloads_url: value.downloads_url,
            events_url: value.events_url,
            fork: value.fork,
            forks: Some(value.forks),
            forks_count: Some(value.forks_count),
            forks_url: value.forks_url,
            full_name: value.full_name,
            git_commits_url: value.git_commits_url,
            git_refs_url: value.git_refs_url,
            git_tags_url: value.git_tags_url,
            git_url: Some(value.git_url),
            has_discussions: Some(value.has_discussions),
            has_downloads: value.has_downloads,
            has_issues: Some(value.has_issues),
            has_pages: Some(value.has_pages),
            has_projects: Some(value.has_projects),
            has_pull_requests: value.has_pull_requests,
            has_wiki: Some(value.has_wiki),
            homepage: value.homepage,
            hooks_url: value.hooks_url,
            html_url: value.html_url,
            id: value.id,
            is_template: value.is_template,
            issue_comment_url: value.issue_comment_url,
            issue_events_url: value.issue_events_url,
            issues_url: value.issues_url,
            keys_url: value.keys_url,
            labels_url: value.labels_url,
            language: value.language,
            languages_url: value.languages_url,
            license: value.license.map(|value| value.into()),
            merges_url: value.merges_url,
            milestones_url: value.milestones_url,
            mirror_url: value.mirror_url,
            name: value.name,
            network_count: Some(value.network_count),
            node_id: value.node_id,
            notifications_url: value.notifications_url,
            open_issues: Some(value.open_issues),
            open_issues_count: Some(value.open_issues_count),
            owner: value.owner,
            permissions: value.permissions.map(|value| value.into()),
            private: value.private,
            pull_request_creation_policy: value
                .pull_request_creation_policy
                .map(|value| value.into()),
            pulls_url: value.pulls_url,
            pushed_at: Some(value.pushed_at),
            releases_url: value.releases_url,
            role_name: None,
            security_and_analysis: value.security_and_analysis,
            size: Some(value.size),
            ssh_url: Some(value.ssh_url),
            stargazers_count: Some(value.stargazers_count),
            stargazers_url: value.stargazers_url,
            statuses_url: value.statuses_url,
            subscribers_count: Some(value.subscribers_count),
            subscribers_url: value.subscribers_url,
            subscription_url: value.subscription_url,
            svn_url: Some(value.svn_url),
            tags_url: value.tags_url,
            teams_url: value.teams_url,
            temp_clone_token: value.temp_clone_token,
            topics: value.topics,
            trees_url: value.trees_url,
            updated_at: Some(value.updated_at),
            url: value.url,
            visibility: value.visibility,
            watchers: Some(value.watchers),
            watchers_count: Some(value.watchers_count),
            web_commit_signoff_required: value.web_commit_signoff_required,
        }
    }
}

/// Projects `CodeOfConductSimple` into the fields of `CodeOfConduct`.
impl From<types::CodeOfConductSimple> for types::CodeOfConduct {
    fn from(value: types::CodeOfConductSimple) -> Self {
        Self {
            body: None,
            html_url: value.html_url,
            key: value.key,
            name: value.name,
            url: value.url,
        }
    }
}

/// Projects `NullableLicenseSimple` into the fields of `MinimalRepositoryLicense`.
impl From<types::NullableLicenseSimple> for types::MinimalRepositoryLicense {
    fn from(value: types::NullableLicenseSimple) -> Self {
        Self {
            key: Some(value.key),
            name: Some(value.name),
            node_id: Some(value.node_id),
            spdx_id: value.spdx_id,
            url: value.url,
        }
    }
}

/// Projects `FullRepositoryPermissions` into the fields of `MinimalRepositoryPermissions`.
impl From<types::FullRepositoryPermissions> for types::MinimalRepositoryPermissions {
    fn from(value: types::FullRepositoryPermissions) -> Self {
        Self {
            admin: Some(value.admin),
            maintain: value.maintain,
            pull: Some(value.pull),
            push: Some(value.push),
            triage: value.triage,
        }
    }
}

/// Projects `FullRepositoryPullRequestCreationPolicy` into the fields of `MinimalRepositoryPullRequestCreationPolicy`.
impl From<types::FullRepositoryPullRequestCreationPolicy>
    for types::MinimalRepositoryPullRequestCreationPolicy
{
    fn from(value: types::FullRepositoryPullRequestCreationPolicy) -> Self {
        match value {
            types::FullRepositoryPullRequestCreationPolicy::All => Self::All,
            types::FullRepositoryPullRequestCreationPolicy::CollaboratorsOnly => {
                Self::CollaboratorsOnly
            }
        }
    }
}

/// Projects `FullRepository` into the fields of `Repository`.
impl From<types::FullRepository> for types::Repository {
    fn from(value: types::FullRepository) -> Self {
        Self {
            allow_auto_merge: value
                .allow_auto_merge
                .expect("full repository supplies allow_auto_merge"),
            allow_forking: value.allow_forking,
            allow_merge_commit: value
                .allow_merge_commit
                .expect("full repository supplies allow_merge_commit"),
            allow_rebase_merge: value
                .allow_rebase_merge
                .expect("full repository supplies allow_rebase_merge"),
            allow_squash_merge: value
                .allow_squash_merge
                .expect("full repository supplies allow_squash_merge"),
            allow_update_branch: value
                .allow_update_branch
                .expect("full repository supplies allow_update_branch"),
            anonymous_access_enabled: Some(value.anonymous_access_enabled),
            archive_url: value.archive_url,
            archived: value.archived,
            assignees_url: value.assignees_url,
            blobs_url: value.blobs_url,
            branches_url: value.branches_url,
            clone_url: value.clone_url,
            code_search_index_status: None,
            collaborators_url: value.collaborators_url,
            comments_url: value.comments_url,
            commits_url: value.commits_url,
            compare_url: value.compare_url,
            contents_url: value.contents_url,
            contributors_url: value.contributors_url,
            created_at: Some(value.created_at),
            default_branch: value.default_branch,
            delete_branch_on_merge: value
                .delete_branch_on_merge
                .expect("full repository supplies delete_branch_on_merge"),
            deployments_url: value.deployments_url,
            description: value.description,
            disabled: value.disabled,
            downloads_url: value.downloads_url,
            events_url: value.events_url,
            fork: value.fork,
            forks: value.forks,
            forks_count: value.forks_count,
            forks_url: value.forks_url,
            full_name: value.full_name,
            git_commits_url: value.git_commits_url,
            git_refs_url: value.git_refs_url,
            git_tags_url: value.git_tags_url,
            git_url: value.git_url,
            has_discussions: value.has_discussions,
            has_downloads: value
                .has_downloads
                .expect("full repository supplies has_downloads"),
            has_issues: value.has_issues,
            has_pages: value.has_pages,
            has_projects: value.has_projects,
            has_pull_requests: value
                .has_pull_requests
                .expect("full repository supplies has_pull_requests"),
            has_wiki: value.has_wiki,
            homepage: value.homepage,
            hooks_url: value.hooks_url,
            html_url: value.html_url,
            id: value.id,
            is_template: value
                .is_template
                .expect("full repository supplies is_template"),
            issue_comment_url: value.issue_comment_url,
            issue_events_url: value.issue_events_url,
            issues_url: value.issues_url,
            keys_url: value.keys_url,
            labels_url: value.labels_url,
            language: value.language,
            languages_url: value.languages_url,
            license: value.license,
            master_branch: value.master_branch,
            merge_commit_message: value.merge_commit_message.map(|value| value.into()),
            merge_commit_title: value.merge_commit_title.map(|value| value.into()),
            merges_url: value.merges_url,
            milestones_url: value.milestones_url,
            mirror_url: value.mirror_url,
            name: value.name,
            node_id: value.node_id,
            notifications_url: value.notifications_url,
            open_issues: value.open_issues,
            open_issues_count: value.open_issues_count,
            owner: value.owner,
            permissions: value.permissions.map(|value| value.into()),
            private: value.private,
            pull_request_creation_policy: value
                .pull_request_creation_policy
                .map(|value| value.into()),
            pulls_url: value.pulls_url,
            pushed_at: Some(value.pushed_at),
            releases_url: value.releases_url,
            size: value.size,
            squash_merge_commit_message: value
                .squash_merge_commit_message
                .map(|value| value.into()),
            squash_merge_commit_title: value.squash_merge_commit_title.map(|value| value.into()),
            ssh_url: value.ssh_url,
            stargazers_count: value.stargazers_count,
            stargazers_url: value.stargazers_url,
            starred_at: None,
            statuses_url: value.statuses_url,
            subscribers_url: value.subscribers_url,
            subscription_url: value.subscription_url,
            svn_url: value.svn_url,
            tags_url: value.tags_url,
            teams_url: value.teams_url,
            temp_clone_token: value.temp_clone_token,
            topics: value.topics,
            trees_url: value.trees_url,
            updated_at: Some(value.updated_at),
            url: value.url,
            use_squash_pr_title_as_default: value
                .use_squash_pr_title_as_default
                .expect("full repository supplies use_squash_pr_title_as_default"),
            visibility: value
                .visibility
                .expect("full repository supplies visibility"),
            watchers: value.watchers,
            watchers_count: value.watchers_count,
            web_commit_signoff_required: value
                .web_commit_signoff_required
                .expect("full repository supplies web_commit_signoff_required"),
        }
    }
}

/// Projects `FullRepositoryMergeCommitMessage` into the fields of `RepositoryMergeCommitMessage`.
impl From<types::FullRepositoryMergeCommitMessage> for types::RepositoryMergeCommitMessage {
    fn from(value: types::FullRepositoryMergeCommitMessage) -> Self {
        match value {
            types::FullRepositoryMergeCommitMessage::PrBody => Self::PrBody,
            types::FullRepositoryMergeCommitMessage::PrTitle => Self::PrTitle,
            types::FullRepositoryMergeCommitMessage::Blank => Self::Blank,
        }
    }
}

/// Projects `FullRepositoryMergeCommitTitle` into the fields of `RepositoryMergeCommitTitle`.
impl From<types::FullRepositoryMergeCommitTitle> for types::RepositoryMergeCommitTitle {
    fn from(value: types::FullRepositoryMergeCommitTitle) -> Self {
        match value {
            types::FullRepositoryMergeCommitTitle::PrTitle => Self::PrTitle,
            types::FullRepositoryMergeCommitTitle::MergeMessage => Self::MergeMessage,
        }
    }
}

/// Projects `FullRepositoryPermissions` into the fields of `RepositoryPermissions`.
impl From<types::FullRepositoryPermissions> for types::RepositoryPermissions {
    fn from(value: types::FullRepositoryPermissions) -> Self {
        Self {
            admin: value.admin,
            maintain: value.maintain,
            pull: value.pull,
            push: value.push,
            triage: value.triage,
        }
    }
}

/// Projects `FullRepositoryPullRequestCreationPolicy` into the fields of `RepositoryPullRequestCreationPolicy`.
impl From<types::FullRepositoryPullRequestCreationPolicy>
    for types::RepositoryPullRequestCreationPolicy
{
    fn from(value: types::FullRepositoryPullRequestCreationPolicy) -> Self {
        match value {
            types::FullRepositoryPullRequestCreationPolicy::All => Self::All,
            types::FullRepositoryPullRequestCreationPolicy::CollaboratorsOnly => {
                Self::CollaboratorsOnly
            }
        }
    }
}

/// Projects `FullRepositorySquashMergeCommitMessage` into the fields of `RepositorySquashMergeCommitMessage`.
impl From<types::FullRepositorySquashMergeCommitMessage>
    for types::RepositorySquashMergeCommitMessage
{
    fn from(value: types::FullRepositorySquashMergeCommitMessage) -> Self {
        match value {
            types::FullRepositorySquashMergeCommitMessage::PrBody => Self::PrBody,
            types::FullRepositorySquashMergeCommitMessage::CommitMessages => Self::CommitMessages,
            types::FullRepositorySquashMergeCommitMessage::Blank => Self::Blank,
        }
    }
}

/// Projects `FullRepositorySquashMergeCommitTitle` into the fields of `RepositorySquashMergeCommitTitle`.
impl From<types::FullRepositorySquashMergeCommitTitle> for types::RepositorySquashMergeCommitTitle {
    fn from(value: types::FullRepositorySquashMergeCommitTitle) -> Self {
        match value {
            types::FullRepositorySquashMergeCommitTitle::PrTitle => Self::PrTitle,
            types::FullRepositorySquashMergeCommitTitle::CommitOrPrTitle => Self::CommitOrPrTitle,
        }
    }
}

/// Projects `SimpleUser` into the fields of `NullableSimpleUser`.
impl From<types::SimpleUser> for types::NullableSimpleUser {
    fn from(value: types::SimpleUser) -> Self {
        Self {
            avatar_url: value.avatar_url,
            email: value.email,
            events_url: value.events_url,
            followers_url: value.followers_url,
            following_url: value.following_url,
            gists_url: value.gists_url,
            gravatar_id: value.gravatar_id,
            html_url: value.html_url,
            id: value.id,
            login: value.login,
            name: value.name,
            node_id: value.node_id,
            organizations_url: value.organizations_url,
            received_events_url: value.received_events_url,
            repos_url: value.repos_url,
            site_admin: value.site_admin,
            starred_at: value.starred_at,
            starred_url: value.starred_url,
            subscriptions_url: value.subscriptions_url,
            type_: value.type_,
            url: value.url,
            user_view_type: value.user_view_type,
        }
    }
}

/// Projects `NullableSimpleUser` back into `SimpleUser`; the two have the same fields.
impl From<types::NullableSimpleUser> for types::SimpleUser {
    fn from(value: types::NullableSimpleUser) -> Self {
        Self {
            avatar_url: value.avatar_url,
            email: value.email,
            events_url: value.events_url,
            followers_url: value.followers_url,
            following_url: value.following_url,
            gists_url: value.gists_url,
            gravatar_id: value.gravatar_id,
            html_url: value.html_url,
            id: value.id,
            login: value.login,
            name: value.name,
            node_id: value.node_id,
            organizations_url: value.organizations_url,
            received_events_url: value.received_events_url,
            repos_url: value.repos_url,
            site_admin: value.site_admin,
            starred_at: value.starred_at,
            starred_url: value.starred_url,
            subscriptions_url: value.subscriptions_url,
            type_: value.type_,
            url: value.url,
            user_view_type: value.user_view_type,
        }
    }
}

/// The short account representation embedded in other objects.
pub fn simple_user(request: &ApiRequest, account: &Account) -> types::SimpleUser {
    let login = &account.login;
    let urls = request.urls().user(login);
    types::SimpleUser {
        avatar_url: urls.avatar.clone(),
        email: None,
        events_url: urls.api("/events{/privacy}"),
        followers_url: urls.api("/followers"),
        following_url: urls.api("/following{/other_user}"),
        gists_url: urls.api("/gists{/gist_id}"),
        gravatar_id: Some(String::new()),
        html_url: urls.html.clone(),
        id: id(account.id),
        login: login.clone(),
        name: account.name.clone(),
        node_id: node_id(account.kind.as_str(), account.id),
        organizations_url: urls.api("/orgs"),
        received_events_url: urls.api("/received_events"),
        repos_url: urls.api("/repos"),
        site_admin: false,
        starred_at: None,
        starred_url: urls.api("/starred{/owner}{/repo}"),
        subscriptions_url: urls.api("/subscriptions"),
        type_: account.kind.as_str().to_owned(),
        url: urls.api,
        user_view_type: Some("public".to_owned()),
    }
}

/// [`simple_user`] of the account `login`, which events and records reference.
///
/// # Panics
/// When no account has that login; accounts are never deleted.
pub fn referenced_user(state: &State, request: &ApiRequest, login: &str) -> types::SimpleUser {
    let account = state
        .account(login)
        .unwrap_or_else(|| panic!("referenced account {login} exists"));
    simple_user(request, account)
}

/// [`team_simple`] of the team `id` of `org`; `None` once the team is deleted.
pub fn referenced_team(
    state: &State,
    request: &ApiRequest,
    org: &str,
    id: u64,
) -> Option<types::TeamSimple> {
    let org = state.account(org)?;
    let team = org.teams.values().find(|team| team.id == id)?;
    Some(team_simple(request, org, team))
}

/// A team of the organization `org`, as pull requests list their requested teams.
pub fn team_simple(
    request: &ApiRequest,
    org: &Account,
    team: &crate::state::Team,
) -> types::TeamSimple {
    let urls = request.urls().team(&org.login, &team.slug);
    types::TeamSimple {
        description: None,
        enterprise_id: None,
        html_url: urls.html.clone(),
        id: id(team.id),
        ldap_dn: None,
        members_url: urls.members(),
        name: team.slug.clone(),
        node_id: node_id("Team", team.id),
        notification_setting: Some("notifications_enabled".to_owned()),
        organization_id: Some(id(org.id)),
        permission: "pull".to_owned(),
        privacy: Some("closed".to_owned()),
        repositories_url: urls.repositories(),
        slug: team.slug.clone(),
        type_: types::TeamSimpleType::Organization,
        url: urls.api,
    }
}

/// [`simple_user`] where the spec allows it to be absent.
pub fn nullable_simple_user(request: &ApiRequest, account: &Account) -> types::NullableSimpleUser {
    simple_user(request, account).into()
}

/// The public profile of a user or organization.
pub fn public_user(request: &ApiRequest, account: &Account) -> types::PublicUser {
    let user = simple_user(request, account);
    let created = timestamp(&account.created_at);
    types::PublicUser {
        avatar_url: user.avatar_url,
        bio: None,
        blog: Some(String::new()),
        collaborators: None,
        company: None,
        created_at: created,
        disk_usage: None,
        email: None,
        events_url: user.events_url,
        followers: 0,
        followers_url: user.followers_url,
        following: 0,
        following_url: user.following_url,
        gists_url: user.gists_url,
        gravatar_id: user.gravatar_id,
        hireable: None,
        html_url: user.html_url,
        id: user.id,
        location: None,
        login: user.login,
        name: account.name.clone(),
        node_id: user.node_id,
        notification_email: None,
        organizations_url: user.organizations_url,
        owned_private_repos: None,
        plan: None,
        private_gists: None,
        public_gists: 0,
        public_repos: public_repos(account),
        received_events_url: user.received_events_url,
        repos_url: user.repos_url,
        site_admin: false,
        starred_url: user.starred_url,
        subscriptions_url: user.subscriptions_url,
        total_private_repos: None,
        twitter_username: None,
        type_: user.type_,
        updated_at: created,
        url: user.url,
        user_view_type: user.user_view_type,
    }
}

fn public_repos(account: &Account) -> i64 {
    account.repos().filter(|repo| !repo.private).count() as i64
}

/// An organization as GitHub returns it from `GET /orgs/{org}`.
pub fn organization(request: &ApiRequest, org: &Account) -> types::OrganizationFull {
    debug_assert_eq!(org.kind, AccountKind::Organization);
    let login = &org.login;
    let urls = request.urls().org(login);
    let created = timestamp(&org.created_at);
    let profile = &org.profile;
    types::OrganizationFull {
        advanced_security_enabled_for_new_repositories: None,
        archived_at: None,
        avatar_url: urls.avatar.clone(),
        billing_email: profile.billing_email.clone(),
        blog: Some(profile.blog.clone().unwrap_or_default()),
        collaborators: None,
        company: profile.company.clone(),
        created_at: created,
        default_repository_branch: None,
        default_repository_permission: profile.default_repository_permission.clone(),
        dependabot_alerts_enabled_for_new_repositories: None,
        dependabot_security_updates_enabled_for_new_repositories: None,
        dependency_graph_enabled_for_new_repositories: None,
        deploy_keys_enabled_for_repositories: None,
        description: Some(profile.description.clone().unwrap_or_default()),
        disk_usage: None,
        display_commenter_full_name_setting_enabled: None,
        email: profile.email.clone(),
        events_url: urls.api("/events"),
        followers: 0,
        following: 0,
        has_organization_projects: profile.has_organization_projects.unwrap_or(true),
        has_repository_projects: profile.has_repository_projects.unwrap_or(true),
        hooks_url: urls.api("/hooks"),
        html_url: urls.html.clone(),
        id: id(org.id),
        is_verified: None,
        issues_url: urls.api("/issues"),
        location: profile.location.clone(),
        login: login.clone(),
        members_allowed_repository_creation_type: None,
        members_can_change_repo_visibility: None,
        members_can_create_internal_repositories: None,
        members_can_create_pages: None,
        members_can_create_private_pages: None,
        members_can_create_private_repositories: None,
        members_can_create_public_pages: None,
        members_can_create_public_repositories: None,
        members_can_create_repositories: None,
        members_can_create_teams: None,
        members_can_delete_issues: None,
        members_can_delete_repositories: None,
        members_can_fork_private_repositories: None,
        members_can_invite_outside_collaborators: None,
        members_can_view_dependency_insights: None,
        members_url: urls.api("/members{/member}"),
        name: Some(org.name.clone().unwrap_or_else(|| login.clone())),
        node_id: node_id("Organization", org.id),
        owned_private_repos: None,
        plan: None,
        private_gists: None,
        public_gists: 0,
        public_members_url: urls.api("/public_members{/member}"),
        public_repos: public_repos(org),
        readers_can_create_discussions: None,
        repos_url: urls.api("/repos"),
        secret_scanning_enabled_for_new_repositories: None,
        secret_scanning_push_protection_custom_link: None,
        secret_scanning_push_protection_custom_link_enabled: None,
        secret_scanning_push_protection_enabled_for_new_repositories: None,
        total_private_repos: None,
        twitter_username: profile.twitter_username.clone(),
        two_factor_requirement_enabled: None,
        type_: "Organization".to_owned(),
        updated_at: created,
        url: urls.api,
        web_commit_signoff_required: None,
    }
}

/// A repository as GitHub returns it from `GET /repos/{owner}/{repo}`.
pub fn full_repository(
    request: &ApiRequest,
    owner: &Account,
    repo: &Repo,
    permission: Permission,
) -> types::FullRepository {
    let full_name = format!("{}/{}", owner.login, repo.name);
    let urls = request.urls().repo(&owner.login, &repo.name);
    let created = timestamp(&repo.created_at);
    let settings = &repo.settings;
    types::FullRepository {
        allow_auto_merge: Some(settings.allow_auto_merge),
        allow_forking: Some(settings.allow_forking),
        allow_merge_commit: Some(settings.allow_merge_commit),
        allow_rebase_merge: Some(settings.allow_rebase_merge),
        allow_squash_merge: Some(settings.allow_squash_merge),
        allow_update_branch: Some(settings.allow_update_branch),
        anonymous_access_enabled: false,
        archive_url: urls.api("/{archive_format}{/ref}"),
        archived: settings.archived,
        assignees_url: urls.api("/assignees{/user}"),
        blobs_url: urls.api("/git/blobs{/sha}"),
        branches_url: urls.api("/branches{/branch}"),
        clone_url: urls.clone_url(),
        code_of_conduct: None,
        collaborators_url: urls.api("/collaborators{/collaborator}"),
        comments_url: urls.api("/comments{/number}"),
        commits_url: urls.api("/commits{/sha}"),
        compare_url: urls.api("/compare/{base}...{head}"),
        contents_url: urls.api("/contents/{+path}"),
        contributors_url: urls.api("/contributors"),
        created_at: created,
        custom_properties: serde_json::Map::new(),
        default_branch: repo.default_branch.clone(),
        delete_branch_on_merge: Some(settings.delete_branch_on_merge),
        deployments_url: urls.api("/deployments"),
        description: repo.description.clone(),
        disabled: false,
        downloads_url: urls.api("/downloads"),
        events_url: urls.api("/events"),
        fork: false,
        forks: 0,
        forks_count: 0,
        forks_url: urls.api("/forks"),
        full_name: full_name.clone(),
        git_commits_url: urls.api("/git/commits{/sha}"),
        git_refs_url: urls.api("/git/refs{/sha}"),
        git_tags_url: urls.api("/git/tags{/sha}"),
        git_url: urls.git_url(),
        has_discussions: false,
        has_downloads: Some(true),
        has_issues: settings.has_issues,
        has_pages: false,
        has_projects: settings.has_projects,
        has_pull_requests: Some(settings.has_pull_requests),
        has_wiki: settings.has_wiki,
        homepage: settings.homepage.clone(),
        hooks_url: urls.api("/hooks"),
        html_url: urls.html.clone(),
        id: id(repo.id),
        is_template: Some(settings.is_template),
        issue_comment_url: urls.api("/issues/comments{/number}"),
        issue_events_url: urls.api("/issues/events{/number}"),
        issues_url: urls.api("/issues{/number}"),
        keys_url: urls.api("/keys{/key_id}"),
        labels_url: urls.api("/labels{/name}"),
        language: repo.language.clone(),
        languages_url: urls.api("/languages"),
        license: None,
        master_branch: None,
        merge_commit_message: Some(settings.merge_commit_message),
        merge_commit_title: Some(settings.merge_commit_title),
        merges_url: urls.api("/merges"),
        milestones_url: urls.api("/milestones{/number}"),
        mirror_url: None,
        name: repo.name.clone(),
        network_count: 0,
        node_id: node_id("Repository", repo.id),
        notifications_url: urls.api("/notifications{?since,all,participating}"),
        open_issues: 0,
        open_issues_count: 0,
        organization: None,
        owner: simple_user(request, owner),
        parent: None,
        permissions: Some(types::FullRepositoryPermissions {
            admin: permission.admin,
            maintain: Some(permission.admin),
            pull: permission.pull,
            push: permission.push,
            triage: Some(permission.push),
        }),
        private: repo.private,
        pull_request_creation_policy: Some(settings.pull_request_creation_policy),
        pulls_url: urls.api("/pulls{/number}"),
        pushed_at: repo.pushed_at.as_deref().map_or(created, timestamp),
        releases_url: urls.api("/releases{/id}"),
        security_and_analysis: None,
        size: 0,
        source: None,
        squash_merge_commit_message: Some(settings.squash_merge_commit_message),
        squash_merge_commit_title: Some(settings.squash_merge_commit_title),
        ssh_url: urls.ssh_url(),
        stargazers_count: 0,
        stargazers_url: urls.api("/stargazers"),
        statuses_url: urls.api("/statuses/{sha}"),
        subscribers_count: 0,
        subscribers_url: urls.api("/subscribers"),
        subscription_url: urls.api("/subscription"),
        svn_url: urls.html.clone(),
        tags_url: urls.api("/tags"),
        teams_url: urls.api("/teams"),
        temp_clone_token: None,
        template_repository: None,
        topics: Vec::new(),
        trees_url: urls.api("/git/trees{/sha}"),
        updated_at: timestamp(&repo.updated_at),
        url: urls.api,
        use_squash_pr_title_as_default: Some(settings.use_squash_pr_title_as_default),
        visibility: Some(if repo.private { "private" } else { "public" }.to_owned()),
        watchers: 0,
        watchers_count: 0,
        web_commit_signoff_required: Some(settings.web_commit_signoff_required),
    }
}

/// Renders the minimal repository representation without a JSON conversion.
pub fn minimal_repository(
    request: &ApiRequest,
    account: &Account,
    repo: &Repo,
    permission: Permission,
) -> types::MinimalRepository {
    full_repository(request, account, repo, permission).into()
}

/// Renders the repository representation embedded in a pull request.
pub fn repository_for_pull(
    request: &ApiRequest,
    account: &Account,
    repo: &Repo,
    permission: Permission,
) -> types::Repository {
    full_repository(request, account, repo, permission).into()
}

/// Applies the request policy to repository settings.
impl From<types::ReposUpdateRequestPullRequestCreationPolicy>
    for types::FullRepositoryPullRequestCreationPolicy
{
    fn from(value: types::ReposUpdateRequestPullRequestCreationPolicy) -> Self {
        match value {
            types::ReposUpdateRequestPullRequestCreationPolicy::All => Self::All,
            types::ReposUpdateRequestPullRequestCreationPolicy::CollaboratorsOnly => {
                Self::CollaboratorsOnly
            }
        }
    }
}

/// Applies the request policy to repository settings.
impl From<types::ReposUpdateRequestSquashMergeCommitTitle>
    for types::FullRepositorySquashMergeCommitTitle
{
    fn from(value: types::ReposUpdateRequestSquashMergeCommitTitle) -> Self {
        match value {
            types::ReposUpdateRequestSquashMergeCommitTitle::PrTitle => Self::PrTitle,
            types::ReposUpdateRequestSquashMergeCommitTitle::CommitOrPrTitle => {
                Self::CommitOrPrTitle
            }
        }
    }
}

/// Applies the request policy to repository settings.
impl From<types::ReposUpdateRequestSquashMergeCommitMessage>
    for types::FullRepositorySquashMergeCommitMessage
{
    fn from(value: types::ReposUpdateRequestSquashMergeCommitMessage) -> Self {
        match value {
            types::ReposUpdateRequestSquashMergeCommitMessage::PrBody => Self::PrBody,
            types::ReposUpdateRequestSquashMergeCommitMessage::CommitMessages => {
                Self::CommitMessages
            }
            types::ReposUpdateRequestSquashMergeCommitMessage::Blank => Self::Blank,
        }
    }
}

/// Applies the request policy to repository settings.
impl From<types::ReposUpdateRequestMergeCommitTitle> for types::FullRepositoryMergeCommitTitle {
    fn from(value: types::ReposUpdateRequestMergeCommitTitle) -> Self {
        match value {
            types::ReposUpdateRequestMergeCommitTitle::PrTitle => Self::PrTitle,
            types::ReposUpdateRequestMergeCommitTitle::MergeMessage => Self::MergeMessage,
        }
    }
}

/// Applies the request policy to repository settings.
impl From<types::ReposUpdateRequestMergeCommitMessage> for types::FullRepositoryMergeCommitMessage {
    fn from(value: types::ReposUpdateRequestMergeCommitMessage) -> Self {
        match value {
            types::ReposUpdateRequestMergeCommitMessage::PrBody => Self::PrBody,
            types::ReposUpdateRequestMergeCommitMessage::PrTitle => Self::PrTitle,
            types::ReposUpdateRequestMergeCommitMessage::Blank => Self::Blank,
        }
    }
}

/// Projects `PullRequest` into the fields of `PullRequestSimple`.
impl From<types::PullRequest> for types::PullRequestSimple {
    fn from(value: types::PullRequest) -> Self {
        Self {
            active_lock_reason: value.active_lock_reason,
            assignee: value.assignee,
            assignees: value.assignees,
            author_association: value.author_association,
            auto_merge: value.auto_merge,
            base: value.base.into(),
            body: value.body,
            closed_at: value.closed_at,
            comments_url: value.comments_url,
            commits_url: value.commits_url,
            created_at: value.created_at,
            diff_url: value.diff_url,
            draft: value.draft,
            head: value.head.into(),
            html_url: value.html_url,
            id: value.id,
            issue_url: value.issue_url,
            labels: value.labels.into_iter().map(|value| value.into()).collect(),
            links: value.links.into(),
            locked: value.locked,
            merge_commit_sha: value.merge_commit_sha,
            merged_at: value.merged_at,
            milestone: value.milestone,
            node_id: value.node_id,
            number: value.number,
            patch_url: value.patch_url,
            requested_reviewers: value.requested_reviewers,
            requested_teams: value
                .requested_teams
                .into_iter()
                .map(|value| value.into())
                .collect(),
            review_comment_url: value.review_comment_url,
            review_comments_url: value.review_comments_url,
            stack: value.stack,
            state: value.state.to_string(),
            statuses_url: value.statuses_url,
            title: value.title,
            updated_at: value.updated_at,
            url: value.url,
            user: Some(value.user.into()),
        }
    }
}

/// Projects `PullRequestBase` into the fields of `PullRequestSimpleBase`.
impl From<types::PullRequestBase> for types::PullRequestSimpleBase {
    fn from(value: types::PullRequestBase) -> Self {
        Self {
            label: value.label,
            ref_: value.ref_,
            repo: value.repo,
            sha: value.sha,
            user: Some(value.user.into()),
        }
    }
}

/// Projects `PullRequestHead` into the fields of `PullRequestSimpleHead`.
impl From<types::PullRequestHead> for types::PullRequestSimpleHead {
    fn from(value: types::PullRequestHead) -> Self {
        Self {
            label: value.label,
            ref_: value.ref_,
            repo: value.repo,
            sha: value.sha,
            user: Some(value.user.into()),
        }
    }
}

/// Projects `PullRequestLabelsItem` into the fields of `PullRequestSimpleLabelsItem`.
impl From<types::PullRequestLabelsItem> for types::PullRequestSimpleLabelsItem {
    fn from(value: types::PullRequestLabelsItem) -> Self {
        Self {
            archived_at: value.archived_at,
            archived_by: value.archived_by,
            color: value.color,
            default: value.default,
            description: value
                .description
                .expect("the full response supplies this required field"),
            id: value.id,
            name: value.name,
            node_id: value.node_id,
            url: value.url,
        }
    }
}

/// Projects `PullRequestLinks` into the fields of `PullRequestSimpleLinks`.
impl From<types::PullRequestLinks> for types::PullRequestSimpleLinks {
    fn from(value: types::PullRequestLinks) -> Self {
        Self {
            comments: value.comments,
            commits: value.commits,
            html: value.html,
            issue: value.issue,
            review_comment: value.review_comment,
            review_comments: value.review_comments,
            self_: value.self_,
            statuses: value.statuses,
        }
    }
}

/// Projects `TeamSimple` into the fields of `Team`.
impl From<types::TeamSimple> for types::Team {
    fn from(value: types::TeamSimple) -> Self {
        Self {
            access_source: None,
            description: value.description,
            enterprise_id: value.enterprise_id,
            html_url: value.html_url,
            id: value.id,
            members_url: value.members_url,
            name: value.name,
            node_id: value.node_id,
            notification_setting: value.notification_setting,
            organization_id: value.organization_id,
            parent: None,
            permission: value.permission,
            permissions: None,
            privacy: value.privacy,
            repositories_url: value.repositories_url,
            slug: value.slug,
            type_: value.type_.into(),
            url: value.url,
        }
    }
}

/// Projects `TeamSimpleType` into the fields of `TeamType`.
impl From<types::TeamSimpleType> for types::TeamType {
    fn from(value: types::TeamSimpleType) -> Self {
        match value {
            types::TeamSimpleType::Enterprise => Self::Enterprise,
            types::TeamSimpleType::Organization => Self::Organization,
        }
    }
}

/// Projects a pull request into its list representation.
pub fn pull_request_simple(pull: &types::PullRequest) -> types::PullRequestSimple {
    pull.clone().into()
}

/// Projects a REST resource into its webhook representation.
impl From<types::FullRepository> for webhook_types::Repository {
    fn from(value: types::FullRepository) -> Self {
        Self {
            allow_auto_merge: value
                .allow_auto_merge
                .expect("the rendered resource supplies this required field"),
            allow_forking: value.allow_forking,
            allow_merge_commit: value
                .allow_merge_commit
                .expect("the rendered resource supplies this required field"),
            allow_rebase_merge: value
                .allow_rebase_merge
                .expect("the rendered resource supplies this required field"),
            allow_squash_merge: value
                .allow_squash_merge
                .expect("the rendered resource supplies this required field"),
            allow_update_branch: value.allow_update_branch,
            archive_url: value.archive_url,
            archived: value.archived,
            assignees_url: value.assignees_url,
            blobs_url: value.blobs_url,
            branches_url: value.branches_url,
            clone_url: value.clone_url,
            collaborators_url: value.collaborators_url,
            comments_url: value.comments_url,
            commits_url: value.commits_url,
            compare_url: value.compare_url,
            contents_url: value.contents_url,
            contributors_url: value.contributors_url,
            created_at: webhook_types::RepositoryCreatedAt::DateTime(value.created_at),
            custom_properties: value
                .custom_properties
                .into_iter()
                .map(|(name, value)| (name, webhook_custom_property(value)))
                .collect(),
            default_branch: value.default_branch,
            delete_branch_on_merge: value
                .delete_branch_on_merge
                .expect("the rendered resource supplies this required field"),
            deployments_url: value.deployments_url,
            description: value.description,
            disabled: Some(value.disabled),
            downloads_url: value.downloads_url,
            events_url: value.events_url,
            fork: value.fork,
            forks: value.forks,
            forks_count: value.forks_count,
            forks_url: value.forks_url,
            full_name: value.full_name,
            git_commits_url: value.git_commits_url,
            git_refs_url: value.git_refs_url,
            git_tags_url: value.git_tags_url,
            git_url: value.git_url,
            has_discussions: value.has_discussions,
            has_downloads: value
                .has_downloads
                .expect("the rendered resource supplies this required field"),
            has_issues: value.has_issues,
            has_pages: value.has_pages,
            has_projects: value.has_projects,
            has_wiki: value.has_wiki,
            homepage: value.homepage,
            hooks_url: value.hooks_url,
            html_url: value.html_url,
            id: value.id,
            is_template: value
                .is_template
                .expect("the rendered resource supplies this required field"),
            issue_comment_url: value.issue_comment_url,
            issue_events_url: value.issue_events_url,
            issues_url: value.issues_url,
            keys_url: value.keys_url,
            labels_url: value.labels_url,
            language: value.language,
            languages_url: value.languages_url,
            license: value.license.map(|value| value.into()),
            master_branch: value.master_branch,
            merge_commit_message: value.merge_commit_message.map(|value| value.to_string()),
            merge_commit_title: value.merge_commit_title.map(|value| value.to_string()),
            merges_url: value.merges_url,
            milestones_url: value.milestones_url,
            mirror_url: value.mirror_url,
            name: value.name,
            node_id: value.node_id,
            notifications_url: value.notifications_url,
            open_issues: value.open_issues,
            open_issues_count: value.open_issues_count,
            organization: value.organization.map(|value| value.login),
            owner: value.owner.into(),
            permissions: value.permissions.map(|value| value.into()),
            private: value.private,
            public: None,
            pulls_url: value.pulls_url,
            pushed_at: webhook_types::RepositoryPushedAt::DateTime(value.pushed_at),
            releases_url: value.releases_url,
            size: value.size,
            squash_merge_commit_message: value
                .squash_merge_commit_message
                .map(|value| value.to_string()),
            squash_merge_commit_title: value
                .squash_merge_commit_title
                .map(|value| value.to_string()),
            ssh_url: value.ssh_url,
            stargazers: None,
            stargazers_count: value.stargazers_count,
            stargazers_url: value.stargazers_url,
            statuses_url: value.statuses_url,
            subscribers_url: value.subscribers_url,
            subscription_url: value.subscription_url,
            svn_url: value.svn_url,
            tags_url: value.tags_url,
            teams_url: value.teams_url,
            topics: value.topics,
            trees_url: value.trees_url,
            updated_at: value.updated_at,
            url: value.url,
            use_squash_pr_title_as_default: value.use_squash_pr_title_as_default,
            visibility: value
                .visibility
                .expect("the rendered resource supplies this required field")
                .parse()
                .expect("rendered resource values match the webhook schema"),
            watchers: value.watchers,
            watchers_count: value.watchers_count,
            web_commit_signoff_required: value
                .web_commit_signoff_required
                .expect("the rendered resource supplies this required field"),
        }
    }
}

/// Projects a REST resource into its webhook representation.
impl From<types::NullableLicenseSimple> for webhook_types::License {
    fn from(value: types::NullableLicenseSimple) -> Self {
        Self {
            key: value.key,
            name: value.name,
            node_id: value.node_id,
            spdx_id: value
                .spdx_id
                .expect("the rendered resource supplies this required field"),
            url: value.url,
        }
    }
}

/// Projects a REST resource into its webhook representation.
impl From<types::SimpleUser> for webhook_types::User {
    fn from(value: types::SimpleUser) -> Self {
        Self {
            avatar_url: value.avatar_url,
            email: value.email,
            events_url: value.events_url,
            followers_url: value.followers_url,
            following_url: value.following_url,
            gists_url: value.gists_url,
            gravatar_id: value
                .gravatar_id
                .expect("the rendered resource supplies this required field"),
            html_url: value.html_url,
            id: value.id,
            login: value.login,
            name: value.name,
            node_id: value.node_id,
            organizations_url: value.organizations_url,
            received_events_url: value.received_events_url,
            repos_url: value.repos_url,
            site_admin: value.site_admin,
            starred_url: value.starred_url,
            subscriptions_url: value.subscriptions_url,
            type_: value
                .type_
                .parse()
                .expect("rendered resource values match the webhook schema"),
            url: value.url,
        }
    }
}

/// Projects a REST resource into its webhook representation.
impl From<types::FullRepositoryPermissions> for webhook_types::RepositoryPermissions {
    fn from(value: types::FullRepositoryPermissions) -> Self {
        Self {
            admin: value.admin,
            maintain: value.maintain,
            pull: value.pull,
            push: value.push,
            triage: value.triage,
        }
    }
}

/// The repository coordinates and live issue counts a milestone response needs.
pub struct MilestoneRender<'a> {
    /// Request the response is being built for.
    pub request: &'a ApiRequest,
    /// Repository owner login.
    pub owner: &'a str,
    /// Repository name.
    pub name: &'a str,
    /// Open issues and pull requests currently in the milestone.
    pub open_issues: i64,
    /// Closed issues and pull requests currently in the milestone.
    pub closed_issues: i64,
}

/// Renders a stored milestone, including the issue counts in `view`.
pub fn milestone(source: &Milestone, view: MilestoneRender<'_>) -> types::NullableMilestone {
    let urls = view
        .request
        .urls()
        .repo(view.owner, view.name)
        .milestone(source.number);
    let number = id(source.number);
    types::NullableMilestone {
        closed_at: source.closed_at,
        closed_issues: view.closed_issues,
        created_at: source.created_at,
        creator: Some(source.creator.clone()),
        description: source.description.clone(),
        due_on: source.due_on,
        html_url: urls.html.clone(),
        id: id(source.id),
        labels_url: urls.labels(),
        node_id: node_id("Milestone", source.id),
        number,
        open_issues: view.open_issues,
        state: source.state,
        title: source.title.clone(),
        updated_at: source.updated_at,
        url: urls.api,
    }
}

/// Open and closed issue and pull request counts keyed by milestone number.
pub fn milestone_counts(repo: &Repo) -> std::collections::BTreeMap<u64, (i64, i64)> {
    let mut counts = std::collections::BTreeMap::<u64, (i64, i64)>::new();
    let mut bump = |number: u64, open: bool| {
        let entry = counts.entry(number).or_insert((0, 0));
        if open {
            entry.0 += 1;
        } else {
            entry.1 += 1;
        }
    };
    for issue in repo.issues.values() {
        if let Some(number) = issue.milestone {
            bump(number, issue.is_open());
        }
    }
    for pull in repo
        .pull_requests
        .values()
        .filter_map(|pull| pull.data.as_ref())
    {
        if let Some(milestone) = &pull.milestone
            && let Ok(number) = u64::try_from(milestone.number)
        {
            bump(number, pull.state == types::PullRequestState::Open);
        }
    }
    counts
}

/// Renders a milestone of `repo` with its live issue counts.
pub fn stored_milestone(
    request: &ApiRequest,
    owner: &str,
    repo: &Repo,
    milestone: &Milestone,
) -> types::NullableMilestone {
    let (open_issues, closed_issues) = milestone_counts(repo)
        .get(&milestone.number)
        .copied()
        .unwrap_or((0, 0));
    self::milestone(
        milestone,
        MilestoneRender {
            request,
            owner,
            name: &repo.name,
            open_issues,
            closed_issues,
        },
    )
}

/// What rendering an issue or one of its comments reads besides the record itself.
#[derive(Clone, Copy)]
pub struct IssueContext<'a> {
    /// Accounts the issue's users are looked up in.
    pub state: &'a State,
    /// Request the response is being built for.
    pub request: &'a ApiRequest,
    /// Login of the repository owner.
    pub owner: &'a str,
    /// Repository holding the issue, its labels and milestones.
    pub repo: &'a Repo,
}

impl IssueContext<'_> {
    fn user(&self, login: &str) -> types::SimpleUser {
        let account = self
            .state
            .account(login)
            .unwrap_or_else(|| panic!("issue user {login} is not an account"));
        simple_user(self.request, account)
    }

    /// How `login` relates to the repository.
    ///
    /// Source: https://docs.github.com/en/graphql/reference/enums#commentauthorassociation
    /// (OWNER, MEMBER of the owning organization, NONE otherwise; this fake stores no outside
    /// collaborators or contributor history).
    fn association(&self, login: &str) -> types::AuthorAssociation {
        if self.owner.eq_ignore_ascii_case(login) {
            return types::AuthorAssociation::Owner;
        }
        let member = self.state.account(self.owner).is_some_and(|owner| {
            owner.kind == AccountKind::Organization && owner.members.contains(fold(login).as_ref())
        });
        if member {
            types::AuthorAssociation::Member
        } else {
            types::AuthorAssociation::None
        }
    }
}

/// Renders a stored issue.
///
/// # Panics
///
/// When a label, milestone or user the issue references is not stored.
pub fn issue(context: IssueContext<'_>, issue: &Issue) -> types::Issue {
    let repo_urls = context
        .request
        .urls()
        .repo(context.owner, &context.repo.name);
    let urls = repo_urls.issue(issue.number);
    let number = id(issue.number);
    let assignees: Vec<types::SimpleUser> = issue
        .assignees
        .iter()
        .map(|login| context.user(login.as_str()))
        .collect();
    let labels = issue
        .labels
        .iter()
        .map(|name| {
            let stored = context
                .repo
                .labels
                .get(name.folded())
                .unwrap_or_else(|| panic!("issue label {name} is not stored"));
            label_on_issue(&self::label(
                context.state,
                context.request,
                context.owner,
                &context.repo.name,
                stored,
            ))
        })
        .collect();
    let milestone = issue.milestone.map(|number| {
        let stored = context
            .repo
            .milestones
            .get(&number)
            .unwrap_or_else(|| panic!("issue milestone {number} is not stored"));
        stored_milestone(context.request, context.owner, context.repo, stored)
    });
    types::Issue {
        active_lock_reason: issue.lock_reason.map(|reason| reason.as_str().to_owned()),
        // Source: OpenAPI example `issue` (issues/get 200), whose `assignee` is the first of
        // its `assignees`; REST edits keep the same rule.
        assignee: assignees.first().cloned().map(Into::into),
        assignees,
        author_association: Some(context.association(issue.author.as_str())),
        body: issue.body.clone(),
        body_html: None,
        body_text: None,
        closed_at: issue.closed_at,
        closed_by: issue
            .closed_by
            .as_ref()
            .map(|login| context.user(login.as_str()).into()),
        comments: i64::try_from(issue.comments.len()).expect("comment counts fit in i64"),
        comments_url: urls.comments(),
        created_at: issue.created_at,
        draft: None,
        events_url: urls.events(),
        html_url: urls.html.clone(),
        id: number,
        issue_dependencies_summary: None,
        issue_field_values: Vec::new(),
        labels,
        labels_url: urls.labels(),
        locked: issue.locked,
        milestone,
        node_id: issue.node_id.clone(),
        number,
        parent_issue_url: None,
        performed_via_github_app: None,
        pinned_comment: None,
        pull_request: None,
        reactions: None,
        repository: None,
        repository_url: repo_urls.api.clone(),
        state: issue.state.as_str().to_owned(),
        state_reason: issue.state_reason.map(state_reason),
        sub_issues_summary: None,
        timeline_url: None,
        title: issue.title.clone(),
        type_: None,
        updated_at: issue.updated_at,
        url: urls.api,
        user: Some(context.user(issue.author.as_str()).into()),
    }
}

fn state_reason(reason: StateReason) -> types::IssueStateReason {
    match reason {
        StateReason::Completed => types::IssueStateReason::Completed,
        StateReason::NotPlanned => types::IssueStateReason::NotPlanned,
        StateReason::Duplicate => types::IssueStateReason::Duplicate,
        StateReason::Reopened => types::IssueStateReason::Reopened,
    }
}

/// Renders a stored comment on the issue `number`.
pub fn issue_comment(
    context: IssueContext<'_>,
    number: u64,
    comment: &Comment,
) -> types::IssueComment {
    let repo_urls = context
        .request
        .urls()
        .repo(context.owner, &context.repo.name);
    let urls = repo_urls.issue(number);
    types::IssueComment {
        author_association: Some(
            comment
                .author
                .as_ref()
                .map_or(types::AuthorAssociation::None, |login| {
                    context.association(login.as_str())
                }),
        ),
        body: Some(comment.body.clone()),
        body_html: None,
        body_text: None,
        created_at: comment.created_at,
        html_url: urls.comment_html(comment.id),
        id: id(comment.id),
        issue_url: urls.api,
        minimized: None,
        node_id: node_id("IssueComment", comment.id),
        performed_via_github_app: None,
        pin: None,
        reactions: None,
        updated_at: comment.updated_at,
        url: repo_urls.issue_comment(comment.id),
        user: comment
            .author
            .as_ref()
            .map(|login| context.user(login.as_str()).into()),
    }
}

/// The issue-label object embedded on an issue, copied from a repository label.
pub fn label_on_issue(label: &types::Label) -> types::IssueLabelsItem {
    types::IssueLabelsItem::Object {
        archived_by: label.archived_by.clone(),
        color: Some(label.color.clone()),
        default: Some(label.default),
        description: label.description.clone(),
        id: Some(label.id),
        name: Some(label.name.clone()),
        node_id: Some(label.node_id.clone()),
        url: Some(label.url.clone()),
    }
}

/// A stored label of `owner/name` as the label endpoints return it.
pub fn label(
    state: &State,
    request: &ApiRequest,
    owner: &str,
    name: &str,
    label: &LabelRecord,
) -> types::Label {
    types::Label {
        archived_at: label.archived_at,
        archived_by: label
            .archived_by
            .as_deref()
            .map(|login| referenced_user(state, request, login)),
        color: label.color.clone(),
        default: label.default,
        description: label.description.clone(),
        id: id(label.id),
        name: label.name.clone(),
        node_id: label.node_id.clone(),
        url: request.urls().repo(owner, name).label(&label.name),
    }
}

/// The label object embedded on a pull request, copied from a repository label.
pub fn label_on_pull(label: &types::Label) -> types::PullRequestLabelsItem {
    types::PullRequestLabelsItem {
        archived_at: label.archived_at,
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

/// Projects a pull request into the issue shape the issue endpoints return.
pub fn issue_from_pull(pull: &types::PullRequest) -> types::Issue {
    let repository_url = pull
        .issue_url
        .rsplit_once("/issues/")
        .map(|(repository, _)| repository.to_owned())
        .unwrap_or_else(|| pull.issue_url.clone());
    types::Issue {
        active_lock_reason: pull.active_lock_reason.clone(),
        assignee: pull.assignee.clone(),
        assignees: pull.assignees.clone(),
        author_association: Some(pull.author_association),
        body: pull.body.clone(),
        body_html: None,
        body_text: None,
        closed_at: pull.closed_at,
        closed_by: None,
        comments: pull.comments,
        comments_url: pull.comments_url.clone(),
        created_at: pull.created_at,
        draft: pull.draft,
        events_url: format!("{}/events", pull.issue_url),
        html_url: pull.html_url.clone(),
        id: pull.id,
        issue_dependencies_summary: None,
        issue_field_values: Vec::new(),
        labels: pull.labels.iter().map(label_from_pull_item).collect(),
        labels_url: format!("{}/labels{{/name}}", pull.issue_url),
        locked: pull.locked,
        milestone: pull.milestone.clone(),
        node_id: pull.node_id.clone(),
        number: pull.number,
        parent_issue_url: None,
        performed_via_github_app: None,
        pinned_comment: None,
        pull_request: Some(types::IssuePullRequest {
            diff_url: Some(pull.diff_url.clone()),
            html_url: Some(pull.html_url.clone()),
            merged_at: pull.merged_at,
            patch_url: Some(pull.patch_url.clone()),
            url: Some(pull.url.clone()),
        }),
        reactions: None,
        repository: None,
        repository_url,
        state: pull.state.to_string(),
        state_reason: None,
        sub_issues_summary: None,
        timeline_url: Some(format!("{}/timeline", pull.issue_url)),
        title: pull.title.clone(),
        type_: None,
        updated_at: pull.updated_at,
        url: pull.issue_url.clone(),
        user: Some(pull.user.clone().into()),
    }
}

/// Converts one pull-request label into the issue label union.
fn label_from_pull_item(label: &types::PullRequestLabelsItem) -> types::IssueLabelsItem {
    types::IssueLabelsItem::Object {
        archived_by: label.archived_by.clone(),
        color: Some(label.color.clone()),
        default: Some(label.default),
        description: label.description.clone(),
        id: Some(label.id),
        name: Some(label.name.clone()),
        node_id: Some(label.node_id.clone()),
        url: Some(label.url.clone()),
    }
}

/// Converts one dynamically named webhook custom property.
fn webhook_custom_property(
    value: serde_json::Value,
) -> webhook_types::RepositoryCustomPropertiesValue {
    match value {
        serde_json::Value::Null => webhook_types::RepositoryCustomPropertiesValue::Null,
        serde_json::Value::String(value) => {
            webhook_types::RepositoryCustomPropertiesValue::String(value)
        }
        serde_json::Value::Array(values) => webhook_types::RepositoryCustomPropertiesValue::Array(
            values
                .into_iter()
                .map(|value| match value {
                    serde_json::Value::String(value) => value,
                    _ => unreachable!("custom property arrays contain strings"),
                })
                .collect(),
        ),
        _ => unreachable!("rendered custom properties are strings, string arrays or null"),
    }
}

/// A workflow run of `owner`'s `repo`, as the Actions API returns it.
pub fn workflow_run(
    request: &ApiRequest,
    owner: &Account,
    repo: &Repo,
    run: &crate::state::WorkflowRunRecord,
) -> types::WorkflowRun {
    let repo_urls = request.urls().repo(&owner.login, &repo.name);
    let urls = repo_urls.workflow_run(run.id);
    let minimal_repo = minimal_repository(request, owner, repo, Permission::ADMIN);
    types::WorkflowRun {
        actor: None,
        artifacts_url: urls.api("/artifacts"),
        cancel_url: urls.api("/cancel"),
        check_suite_id: None,
        check_suite_node_id: None,
        check_suite_url: repo_urls.check_suite(run.id).api,
        conclusion: run.conclusion.clone(),
        created_at: run.created_at,
        display_title: run.display_title.clone(),
        event: run.event.clone(),
        head_branch: run.head_branch.clone(),
        head_commit: None,
        head_repository: minimal_repo.clone(),
        head_repository_id: Some(id(repo.id)),
        head_sha: run.head_sha.clone(),
        html_url: run.html_url.clone().unwrap_or_else(|| urls.html.clone()),
        id: id(run.id),
        jobs_url: urls.api("/jobs"),
        logs_url: urls.api("/logs"),
        name: run.name.clone(),
        node_id: node_id("WorkflowRun", run.id),
        path: run.path.clone(),
        previous_attempt_url: None,
        pull_requests: Some(Vec::new()),
        referenced_workflows: None,
        repository: minimal_repo,
        rerun_url: urls.api("/rerun"),
        run_attempt: Some(id(run.run_attempt)),
        run_number: id(run.run_number),
        run_started_at: run.run_started_at,
        status: run.status.clone(),
        triggering_actor: None,
        updated_at: run.updated_at,
        url: urls.api.clone(),
        workflow_id: id(run.workflow_id),
        // The fake serves no workflow files; every run names `ci.yml`.
        workflow_url: repo_urls.workflow("ci.yml"),
    }
}

/// A job of `run` in `owner/name`, as the Actions API returns it.
pub fn workflow_job(
    request: &ApiRequest,
    owner: &str,
    name: &str,
    run: &crate::state::WorkflowRunRecord,
    job: &crate::state::JobRecord,
) -> types::Job {
    let repo_urls = request.urls().repo(owner, name);
    let urls = repo_urls.workflow_job(run.id, job.id);
    types::Job {
        check_run_url: repo_urls.check_run(job.id).api,
        completed_at: job.completed_at,
        conclusion: job.conclusion,
        created_at: job.created_at,
        head_branch: run.head_branch.clone(),
        head_sha: run.head_sha.clone(),
        html_url: Some(urls.html),
        id: id(job.id),
        labels: job.labels.clone(),
        name: job.name.clone(),
        node_id: node_id("WorkflowJob", job.id),
        run_attempt: Some(id(job.run_attempt)),
        run_id: id(run.id),
        run_url: repo_urls.workflow_run(run.id).api,
        runner_group_id: job.runner_group_id.map(id),
        runner_group_name: None,
        runner_id: job.runner_id.map(id),
        runner_name: job.runner_name.clone(),
        started_at: job.started_at,
        status: job.status,
        steps: job.steps.clone(),
        url: urls.api,
        workflow_name: run.name.clone(),
    }
}
