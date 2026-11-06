//! Self-hosted Actions runners, runner groups and the one-time tokens that register them.

use super::{Account, AccountKind, State};
use crate::api::ApiResponse;
use crate::time::{format_timestamp, unix_now};

/// How long a runner registration or removal token stays valid. GitHub uses one hour.
const RUNNER_TOKEN_TTL: u64 = 60 * 60;

/// A label attached to a self-hosted runner.
#[derive(Clone, Debug)]
pub struct RunnerLabel {
    pub id: u64,
    pub name: String,
    /// System labels such as `self-hosted` and the operating system cannot be removed.
    pub read_only: bool,
}

/// A self-hosted Actions runner registered to an organization.
#[derive(Clone, Debug)]
pub struct Runner {
    pub id: u64,
    pub name: String,
    pub os: String,
    /// `online` or `offline`.
    pub status: String,
    pub busy: bool,
    /// Just-in-time runners are ephemeral; configured runners are not.
    pub ephemeral: bool,
    pub labels: Vec<RunnerLabel>,
    pub group_id: u64,
}

/// A group of self-hosted runners. Every organization has one default group.
#[derive(Debug)]
pub struct RunnerGroup {
    pub id: u64,
    pub name: String,
    /// `all`, `selected`, or `private`.
    pub visibility: String,
    pub is_default: bool,
    pub allows_public_repositories: bool,
    pub restricted_to_workflows: bool,
    pub selected_workflows: Vec<String>,
    pub network_configuration_id: Option<String>,
    pub repository_ids: Vec<u64>,
}

/// Which runner application a short-lived token authorizes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RunnerTokenKind {
    Registration,
    Removal,
}

pub(super) struct RunnerToken {
    /// Lowercase organization login.
    org: String,
    kind: RunnerTokenKind,
    expires_unix: u64,
    used: bool,
}

/// Fields of a runner being added to an organization.
pub(crate) struct RunnerSpec<'a> {
    pub name: &'a str,
    pub os: &'a str,
    pub status: &'a str,
    pub ephemeral: bool,
    pub group_id: u64,
    pub labels: &'a [String],
    /// Operating system used to add the read-only OS label.
    pub label_os: Option<&'a str>,
}

#[derive(Clone, Copy)]
pub(crate) enum CreateRunnerError {
    InvalidName,
    DuplicateName,
    UnknownGroup,
}

impl CreateRunnerError {
    const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidName => "runner name is invalid",
            Self::DuplicateName => "a runner with this name already exists",
            Self::UnknownGroup => "unknown runner group",
        }
    }
}

/// Letters, digits and a few punctuation marks. GitHub caps runner names at 64 characters.
pub(crate) fn valid_runner_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name
            .chars()
            .any(|character| character.is_control() || character == '/')
}

pub(crate) fn valid_label_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 255 && !name.chars().any(char::is_control)
}

/// GitHub stores the operating system in lowercase.
pub(crate) fn canonical_os(os: &str) -> String {
    match os.to_ascii_lowercase().as_str() {
        "linux" => "linux".to_owned(),
        "macos" | "mac" | "osx" | "darwin" => "macos".to_owned(),
        "windows" | "win" => "windows".to_owned(),
        _ => os.to_owned(),
    }
}

fn canonical_system_label(name: &str) -> Option<&'static str> {
    match name.to_ascii_lowercase().as_str() {
        "self-hosted" => Some("self-hosted"),
        "linux" => Some("Linux"),
        "macos" | "mac" | "osx" | "darwin" => Some("macOS"),
        "windows" | "win" => Some("Windows"),
        "x64" | "amd64" => Some("X64"),
        "arm" => Some("ARM"),
        "arm64" | "aarch64" => Some("ARM64"),
        _ => None,
    }
}

fn os_label(os: &str) -> Option<&'static str> {
    match os.to_ascii_lowercase().as_str() {
        "linux" => Some("Linux"),
        "macos" | "mac" | "osx" | "darwin" => Some("macOS"),
        "windows" | "win" => Some("Windows"),
        _ => None,
    }
}

fn push_label(specs: &mut Vec<(String, bool)>, name: &str, read_only: bool) {
    if !valid_label_name(name) {
        return;
    }
    let (display, read_only) = match canonical_system_label(name) {
        Some(canonical) => (canonical.to_owned(), true),
        None => (name.to_owned(), read_only),
    };
    if let Some((existing, existing_read_only)) = specs
        .iter_mut()
        .find(|(existing, _)| existing.eq_ignore_ascii_case(&display))
    {
        if read_only {
            *existing = display;
            *existing_read_only = true;
        }
        return;
    }
    specs.push((display, read_only));
}

impl Account {
    /// The id of the default self-hosted runner group, once one exists.
    pub fn default_runner_group_id(&self) -> Option<u64> {
        self.runner_groups
            .values()
            .find(|group| group.is_default)
            .map(|group| group.id)
    }
}

