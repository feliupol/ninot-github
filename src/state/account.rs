//! Users and organizations.

use std::collections::{BTreeMap, BTreeSet};

use super::{Repo, Runner, RunnerGroup};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccountKind {
    User,
    Organization,
}

impl AccountKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "User",
            Self::Organization => "Organization",
        }
    }
}

/// A user or an organization.
pub struct Account {
    pub id: u64,
    pub login: String,
    /// Display name shown by public profile endpoints.
    pub name: Option<String>,
    pub kind: AccountKind,
    pub created_at: String,
    /// Lowercase logins of an organization's active members, owners included.
    pub members: BTreeSet<String>,
    /// Lowercase logins of the members who own the organization and so administer everything
    /// it owns. Always a subset of `members`.
    pub owners: BTreeSet<String>,
    /// Lowercase logins invited to the organization, with the role (`admin` or `member`) offered.
    pub pending: BTreeMap<String, String>,
    /// Lowercase logins of the members who publicize their membership.
    pub public_members: BTreeSet<String>,
    /// Profile and settings an organization owner can change.
    pub profile: OrgProfile,
    /// An organization's teams by lowercase slug.
    pub teams: BTreeMap<String, Team>,
    /// Repositories by lowercase name.
    pub(crate) repos: BTreeMap<String, Repo>,
    /// Self-hosted Actions runners by id.
    pub runners: BTreeMap<u64, Runner>,
    /// Self-hosted runner groups by id.
    pub runner_groups: BTreeMap<u64, RunnerGroup>,
    /// Lowercase runner-label name to its stable id within this account.
    pub(super) runner_label_ids: BTreeMap<String, u64>,
}

/// Settings of an organization that its owners can change with `PATCH /orgs/{org}`.
#[derive(Clone, Debug, Default)]
pub struct OrgProfile {
    pub description: Option<String>,
    pub company: Option<String>,
    pub location: Option<String>,
    pub email: Option<String>,
    pub blog: Option<String>,
    pub twitter_username: Option<String>,
    pub billing_email: Option<String>,
    pub has_organization_projects: Option<bool>,
    pub has_repository_projects: Option<bool>,
    /// `read`, `write`, `admin` or `none`; `read` when unset.
    pub default_repository_permission: Option<String>,
}

/// A team of an organization, which can be asked to review pull requests.
pub struct Team {
    pub id: u64,
    pub slug: String,
    pub name: String,
    pub description: Option<String>,
    /// `secret` or `closed`.
    pub privacy: String,
    /// `notifications_enabled` or `notifications_disabled`.
    pub notification_setting: String,
    /// Default permission for repositories added to the team: `pull`, `push` or `admin`.
    pub permission: String,
    /// Id of the parent team in the same organization.
    pub parent_id: Option<u64>,
    /// Lowercase logins of the team's members, maintainers included.
    pub members: BTreeSet<String>,
    /// Lowercase logins of the members who maintain the team.
    pub maintainers: BTreeSet<String>,
    /// Lowercase logins invited to the team while not yet organization members.
    pub pending: BTreeSet<String>,
    /// Permission (`pull`, `triage`, `push`, `maintain` or `admin`) on the organization's
    /// repositories by lowercase name.
    pub repos: BTreeMap<String, String>,
    pub created_at: String,
    pub updated_at: String,
}

impl Team {
    /// An empty team with the defaults GitHub gives a new one.
    pub fn new(id: u64, slug: String, privacy: &str) -> Self {
        let stamp = super::now();
        Self {
            id,
            name: slug.clone(),
            slug,
            description: None,
            privacy: privacy.to_owned(),
            notification_setting: "notifications_enabled".to_owned(),
            permission: "pull".to_owned(),
            parent_id: None,
            members: BTreeSet::new(),
            maintainers: BTreeSet::new(),
            pending: BTreeSet::new(),
            repos: BTreeMap::new(),
            created_at: stamp.clone(),
            updated_at: stamp,
        }
    }
}

impl Account {
    pub fn team(&self, slug: &str) -> Option<&Team> {
        self.teams.get(&slug.to_lowercase())
    }

    /// Whether `login` belongs to this organization's team `slug`.
    pub fn team_has_member(&self, slug: &str, login: &str) -> bool {
        self.team(slug)
            .is_some_and(|team| team.members.contains(&login.to_lowercase()))
    }

    pub fn repo(&self, name: &str) -> Option<&Repo> {
        self.repos.get(&name.to_lowercase())
    }

    pub fn repo_mut(&mut self, name: &str) -> Option<&mut Repo> {
        self.repos.get_mut(&name.to_lowercase())
    }

    pub fn repos(&self) -> impl Iterator<Item = &Repo> {
        self.repos.values()
    }

    /// Whether `login` administers everything this account owns.
    pub fn is_administered_by(&self, login: &str) -> bool {
        let login = login.to_lowercase();
        match self.kind {
            AccountKind::User => self.login.to_lowercase() == login,
            AccountKind::Organization => self.owners.contains(&login),
        }
    }
}
