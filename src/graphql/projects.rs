//! Project v2 GraphQL objects and the `Issue` of a project item.

use serde::Serialize;

use super::issue_view::issue_from_stored;
use super::objects::{Connection, Issue, NodeRef, SubIssuesSummary};
use super::query::repository_object;
use super::types::ProjectV2ItemType;
use super::{Env, Map, MutationArgs, Output, database_id};
use crate::render::node_id;
use crate::state::{ProjectItem, State, lock};

pub(super) const RESOLVERS: &[(&str, super::Resolver)] = &[
    (
        "Mutation.updateProjectV2ItemFieldValue",
        update_project_item_field_value,
    ),
    ("Mutation.addProjectV2ItemById", add_project_item_by_id),
];

/// A `ProjectV2` board.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ProjectV2 {
    #[serde(rename = "__typename")]
    pub typename: &'static str,
    pub id: String,
    pub database_id: u64,
    pub title: String,
    pub number: u64,
    pub closed: bool,
    pub fields: Connection<ProjectV2SingleSelectField>,
    pub items: Connection<ProjectV2Item>,
}

/// A `ProjectV2SingleSelectField`.
#[derive(Serialize)]
pub(super) struct ProjectV2SingleSelectField {
    #[serde(rename = "__typename")]
    pub typename: &'static str,
    pub id: String,
    pub name: String,
    pub options: Vec<ProjectV2SingleSelectFieldOption>,
}

/// A `ProjectV2SingleSelectFieldOption`.
#[derive(Serialize)]
pub(super) struct ProjectV2SingleSelectFieldOption {
    pub id: String,
    pub name: String,
}

/// A `ProjectV2Item` holding an issue.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ProjectV2Item {
    #[serde(rename = "__typename")]
    pub typename: &'static str,
    pub id: String,
    #[serde(rename = "type")]
    pub type_: ProjectV2ItemType,
    pub content: Issue,
    pub field_values: Connection<ProjectV2ItemFieldSingleSelectValue>,
    pub is_archived: bool,
}

/// A `ProjectV2ItemFieldSingleSelectValue`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ProjectV2ItemFieldSingleSelectValue {
    #[serde(rename = "__typename")]
    pub typename: &'static str,
    pub id: String,
    pub name: String,
    pub option_id: String,
    pub field: FieldName,
}