impl State {
    /// Creates the organization's default runner group, if it does not exist yet, and returns its id.
    pub(crate) fn ensure_default_runner_group(&mut self, org: &str) -> u64 {
        if let Some(id) = self.account(org).and_then(Account::default_runner_group_id) {
            return id;
        }
        let id = self.next_id();
        self.account_mut(org).unwrap().runner_groups.insert(
            id,
            RunnerGroup {
                id,
                name: "Default".to_owned(),
                visibility: "all".to_owned(),
                is_default: true,
                allows_public_repositories: false,
                restricted_to_workflows: false,
                selected_workflows: Vec::new(),
                network_configuration_id: None,
                repository_ids: Vec::new(),
            },
        );
        id
    }

    /// Issues a one-time runner token that expires after [`RUNNER_TOKEN_TTL`].
    pub(crate) fn issue_runner_token(
        &mut self,
        org: &str,
        kind: RunnerTokenKind,
    ) -> (String, String) {
        let expires_unix = unix_now().saturating_add(RUNNER_TOKEN_TTL);
        let expires_at = format_timestamp(expires_unix);
        let token = ninot_common::token::new_token(crate::server::TOKEN_PREFIX);
        self.runner_tokens.insert(
            token.clone(),
            RunnerToken {
                org: org.to_lowercase(),
                kind,
                expires_unix,
                used: false,
            },
        );
        (token, expires_at)
    }

    /// The lowercase organization an unexpired runner token of `kind` belongs to. Does not
    /// consume the token: the Actions runner sends it on every configuration request.
    pub(crate) fn runner_token_org(&self, token: &str, kind: RunnerTokenKind) -> Option<String> {
        self.runner_tokens
            .get(token)
            .filter(|found| found.kind == kind && found.expires_unix > unix_now())
            .map(|found| found.org.clone())
    }

