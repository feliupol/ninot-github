//! Organization membership and the repository access it grants.

use super::{Account, AccountKind, Permission, State};

/// Repository permission names, weakest first.
pub(crate) const TEAM_PERMISSIONS: [&str; 5] = ["pull", "triage", "push", "maintain", "admin"];

/// How a login relates to an organization.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrgRole {
    Owner,
    Member,
    Outsider,
}

impl Permission {
    /// What a team permission name allows; `None` for a name GitHub does not know.
    pub(crate) fn from_team_permission(name: &str) -> Option<Self> {
        match name {
            "pull" | "triage" => Some(Self {
                admin: false,
                push: false,
                pull: true,
            }),
            "push" | "maintain" => Some(Self {
                admin: false,
                push: true,
                pull: true,
            }),
            "admin" => Some(Self::ADMIN),
            _ => None,
        }
    }
}

impl Account {
    /// How `login` relates to this organization.
    pub fn role_of(&self, login: &str) -> OrgRole {
        let login = login.to_lowercase();
        if self.owners.contains(&login) {
            OrgRole::Owner
        } else if self.members.contains(&login) {
            OrgRole::Member
        } else {
            OrgRole::Outsider
        }
    }

    /// Whether `login` may see the team `slug`: owners see every team, members the closed ones
    /// and the secret ones they belong to.
    pub fn team_visible_to(&self, slug: &str, login: Option<&str>) -> bool {
        let (Some(team), Some(login)) = (self.team(slug), login) else {
            return false;
        };
        match self.role_of(login) {
            OrgRole::Owner => true,
            OrgRole::Member => {
                team.privacy != "secret" || team.members.contains(&login.to_lowercase())
            }
            OrgRole::Outsider => false,
        }
    }

    /// Whether `login` owns the organization or maintains the team `slug`.
    pub fn manages_team(&self, slug: &str, login: &str) -> bool {
        self.role_of(login) == OrgRole::Owner
            || self
                .team(slug)
                .is_some_and(|team| team.maintainers.contains(&login.to_lowercase()))
    }

    /// Slugs of the teams below `slug`, at any depth.
    pub fn descendant_slugs(&self, slug: &str) -> Vec<String> {
        let mut found: Vec<String> = Vec::new();
        let mut frontier: Vec<u64> = self.team(slug).map(|team| team.id).into_iter().collect();
        while let Some(parent) = frontier.pop() {
            for child in self
                .teams
                .values()
                .filter(|team| team.parent_id == Some(parent))
            {
                if !found.contains(&child.slug) {
                    found.push(child.slug.clone());
                    frontier.push(child.id);
                }
            }
        }
        found
    }

    /// The strongest permission any team of `login` grants on `repo`, own or inherited from a
    /// parent team.
    fn team_grant(&self, login: &str, repo: &str) -> Option<usize> {
        let login = login.to_lowercase();
        let repo = repo.to_lowercase();
        let mut best = None;
        for team in self
            .teams
            .values()
            .filter(|team| team.members.contains(&login))
        {
            let mut current = Some(team);
            while let Some(link) = current {
                if let Some(name) = link.repos.get(&repo)
                    && let Some(rank) = TEAM_PERMISSIONS.iter().position(|known| known == name)
                {
                    best = best.max(Some(rank));
                }
                current = link
                    .parent_id
                    .and_then(|id| self.teams.values().find(|candidate| candidate.id == id));
            }
        }
        best
    }

    /// The permission `login` holds on `repo` through membership of this organization, or `None`
    /// when they hold none: the default permission of members, raised by team grants.
    pub(crate) fn member_permission(&self, login: &str, repo: &str) -> Option<Permission> {
        if self.kind != AccountKind::Organization {
            return None;
        }
        let team = self.team_grant(login, repo);
        let base = match self.role_of(login) {
            OrgRole::Owner => return Some(Permission::ADMIN),
            OrgRole::Member => match self.profile.default_repository_permission.as_deref() {
                Some("none") => None,
                Some("write") => Some(2),
                Some("admin") => Some(4),
                _ => Some(0),
            },
            OrgRole::Outsider => None,
        };
        let rank = base.max(team)?;
        Permission::from_team_permission(TEAM_PERMISSIONS[rank])
    }
}

impl State {
    /// Makes the membership of `login` in `org` public or concealed.
    pub fn set_public_member(&mut self, org: &str, login: &str, public: bool) {
        if let Some(account) = self.account_mut(org) {
            let login = login.to_lowercase();
            if public && account.members.contains(&login) {
                account.public_members.insert(login);
            } else {
                account.public_members.remove(&login);
            }
        }
    }

    /// Removes `login` from `org`: membership, ownership, invitation and every team.
    pub(crate) fn remove_org_member(&mut self, org: &str, login: &str) {
        let login = login.to_lowercase();
        if let Some(account) = self.account_mut(org) {
            account.members.remove(&login);
            account.owners.remove(&login);
            account.public_members.remove(&login);
            account.pending.remove(&login);
            for team in account.teams.values_mut() {
                team.members.remove(&login);
                team.maintainers.remove(&login);
                team.pending.remove(&login);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn org() -> State {
        let mut state = State::default();
        state.add_user("owner");
        state.add_user("dev");
        state.add_user("guest");
        state.add_org("acme", &["owner"]);
        let acme = state.account_mut("acme").unwrap();
        acme.members.insert("dev".to_owned());
        state
    }

    #[test]
    fn members_read_everything_and_teams_raise_it() {
        let mut state = org();
        state.add_team("acme", "core", &["dev"]);
        let acme = state.account_mut("acme").unwrap();
        assert_eq!(
            acme.member_permission("dev", "app"),
            Permission::from_team_permission("pull")
        );
        acme.teams
            .get_mut("core")
            .unwrap()
            .repos
            .insert("app".to_owned(), "push".to_owned());
        assert!(acme.member_permission("dev", "app").unwrap().push);
        assert!(!acme.member_permission("dev", "app").unwrap().admin);
        assert_eq!(acme.member_permission("guest", "app"), None);
        assert!(acme.member_permission("owner", "app").unwrap().admin);
    }

    #[test]
    fn a_child_team_inherits_the_parent_grant() {
        let mut state = org();
        let parent = state.add_team("acme", "parent", &[]).id;
        state.add_team("acme", "child", &["dev"]);
        let acme = state.account_mut("acme").unwrap();
        acme.profile.default_repository_permission = Some("none".to_owned());
        assert_eq!(acme.member_permission("dev", "app"), None);
        acme.teams
            .get_mut("parent")
            .unwrap()
            .repos
            .insert("app".to_owned(), "admin".to_owned());
        acme.teams.get_mut("child").unwrap().parent_id = Some(parent);
        assert!(acme.member_permission("dev", "app").unwrap().admin);
        assert_eq!(acme.descendant_slugs("parent"), ["child"]);
    }

    #[test]
    fn only_members_can_publicize() {
        let mut state = org();
        state.set_public_member("acme", "dev", true);
        state.set_public_member("acme", "guest", true);
        let acme = state.account("acme").unwrap();
        assert!(acme.public_members.contains("dev"));
        assert!(!acme.public_members.contains("guest"));
    }
}
