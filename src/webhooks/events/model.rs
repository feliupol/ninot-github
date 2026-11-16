//! Webhook objects shared by several events, built from the REST models the handlers render.

use std::fmt;
use std::str::FromStr;

use crate::api::ApiResponse;
use crate::types;
use crate::webhook_types as wt;

use super::Ctx;

/// Why a webhook payload cannot be built from a rendered REST model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PayloadError {
    /// `field` holds `value`, which the webhook schema does not allow.
    Unknown { field: &'static str, value: String },
    /// `field`, which the webhook schema requires, is missing.
    Missing { field: &'static str },
}

impl fmt::Display for PayloadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown { field, value } => {
                write!(
                    formatter,
                    "{field} is {value:?}, which the webhook schema does not allow"
                )
            }
            Self::Missing { field } => {
                write!(
                    formatter,
                    "{field} is missing, and the webhook schema requires it"
                )
            }
        }
    }
}

impl std::error::Error for PayloadError {}

impl From<PayloadError> for ApiResponse {
    // No GitHub source covers a payload the fake cannot build, so this is a 500 that names the
    // field and value.
    fn from(error: PayloadError) -> Self {
        ApiResponse::internal(format!(
            "ninot-github could not build the webhook payload: {error}"
        ))
    }
}

/// `text` as the webhook enum `T`, or [`PayloadError::Unknown`] for `field`.
pub(crate) fn parse<T: FromStr>(field: &'static str, text: &str) -> Result<T, PayloadError> {
    text.parse().map_err(|_| PayloadError::Unknown {
        field,
        value: text.to_owned(),
    })
}

/// The RFC 3339 timestamp `text`, or [`PayloadError::Unknown`] for `field`.
pub(crate) fn timestamp(
    field: &'static str,
    text: &str,
) -> Result<chrono::DateTime<chrono::Utc>, PayloadError> {
    crate::time::parse(text).map_err(|_| PayloadError::Unknown {
        field,
        value: text.to_owned(),
    })
}

/// The value of `field`, or [`PayloadError::Missing`].
pub(crate) fn required<T>(field: &'static str, value: Option<T>) -> Result<T, PayloadError> {
    value.ok_or(PayloadError::Missing { field })
}

/// The webhook user for `user`.
pub(crate) fn user(user: &types::SimpleUser) -> Result<wt::User, PayloadError> {
    user.clone().try_into()
}

/// The webhook user for a nullable REST user.
pub(crate) fn nullable_user(user: &types::NullableSimpleUser) -> Result<wt::User, PayloadError> {
    types::SimpleUser::from(user.clone()).try_into()
}

/// The webhook author association of `association`.
pub(crate) fn author_association(
    association: &types::AuthorAssociation,
) -> Result<wt::AuthorAssociation, PayloadError> {
    parse("author_association", &association.to_string())
}

/// The webhook label for `label`, with a URL and node id when the REST model left them empty.
pub(crate) fn label(label: &types::Label) -> wt::Label {
    wt::Label {
        color: label.color.clone(),
        default: label.default,
        description: label.description.clone(),
        id: label.id,
        name: label.name.clone(),
        node_id: if label.node_id.is_empty() {
            format!("L_{}", label.id)
        } else {
            label.node_id.clone()
        },
        url: if label.url.is_empty() {
            format!("https://github.com/labels/{}", label.name)
        } else {
            label.url.clone()
        },
    }
}

/// The reactions of `rollup`, or none counted at `{url}/reactions`.
pub(crate) fn reactions(rollup: Option<&types::ReactionRollup>, url: &str) -> wt::Reactions {
    match rollup {
        Some(rollup) => wt::Reactions {
            confused: rollup.confused,
            eyes: rollup.eyes,
            heart: rollup.heart,
            hooray: rollup.hooray,
            laugh: rollup.laugh,
            minus1: rollup.minus1,
            plus1: rollup.plus1,
            rocket: rollup.rocket,
            total_count: rollup.total_count,
            url: rollup.url.clone(),
        },
        None => wt::Reactions {
            confused: 0,
            eyes: 0,
            heart: 0,
            hooray: 0,
            laugh: 0,
            minus1: 0,
            plus1: 0,
            rocket: 0,
            total_count: 0,
            url: format!("{url}/reactions"),
        },
    }
}

