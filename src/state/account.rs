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
    /// Lowercase logins of an organization's members, all of whom administer its repositories.
    pub members: BTreeSet<String>,
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

/// A team of an organization, which can be asked to review pull requests.
pub struct Team {
    pub id: u64,
    pub slug: String,
    /// Lowercase logins of the team's members.
    pub members: BTreeSet<String>,
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
            AccountKind::Organization => self.members.contains(&login),
        }
    }
}
