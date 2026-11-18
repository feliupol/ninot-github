//! Repository administration settings: topics, Actions permissions, deployment environments
//! and rulesets, and the billing plan that limits them on private repositories.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};

use crate::types;

/// A billing plan of an account. Unset means no plan limits and no `plan` in the
/// organization response.
// Source: <https://docs.github.com/en/get-started/learning-about-github/githubs-plans>
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plan {
    Free,
    Pro,
    Team,
    Enterprise,
}

impl Plan {
    /// The plan's `name` in the REST API.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Free => "free",
            Self::Pro => "pro",
            Self::Team => "team",
            Self::Enterprise => "enterprise",
        }
    }

    /// Parses a plan `name`.
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "free" => Some(Self::Free),
            "pro" => Some(Self::Pro),
            "team" => Some(Self::Team),
            "enterprise" => Some(Self::Enterprise),
            _ => None,
        }
    }

    /// Whether private repositories get rulesets.
    // Source: <https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/managing-rulesets/about-rulesets>
    pub fn private_rulesets(self) -> bool {
        self != Self::Free
    }

    /// Whether private repositories get the required reviewers protection rule. Their
    /// environments exist on every plan: a Free organization's private repository takes an
    /// environment and refuses only its reviewers, with a 422.
    // Source: <https://docs.github.com/en/actions/reference/workflows-and-actions/deployments-and-environments#required-reviewers>
    pub fn private_required_reviewers(self) -> bool {
        self == Self::Enterprise
    }
}

/// Settings of a repository changed through its administration endpoints.
#[derive(Clone, Debug, Default)]
pub struct RepoSetup {
    /// Topics, lowercase, in the order last set.
    pub topics: Vec<String>,
    pub actions: ActionsPermissions,
    /// Deployment environments by name.
    pub environments: BTreeMap<String, Environment>,
    /// Rulesets by id.
    pub rulesets: BTreeMap<i64, Ruleset>,
}

/// The repository's GitHub Actions policy.
#[derive(Clone, Debug)]
pub struct ActionsPermissions {
    pub enabled: bool,
    pub allowed_actions: types::AllowedActions,
    pub default_workflow_permissions: types::ActionsDefaultWorkflowPermissions,
    pub can_approve_pull_request_reviews: bool,
}

impl Default for ActionsPermissions {
    /// What GitHub gives a new repository.
    fn default() -> Self {
        Self {
            enabled: true,
            allowed_actions: types::AllowedActions::All,
            default_workflow_permissions: types::ActionsDefaultWorkflowPermissions::Read,
            can_approve_pull_request_reviews: false,
        }
    }
}

/// A deployment environment.
#[derive(Clone, Debug)]
pub struct Environment {
    pub id: u64,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Id of the required reviewers rule, kept while the environment has reviewers.
    pub reviewers_rule_id: u64,
    /// Ids of the user reviewers.
    pub reviewers: Vec<u64>,
    pub prevent_self_review: bool,
    /// Id of the branch policy rule, shown while `branch_policy` is set.
    pub branch_policy_rule_id: u64,
    /// `None` lets any branch deploy.
    pub branch_policy: Option<types::DeploymentBranchPolicySettings>,
    /// Custom deployment branch and tag policies by id.
    pub policies: BTreeMap<u64, DeploymentPolicy>,
    /// Encrypted secrets by name.
    pub secrets: BTreeMap<String, EnvironmentSecret>,
}

/// A name pattern that a branch or tag must match to deploy to an environment.
#[derive(Clone, Debug)]
pub struct DeploymentPolicy {
    pub id: u64,
    pub name: String,
    pub type_: types::DeploymentBranchPolicyType,
}

/// An encrypted environment secret.
#[derive(Clone, Debug)]
pub struct EnvironmentSecret {
    pub name: String,
    pub encrypted_value: String,
    pub key_id: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A repository ruleset as created or last updated.
#[derive(Clone, Debug)]
pub struct Ruleset {
    pub id: i64,
    pub name: String,
    pub target: types::RepositoryRulesetTarget,
    pub enforcement: types::RepositoryRuleEnforcement,
    pub bypass_actors: Vec<types::RepositoryRulesetBypassActor>,
    pub conditions: Option<types::RepositoryRulesetConditions>,
    pub rules: Vec<types::RepositoryRule>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