    /// Registers a self-hosted runner with a one-time registration token.
    ///
    /// The runner is online, carries `labels` plus the read-only system labels, and belongs to
    /// the organization's default runner group. Reusing `registration_token` fails.
    pub fn register_runner(
        &mut self,
        org: &str,
        registration_token: &str,
        name: &str,
        labels: &[&str],
        os: &str,
    ) -> Result<u64, &'static str> {
        let org_key = org.to_lowercase();
        match self.account(&org_key) {
            Some(account) if account.kind == AccountKind::Organization => {}
            _ => return Err("unknown organization"),
        }
        let (kind, token_org, used, expires_unix) = match self.runner_tokens.get(registration_token)
        {
            Some(token) => (
                token.kind,
                token.org.clone(),
                token.used,
                token.expires_unix,
            ),
            None => return Err("registration token is invalid"),
        };
        if kind != RunnerTokenKind::Registration || token_org != org_key {
            return Err("registration token is invalid");
        }
        if used {
            return Err("registration token has already been used");
        }
        if expires_unix <= unix_now() {
            return Err("registration token has expired");
        }
        if !valid_runner_name(name) {
            return Err("runner name is invalid");
        }
        if os.is_empty() || os.chars().any(char::is_control) {
            return Err("runner os is invalid");
        }
        if labels.iter().any(|label| !valid_label_name(label)) {
            return Err("runner label is invalid");
        }
        let os = canonical_os(os);
        let names: Vec<String> = labels.iter().map(|label| (*label).to_owned()).collect();
        let group_id = self.ensure_default_runner_group(&org_key);
        let id = self
            .insert_runner(
                &org_key,
                RunnerSpec {
                    name,
                    os: &os,
                    status: "online",
                    ephemeral: false,
                    group_id,
                    labels: &names,
                    label_os: Some(&os),
                },
            )
            .map_err(CreateRunnerError::as_str)?;
        self.runner_tokens.get_mut(registration_token).unwrap().used = true;
        Ok(id)
    }

    /// Marks a registered runner offline, as if it had disconnected.
    pub fn set_runner_offline(&mut self, org: &str, runner_id: u64) -> Result<(), &'static str> {
        let Some(account) = self.account_mut(org) else {
            return Err("unknown organization");
        };
        if account.kind != AccountKind::Organization {
            return Err("unknown organization");
        }
        let Some(runner) = account.runners.get_mut(&runner_id) else {
            return Err("unknown runner");
        };
        runner.status = "offline".to_owned();
        Ok(())
    }

    pub(crate) fn insert_runner(
        &mut self,
        org: &str,
        spec: RunnerSpec<'_>,
    ) -> Result<u64, CreateRunnerError> {
        if !valid_runner_name(spec.name) {
            return Err(CreateRunnerError::InvalidName);
        }
        if !self
            .account(org)
            .is_some_and(|account| account.runner_groups.contains_key(&spec.group_id))
        {
            return Err(CreateRunnerError::UnknownGroup);
        }
        if self.account(org).is_some_and(|account| {
            account
                .runners
                .values()
                .any(|runner| runner.name.eq_ignore_ascii_case(spec.name))
        }) {
            return Err(CreateRunnerError::DuplicateName);
        }
        let labels = self.runner_labels(org, spec.labels, spec.label_os);
        let id = self.next_id();
        self.account_mut(org).unwrap().runners.insert(
            id,
            Runner {
                id,
                name: spec.name.to_owned(),
                os: spec.os.to_owned(),
                status: spec.status.to_owned(),
                busy: false,
                ephemeral: spec.ephemeral,
                labels,
                group_id: spec.group_id,
            },
        );
        Ok(id)
    }

    /// Adds `names` to a runner's labels. When `replace_custom` is set, custom labels are
    /// replaced and read-only labels stay.
    pub(crate) fn assign_runner_labels(
        &mut self,
        org: &str,
        runner_id: u64,
        names: &[String],
        replace_custom: bool,
    ) -> Result<Vec<RunnerLabel>, ApiResponse> {
        let existing = self
            .account(org)
            .and_then(|account| account.runners.get(&runner_id))
            .map(|runner| runner.labels.clone())
            .ok_or_else(ApiResponse::not_found)?;
        let mut specs: Vec<(String, bool)> = existing
            .into_iter()
            .filter(|label| !replace_custom || label.read_only)
            .map(|label| (label.name, label.read_only))
            .collect();
        for name in names {
            push_label(&mut specs, name, false);
        }
        let labels = self.materialize_labels(org, specs);
        self.account_mut(org)
            .unwrap()
            .runners
            .get_mut(&runner_id)
            .unwrap()
            .labels = labels.clone();
        Ok(labels)
    }

    /// Removes one custom label. A read-only label is rejected and a missing one is 404.
    pub(crate) fn remove_runner_label(
        &mut self,
        org: &str,
        runner_id: u64,
        name: &str,
    ) -> Result<Vec<RunnerLabel>, ApiResponse> {
        let account = self.account_mut(org).ok_or_else(ApiResponse::not_found)?;
        let runner = account
            .runners
            .get_mut(&runner_id)
            .ok_or_else(ApiResponse::not_found)?;
        let Some(index) = runner
            .labels
            .iter()
            .position(|label| label.name.eq_ignore_ascii_case(name))
        else {
            return Err(ApiResponse::not_found());
        };
        if runner.labels[index].read_only {
            return Err(ApiResponse::validation_failed("Runner", "name", "invalid"));
        }
        runner.labels.remove(index);
        Ok(runner.labels.clone())
    }

    fn runner_labels(&mut self, org: &str, names: &[String], os: Option<&str>) -> Vec<RunnerLabel> {
        let mut specs = Vec::new();
        push_label(&mut specs, "self-hosted", true);
        if let Some(label) = os.and_then(os_label) {
            push_label(&mut specs, label, true);
        }
        for name in names {
            push_label(&mut specs, name, false);
        }
        self.materialize_labels(org, specs)
    }

    fn materialize_labels(&mut self, org: &str, specs: Vec<(String, bool)>) -> Vec<RunnerLabel> {
        let mut labels = Vec::with_capacity(specs.len());
        for (name, read_only) in specs {
            labels.push(self.make_label(org, name, read_only));
        }
        labels
    }

    fn make_label(&mut self, org: &str, name: String, read_only: bool) -> RunnerLabel {
        let key = name.to_lowercase();
        let id = match self
            .account(org)
            .unwrap()
            .runner_label_ids
            .get(&key)
            .copied()
        {
            Some(id) => id,
            None => {
                let id = self.next_id();
                self.account_mut(org)
                    .unwrap()
                    .runner_label_ids
                    .insert(key, id);
                id
            }
        };
        RunnerLabel {
            id,
            name,
            read_only,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_tokens_are_one_time_and_expire() {
        let mut state = State::default();
        state.add_user("alice");
        state.add_org("acme", &["alice"]);
        let (token, _) = state.issue_runner_token("acme", RunnerTokenKind::Registration);
        let id = state
            .register_runner("acme", &token, "box", &["gpu"], "linux")
            .unwrap();
        let runner = &state.account("acme").unwrap().runners[&id];
        assert_eq!(runner.os, "linux");
        assert_eq!(runner.status, "online");
        assert!(!runner.busy);
        assert!(
            runner
                .labels
                .iter()
                .any(|label| label.name == "gpu" && !label.read_only)
        );
        assert!(
            runner
                .labels
                .iter()
                .any(|label| label.name == "self-hosted" && label.read_only)
        );
        assert!(
            runner
                .labels
                .iter()
                .any(|label| label.name == "Linux" && label.read_only)
        );
        assert_eq!(
            state.register_runner("acme", &token, "other", &[], "linux"),
            Err("registration token has already been used")
        );
        state.set_runner_offline("acme", id).unwrap();
        assert_eq!(
            state.account("acme").unwrap().runners[&id].status,
            "offline"
        );

        let (token, _) = state.issue_runner_token("acme", RunnerTokenKind::Registration);
        state.runner_tokens.get_mut(&token).unwrap().expires_unix = 0;
        assert_eq!(
            state.register_runner("acme", &token, "late", &[], "linux"),
            Err("registration token has expired")
        );
        let (token, _) = state.issue_runner_token("acme", RunnerTokenKind::Removal);
        assert_eq!(
            state.register_runner("acme", &token, "late", &[], "linux"),
            Err("registration token is invalid")
        );
    }
}