/// The webhook milestone for `milestone`.
pub(crate) fn milestone(
    milestone: &types::NullableMilestone,
) -> Result<wt::Milestone, PayloadError> {
    Ok(wt::Milestone {
        closed_at: milestone.closed_at,
        closed_issues: milestone.closed_issues,
        created_at: milestone.created_at,
        creator: nullable_user(required("milestone.creator", milestone.creator.as_ref())?)?,
        description: milestone.description.clone(),
        due_on: milestone.due_on,
        html_url: milestone.html_url.clone(),
        id: milestone.id,
        labels_url: milestone.labels_url.clone(),
        node_id: milestone.node_id.clone(),
        number: milestone.number,
        open_issues: milestone.open_issues,
        state: parse("milestone.state", &milestone.state.to_string())?,
        title: milestone.title.clone(),
        updated_at: milestone.updated_at,
        url: milestone.url.clone(),
    })
}

/// The webhook team for `team`.
pub(crate) fn team(team: &types::TeamSimple) -> Result<wt::Team, PayloadError> {
    Ok(wt::Team {
        description: team.description.clone(),
        html_url: team.html_url.clone(),
        id: team.id,
        members_url: team.members_url.clone(),
        name: team.name.clone(),
        node_id: team.node_id.clone(),
        notification_setting: team
            .notification_setting
            .as_deref()
            .map(|setting| parse("team.notification_setting", setting))
            .transpose()?,
        parent: None,
        permission: team.permission.clone(),
        privacy: parse(
            "team.privacy",
            required("team.privacy", team.privacy.as_deref())?,
        )?,
        repositories_url: team.repositories_url.clone(),
        slug: team.slug.clone(),
        url: team.url.clone(),
    })
}

/// The labels of `issue` as REST labels; a label given only by name gets default fields.
pub(crate) fn issue_labels(issue: &types::Issue) -> Vec<types::Label> {
    issue
        .labels
        .iter()
        .map(|label| match label {
            types::IssueLabelsItem::String(name) => types::Label {
                archived_at: None,
                archived_by: None,
                color: "ededed".to_owned(),
                default: false,
                description: Some(String::new()),
                id: 0,
                name: name.clone(),
                node_id: String::new(),
                url: String::new(),
            },
            types::IssueLabelsItem::Object {
                archived_by,
                color,
                default,
                description,
                id,
                name,
                node_id,
                url,
            } => types::Label {
                archived_at: None,
                archived_by: archived_by.clone(),
                color: color.clone().unwrap_or_else(|| "ededed".to_owned()),
                default: default.unwrap_or(false),
                description: description.clone(),
                id: id.unwrap_or_default(),
                name: name.clone().unwrap_or_default(),
                node_id: node_id.clone().unwrap_or_default(),
                url: url.clone().unwrap_or_default(),
            },
        })
        .collect()
}