/// A project field named only by its name.
#[derive(Serialize)]
pub(super) struct FieldName {
    #[serde(rename = "__typename")]
    pub typename: &'static str,
    pub name: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectItemPayload {
    #[serde(rename = "__typename")]
    typename: &'static str,
    project_v2_item: NodeRef,
}

/// The `Issue` of a project item, a placeholder when the issue no longer exists.
pub(super) fn issue_object(env: &Env, state: &State, item: &ProjectItem) -> Issue {
    let number = item.issue_number;
    if let Some(account) = state.account(&item.owner)
        && let Some(repo) = account.repo(&item.repository)
        && let Some(issue) = repo.issues.get(&number)
    {
        return issue_from_stored(env, state, account, repo, issue);
    }
    let account = state.account(&item.owner);
    Issue {
        typename: "Issue",
        id: node_id("Issue", number),
        database_id: Some(number),
        number,
        title: format!("Issue #{number}"),
        body: String::new(),
        state: super::types::IssueState::Open,
        state_reason: None,
        url: env
            .urls()
            .repo(&item.owner, &item.repository)
            .issue(number)
            .html,
        closed: false,
        locked: false,
        active_lock_reason: None,
        is_pinned: false,
        created_at: String::new(),
        updated_at: String::new(),
        closed_at: None,
        author: None,
        author_association: super::types::CommentAuthorAssociation::None,
        milestone: None,
        repository: account
            .and_then(|account| {
                account
                    .repo(&item.repository)
                    .map(|repo| repository_object(env, account, repo))
            })
            .unwrap_or_else(|| empty_repository(&item.owner, &item.repository)),
        labels: Connection::complete(Vec::new()),
        comments: Connection::complete(Vec::new()),
        assignees: Connection::complete(Vec::new()),
        assigned_actors: Connection::complete(Vec::new()),
        reaction_groups: Vec::new(),
        project_cards: Connection::complete(Vec::new()),
        project_items: Connection::complete(Vec::new()),
        issue_type: None,
        parent: None,
        sub_issues: Connection::complete(Vec::new()),
        sub_issues_summary: SubIssuesSummary::empty(),
        blocked_by: Connection::complete(Vec::new()),
        blocking: Connection::complete(Vec::new()),
        closed_by_pull_requests_references: Connection::complete(Vec::new()),
        linked_branches: Connection::complete(Vec::new()),
    }
}

fn empty_repository(owner: &str, name: &str) -> super::objects::Repository {
    use super::objects::{Account, Repository};
    use super::types::RepositoryVisibility;
    Repository {
        typename: "Repository",
        id: String::new(),
        database_id: 0,
        name: name.to_owned(),
        name_with_owner: format!("{owner}/{name}"),
        owner: Account {
            typename: "User",
            id: String::new(),
            database_id: 0,
            login: owner.to_owned(),
            name: None,
            url: String::new(),
            created_at: String::new(),
        },
        description: None,
        url: String::new(),
        ssh_url: String::new(),
        is_private: false,
        visibility: RepositoryVisibility::Public,
        is_fork: false,
        is_archived: false,
        is_template: false,
        is_mirror: false,
        parent: None,
        created_at: String::new(),
        updated_at: String::new(),
        pushed_at: None,
        has_issues_enabled: true,
        has_wiki_enabled: true,
        merge_commit_allowed: true,
        rebase_merge_allowed: true,
        squash_merge_allowed: true,
    }
}

/// A `ProjectV2Item` of `project` with its issue and single-select values.
fn project_item_object(
    env: &Env,
    state: &State,
    project: &crate::state::Project,
    item: &ProjectItem,
) -> ProjectV2Item {
    let field_values = item
        .single_select_values
        .iter()
        .filter_map(|(field_id, option_id)| {
            let field = project.fields.get(field_id)?;
            let option = field.options.get(option_id)?;
            Some(ProjectV2ItemFieldSingleSelectValue {
                typename: "ProjectV2ItemFieldSingleSelectValue",
                id: node_id(
                    "ProjectV2ItemFieldSingleSelectValue",
                    item.id.wrapping_shl(32).wrapping_add(*field_id),
                ),
                name: option.clone(),
                option_id: node_id("ProjectV2SingleSelectFieldOption", *option_id),
                field: FieldName {
                    typename: "ProjectV2SingleSelectField",
                    name: field.name.clone(),
                },
            })
        })
        .collect();
    ProjectV2Item {
        typename: "ProjectV2Item",
        id: node_id("ProjectV2Item", item.id),
        type_: ProjectV2ItemType::Issue,
        content: issue_object(env, state, item),
        field_values: Connection::complete(field_values),
        is_archived: false,
    }
}

/// A `ProjectV2` board with its single-select fields and issue items.
pub(super) fn project_object(
    env: &Env,
    state: &State,
    project: &crate::state::Project,
) -> ProjectV2 {
    let fields = project
        .fields
        .values()
        .map(|field| ProjectV2SingleSelectField {
            typename: "ProjectV2SingleSelectField",
            id: node_id("ProjectV2SingleSelectField", field.id),
            name: field.name.clone(),
            options: field
                .options
                .iter()
                .map(|(id, name)| ProjectV2SingleSelectFieldOption {
                    id: node_id("ProjectV2SingleSelectFieldOption", *id),
                    name: name.clone(),
                })
                .collect(),
        })
        .collect();
    let items = project
        .items
        .values()
        .map(|item| project_item_object(env, state, project, item))
        .collect();
    ProjectV2 {
        typename: "ProjectV2",
        id: node_id("ProjectV2", project.id),
        database_id: project.id,
        title: project.title.clone(),
        number: project.number,
        closed: false,
        fields: Connection::complete(fields),
        items: Connection::complete(items),
    }
}

fn update_project_item_field_value(
    env: &Env,
    _: &Map,
    args: &Map,
) -> Result<Output, async_graphql::Error> {
    let MutationArgs { input } =
        super::read::<MutationArgs<super::types::UpdateProjectV2ItemFieldValueInput>>(args)?;
    let option_id = input
        .value
        .single_select_option_id
        .as_deref()
        .unwrap_or_default();
    let (Some(project_id), Some(item_id), Some(field_id), Some(option_id)) = (
        database_id(&input.project_id),
        database_id(&input.item_id),
        database_id(&input.field_id),
        database_id(option_id),
    ) else {
        return Err(super::errors::resolve_project_field_value());
    };
    let mut state = lock(&env.state);
    let Some(project) = state.projects.get_mut(&project_id) else {
        return Err(super::errors::resolve_project(project_id));
    };
    let Some(item) = project.items.get_mut(&item_id) else {
        return Err(super::errors::resolve_project_item(item_id));
    };
    if !project
        .fields
        .get(&field_id)
        .is_some_and(|field| field.options.contains_key(&option_id))
    {
        return Err(super::errors::resolve_project_option(option_id));
    }
    item.single_select_values.insert(field_id, option_id);
    let item = NodeRef {
        typename: "ProjectV2Item",
        id: node_id("ProjectV2Item", item_id),
    };
    Output::of(ProjectItemPayload {
        typename: "UpdateProjectV2ItemFieldValuePayload",
        project_v2_item: item,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AddProjectItemPayload {
    #[serde(rename = "__typename")]
    typename: &'static str,
    item: ProjectV2Item,
}

/// `Mutation.addProjectV2ItemById` for an issue; the item an issue already has is returned
/// as it is.
fn add_project_item_by_id(env: &Env, _: &Map, args: &Map) -> Result<Output, async_graphql::Error> {
    let MutationArgs { input } =
        super::read::<MutationArgs<super::types::AddProjectV2ItemByIdInput>>(args)?;
    let Some(project_id) = database_id(&input.project_id) else {
        return Err(super::errors::resolve_project(&input.project_id));
    };
    let mut state = lock(&env.state);
    if !state.projects.contains_key(&project_id) {
        return Err(super::errors::resolve_project(project_id));
    }
    let Some((owner, repository, number)) =
        super::issues::issue_location(&state, &input.content_id)
    else {
        return Err(super::errors::resolve_node(&input.content_id));
    };
    let existing = state.projects[&project_id].items.values().find_map(|item| {
        (item.owner == owner && item.repository == repository && item.issue_number == number)
            .then_some(item.id)
    });
    let item_id = existing.unwrap_or_else(|| {
        state.add_project_item(
            project_id,
            &owner,
            &repository,
            number,
            std::collections::BTreeMap::new(),
        )
    });
    let project = &state.projects[&project_id];
    Output::of(AddProjectItemPayload {
        typename: "AddProjectV2ItemByIdPayload",
        item: project_item_object(env, &state, project, &project.items[&item_id]),
    })
}