/// The webhook issue for `issue`. An issue without reactions gets an empty count.
pub(crate) fn issue(issue: &types::Issue) -> Result<wt::Issue, PayloadError> {
    Ok(wt::Issue {
        active_lock_reason: issue
            .active_lock_reason
            .as_deref()
            .map(|reason| parse("issue.active_lock_reason", reason))
            .transpose()?,
        assignee: issue.assignee.as_ref().map(nullable_user).transpose()?,
        assignees: issue.assignees.iter().map(user).collect::<Result<_, _>>()?,
        author_association: issue
            .author_association
            .as_ref()
            .map_or(Ok(wt::AuthorAssociation::None), author_association)?,
        body: issue.body.clone(),
        closed_at: issue.closed_at,
        comments: issue.comments,
        comments_url: issue.comments_url.clone(),
        created_at: issue.created_at,
        draft: issue.draft,
        events_url: issue.events_url.clone(),
        html_url: issue.html_url.clone(),
        id: issue.id,
        labels: issue_labels(issue).iter().map(label).collect(),
        labels_url: issue.labels_url.clone(),
        locked: issue.locked,
        milestone: issue.milestone.as_ref().map(milestone).transpose()?,
        node_id: issue.node_id.clone(),
        number: issue.number,
        performed_via_github_app: None,
        pull_request: issue
            .pull_request
            .as_ref()
            .map(|pull| wt::IssuePullRequest {
                diff_url: pull.diff_url.clone(),
                html_url: pull.html_url.clone(),
                merged_at: pull.merged_at,
                patch_url: pull.patch_url.clone(),
                url: pull.url.clone(),
            }),
        reactions: reactions(issue.reactions.as_ref(), &issue.url),
        repository_url: issue.repository_url.clone(),
        state: parse("issue.state", &issue.state)?,
        state_reason: issue.state_reason.as_ref().map(ToString::to_string),
        timeline_url: issue.timeline_url.clone(),
        title: issue.title.clone(),
        updated_at: issue.updated_at,
        url: issue.url.clone(),
        user: nullable_user(required("issue.user", issue.user.as_ref())?)?,
    })
}

/// The REST pull request labels as webhook labels.
pub(crate) fn pull_labels(pull: &types::PullRequest) -> Vec<wt::Label> {
    pull.labels
        .iter()
        .map(|label| wt::Label {
            color: label.color.clone(),
            default: label.default,
            description: label.description.clone(),
            id: label.id,
            name: label.name.clone(),
            node_id: label.node_id.clone(),
            url: label.url.clone(),
        })
        .collect()
}

/// A REST link as a webhook link.
pub(crate) fn link(link: &types::Link) -> wt::Link {
    wt::Link {
        href: link.href.clone(),
    }
}

/// Builds one of the webhook pull request objects, which share their fields, from the REST
/// pull request `$pull` with `$repository` as the repository of its head and base. Expands to
/// an expression that uses `?` on [`PayloadError`].
macro_rules! pull_request_object {
    ($type:ident, $base:ident, $head:ident, $links:ident, $pull:expr, $repository:expr, { $($extra:tt)* }) => {{
        let pull: &types::PullRequest = $pull;
        let repository: &wt::Repository = $repository;
        let links = &pull.links;
        wt::$type {
            active_lock_reason: pull
                .active_lock_reason
                .as_deref()
                .map(|reason| model::parse("pull_request.active_lock_reason", reason))
                .transpose()?,
            assignee: pull.assignee.as_ref().map(model::nullable_user).transpose()?,
            assignees: pull.assignees.iter().map(model::user)
            .collect::<Result<_, _>>()?,
            author_association: model::author_association(&pull.author_association)?,
            auto_merge: pull.auto_merge.as_ref().map(model::auto_merge).transpose()?,
            base: wt::$base {
                label: pull.base.label.clone(),
                ref_: pull.base.ref_.clone(),
                repo: repository.clone(),
                sha: pull.base.sha.clone(),
                user: model::user(&pull.base.user)?,
            },
            body: pull.body.clone(),
            closed_at: pull.closed_at,
            comments_url: pull.comments_url.clone(),
            commits_url: pull.commits_url.clone(),
            created_at: pull.created_at,
            diff_url: pull.diff_url.clone(),
            head: wt::$head {
                label: pull.head.label.clone(),
                ref_: pull.head.ref_.clone(),
                repo: repository.clone().into(),
                sha: pull.head.sha.clone(),
                user: model::user(&pull.head.user)?,
            },
            html_url: pull.html_url.clone(),
            id: pull.id,
            issue_url: pull.issue_url.clone(),
            labels: model::pull_labels(pull),
            links: wt::$links {
                comments: model::link(&links.comments),
                commits: model::link(&links.commits),
                html: model::link(&links.html),
                issue: model::link(&links.issue),
                review_comment: model::link(&links.review_comment),
                review_comments: model::link(&links.review_comments),
                self_: model::link(&links.self_),
                statuses: model::link(&links.statuses),
            },
            locked: pull.locked,
            merge_commit_sha: pull.merge_commit_sha.clone(),
            merged_at: pull.merged_at,
            milestone: pull.milestone.as_ref().map(model::milestone).transpose()?,
            node_id: pull.node_id.clone(),
            number: pull.number,
            patch_url: pull.patch_url.clone(),
            requested_reviewers: pull
                .requested_reviewers
                .iter()
                .map(|reviewer| model::user(reviewer).map(Into::into))
                .collect::<Result<_, _>>()?,
            requested_teams: pull
                .requested_teams
                .iter()
                .map(model::team)
                .collect::<Result<_, model::PayloadError>>()?,
            review_comment_url: pull.review_comment_url.clone(),
            review_comments_url: pull.review_comments_url.clone(),
            state: model::parse("pull_request.state", &pull.state.to_string())?,
            statuses_url: pull.statuses_url.clone(),
            title: pull.title.clone(),
            updated_at: pull.updated_at,
            url: pull.url.clone(),
            user: model::user(&pull.user)?,
            $($extra)*
        }
    }};
}
pub(crate) use pull_request_object;

/// The webhook auto-merge settings for `auto_merge`.
pub(crate) fn auto_merge(auto_merge: &types::AutoMerge) -> Result<wt::AutoMerge, PayloadError> {
    Ok(wt::AutoMerge {
        commit_message: Some(auto_merge.commit_message.clone()),
        commit_title: Some(auto_merge.commit_title.clone()),
        enabled_by: Some(user(&auto_merge.enabled_by)?),
        merge_method: parse(
            "auto_merge.merge_method",
            &auto_merge.merge_method.to_string(),
        )?,
    })
}

/// The full webhook pull request for `pull`, whose head and base are in `repository`.
pub(crate) fn pull_request(
    pull: &types::PullRequest,
    repository: &wt::Repository,
) -> Result<wt::PullRequest, PayloadError> {
    use super::model;
    Ok(
        pull_request_object!(PullRequest, PullRequestBase, PullRequestHead, PullRequestLinks, pull, repository, {
            additions: pull.additions,
            changed_files: pull.changed_files,
            comments: pull.comments,
            commits: pull.commits,
            deletions: pull.deletions,
            draft: pull.draft.unwrap_or(false),
            maintainer_can_modify: pull.maintainer_can_modify,
            mergeable: pull.mergeable,
            mergeable_state: pull.mergeable_state.clone(),
            merged: Some(pull.merged),
            merged_by: pull.merged_by.as_ref().map(model::nullable_user).transpose()?,
            rebaseable: pull.rebaseable,
            review_comments: pull.review_comments,
        }),
    )
}

/// A stand-in GitHub App owned by `owner`, for a check without an app of its own.
pub(crate) fn stub_app(owner: &wt::User, now: chrono::DateTime<chrono::Utc>) -> wt::App {
    wt::App {
        created_at: now,
        description: None,
        events: Vec::new(),
        external_url: "https://github.com/apps/ninot-github".to_owned(),
        html_url: "https://github.com/apps/ninot-github".to_owned(),
        id: 1,
        name: "ninot-github".to_owned(),
        node_id: "A_ninot".to_owned(),
        owner: owner.clone(),
        permissions: None,
        slug: None,
        updated_at: now,
    }
}

/// The webhook app for the REST `app`, or [`stub_app`] when it has no page or is owned by an
/// enterprise.
pub(crate) fn app(
    app: Option<&types::NullableIntegration>,
    sender: &wt::User,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<wt::App, PayloadError> {
    let Some(app) = app.filter(|app| !app.html_url.is_empty()) else {
        return Ok(stub_app(sender, now));
    };
    let types::NullableIntegrationOwner::SimpleUser(owner) = &app.owner else {
        return Ok(stub_app(sender, now));
    };
    Ok(wt::App {
        created_at: app.created_at,
        description: app.description.clone(),
        events: app
            .events
            .iter()
            .filter_map(|event| event.parse().ok())
            .collect(),
        external_url: app.external_url.clone(),
        html_url: app.html_url.clone(),
        id: app.id,
        name: app.name.clone(),
        node_id: app.node_id.clone(),
        owner: user(owner)?,
        permissions: Some(app_permissions(&app.permissions)),
        slug: app.slug.clone(),
        updated_at: app.updated_at,
    })
}

/// The permissions an app was granted, keeping those the webhook schema names.
fn app_permissions(granted: &types::NullableIntegrationPermissions) -> wt::AppPermissions {
    let mut permissions = wt::AppPermissions::default();
    let named = [
        ("checks", &granted.checks),
        ("contents", &granted.contents),
        ("deployments", &granted.deployments),
        ("issues", &granted.issues),
        ("metadata", &granted.metadata),
    ];
    let named = named
        .into_iter()
        .filter_map(|(key, level)| level.as_deref().map(|level| (key, level)));
    let extra = granted
        .extra
        .iter()
        .map(|(key, level)| (key.as_str(), level.as_str()));
    for (key, level) in named.chain(extra) {
        set_permission(&mut permissions, key, level);
    }
    permissions
}

/// Sets the permission `key` of `permissions` to `level`, when the schema has both.
fn set_permission(permissions: &mut wt::AppPermissions, key: &str, level: &str) {
    macro_rules! fields {
        ($($field:ident),* $(,)?) => {
            match key {
                $(stringify!($field) => permissions.$field = level.parse().ok(),)*
                _ => {}
            }
        };
    }
    fields!(
        actions,
        administration,
        blocking,
        checks,
        content_references,
        contents,
        deployments,
        discussions,
        emails,
        environments,
        followers,
        gpg_keys,
        interaction_limits,
        issues,
        keys,
        members,
        merge_queues,
        metadata,
        organization_administration,
        organization_hooks,
        organization_packages,
        organization_plan,
        organization_projects,
        organization_secrets,
        organization_self_hosted_runners,
        organization_user_blocking,
        packages,
        pages,
        plan,
        pull_requests,
        repository_hooks,
        repository_projects,
        secret_scanning_alerts,
        secrets,
        security_events,
        security_scanning_alert,
        single_file,
        starring,
        statuses,
        team_discussions,
        vulnerability_alerts,
        watching,
        workflows,
    );
}

/// `text` when it is an RFC 3339 timestamp, or the current time.
pub(crate) fn stamp(text: &str) -> String {
    crate::time::parse(text).map_or_else(|_| crate::time::now(), |_| text.to_owned())
}

/// The webhook person for a git signature.
pub(crate) fn committer(signature: &ninot_common::git::Signature) -> wt::Committer {
    wt::Committer {
        date: Some(stamp(&signature.date)),
        email: Some(signature.email.clone()),
        name: signature.name.clone(),
        username: None,
    }
}

/// The person of a commit the repository has no git object for.
pub(crate) fn stub_committer() -> wt::Committer {
    wt::Committer {
        date: Some(crate::time::now()),
        email: None,
        name: "ninot-github".to_owned(),
        username: None,
    }
}

/// The `commit-simple` object for `sha`: the git commit when the repository has it, and a stub
/// otherwise.
pub(crate) fn commit_simple(ctx: &Ctx, sha: &str) -> wt::CommitSimple {
    if let Some(dir) = ctx.git_dir()
        && let Ok(commit) = ninot_common::git::commit(&dir, sha)
    {
        return wt::CommitSimple {
            author: committer(&commit.author),
            committer: committer(&commit.committer),
            id: commit.sha,
            message: commit.message,
            timestamp: stamp(&commit.author.date),
            tree_id: commit.tree,
        };
    }
    wt::CommitSimple {
        author: stub_committer(),
        committer: stub_committer(),
        id: sha.to_owned(),
        message: String::new(),
        timestamp: crate::time::now(),
        tree_id: sha.to_owned(),
    }
}

/// The webhook form of a REST pull request reference on a check or workflow run.
pub(crate) fn minimal_pull(pull: &types::PullRequestMinimal) -> wt::CheckRunPullRequest {
    let side = |reference: &str, repo: &types::PullRequestMinimalBaseRepo, sha: &str| {
        (
            reference.to_owned(),
            wt::RepoRef {
                id: repo.id,
                name: repo.name.clone(),
                url: repo.url.clone(),
            },
            sha.to_owned(),
        )
    };
    let (base_ref, base_repo, base_sha) = side(&pull.base.ref_, &pull.base.repo, &pull.base.sha);
    let (head_ref, head_repo, head_sha) = side(
        &pull.head.ref_,
        &types::PullRequestMinimalBaseRepo {
            id: pull.head.repo.id,
            name: pull.head.repo.name.clone(),
            url: pull.head.repo.url.clone(),
        },
        &pull.head.sha,
    );
    wt::CheckRunPullRequest {
        base: wt::CheckRunPullRequestBase {
            ref_: base_ref,
            repo: base_repo,
            sha: base_sha,
        },
        head: wt::CheckRunPullRequestHead {
            ref_: head_ref,
            repo: head_repo,
            sha: head_sha,
        },
        id: pull.id,
        number: pull.number,
        url: pull.url.clone(),
    }
}

/// The current time, to the second, as the webhook timestamp types take it.
pub(crate) fn now() -> chrono::DateTime<chrono::Utc> {
    crate::time::now_utc()
}

/// The `repository-lite` object for the full webhook repository.
pub(crate) fn repository_lite(repository: &wt::Repository) -> wt::RepositoryLite {
    let r = repository;
    wt::RepositoryLite {
        archive_url: r.archive_url.clone(),
        assignees_url: r.assignees_url.clone(),
        blobs_url: r.blobs_url.clone(),
        branches_url: r.branches_url.clone(),
        collaborators_url: r.collaborators_url.clone(),
        comments_url: r.comments_url.clone(),
        commits_url: r.commits_url.clone(),
        compare_url: r.compare_url.clone(),
        contents_url: r.contents_url.clone(),
        contributors_url: r.contributors_url.clone(),
        deployments_url: r.deployments_url.clone(),
        description: r.description.clone(),
        downloads_url: r.downloads_url.clone(),
        events_url: r.events_url.clone(),
        fork: r.fork,
        forks_url: r.forks_url.clone(),
        full_name: r.full_name.clone(),
        git_commits_url: r.git_commits_url.clone(),
        git_refs_url: r.git_refs_url.clone(),
        git_tags_url: r.git_tags_url.clone(),
        hooks_url: r.hooks_url.clone(),
        html_url: r.html_url.clone(),
        id: r.id,
        issue_comment_url: r.issue_comment_url.clone(),
        issue_events_url: r.issue_events_url.clone(),
        issues_url: r.issues_url.clone(),
        keys_url: r.keys_url.clone(),
        labels_url: r.labels_url.clone(),
        languages_url: r.languages_url.clone(),
        merges_url: r.merges_url.clone(),
        milestones_url: r.milestones_url.clone(),
        name: r.name.clone(),
        node_id: r.node_id.clone(),
        notifications_url: r.notifications_url.clone(),
        owner: r.owner.clone(),
        private: r.private,
        pulls_url: r.pulls_url.clone(),
        releases_url: r.releases_url.clone(),
        stargazers_url: r.stargazers_url.clone(),
        statuses_url: r.statuses_url.clone(),
        subscribers_url: r.subscribers_url.clone(),
        subscription_url: r.subscription_url.clone(),
        tags_url: r.tags_url.clone(),
        teams_url: r.teams_url.clone(),
        trees_url: r.trees_url.clone(),
        url: r.url.clone(),
    }
}
