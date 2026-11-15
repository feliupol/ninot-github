//! Check runs, their annotations, and commit statuses, as stored. The REST models, their URLs,
//! counts and combined states are built from these when a request reads them.
//!
//! Commits are keyed by the text the run or status was created for, resolved to a full SHA when
//! git knows it. That is not always a [`super::Sha`]: the fake accepts refs git cannot resolve,
//! such as a fixture's `commit("abc")`, and keeps them as given.

use chrono::{DateTime, Utc};

use crate::types;

/// One check run.
#[derive(Clone)]
pub struct CheckRunRecord {
    /// Check run id.
    pub id: i64,
    /// Name, unique per suite only among the runs GitHub treats as the latest.
    pub name: String,
    /// Commit the run was created for, which lists it.
    pub head_sha: String,
    /// Suite the run belongs to; a fixture may seed a run without one.
    pub suite_id: Option<i64>,
    /// GitHub App that owns the run.
    pub app: Option<CheckRunApp>,
    /// `queued`, `in_progress` or `completed`.
    pub status: types::CheckRunStatus,
    /// Set exactly when `status` is `completed`, for runs created through the API.
    pub conclusion: Option<types::CheckRunConclusion>,
    /// When the run was recorded; the embedded app's timestamps.
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub details_url: Option<String>,
    pub external_id: Option<String>,
    pub output_title: Option<String>,
    pub output_summary: Option<String>,
    pub output_text: Option<String>,
    /// Annotations in the order they were submitted.
    pub annotations: Vec<CheckAnnotationRecord>,
    /// Pull requests a fixture associated with the run.
    pub pull_requests: Vec<types::PullRequestMinimal>,
    /// Values a fixture set in place of the computed ones.
    pub seeded: SeededCheckRun,
}

/// The app that owns a check run.
#[derive(Clone)]
pub enum CheckRunApp {
    /// An app writing through the Checks API, rendered from its id.
    Id(i64),
    /// An app a fixture described field by field.
    Seeded(Box<types::NullableIntegration>),
}

impl CheckRunApp {
    /// The app id.
    pub fn id(&self) -> i64 {
        match self {
            Self::Id(id) => *id,
            Self::Seeded(app) => app.id,
        }
    }
}

/// Fixture values that replace what a check run's REST model would otherwise compute.
#[derive(Clone, Default)]
pub struct SeededCheckRun {
    /// The `head_sha` shown, when it differs from the commit that lists the run.
    pub head_sha: Option<String>,
    pub node_id: Option<String>,
    pub url: Option<String>,
    /// `Some(None)` seeds an explicit `null`.
    pub html_url: Option<Option<String>>,
    pub annotations_url: Option<String>,
    pub annotations_count: Option<i64>,
}

/// One annotation on a check run.
#[derive(Clone)]
pub struct CheckAnnotationRecord {
    pub path: String,
    pub start_line: i64,
    pub end_line: i64,
    pub start_column: Option<i64>,
    pub end_column: Option<i64>,
    /// `notice`, `warning` or `failure`, as the request's enum accepted it.
    pub level: String,
    pub message: String,
    pub title: Option<String>,
    pub raw_details: Option<String>,
}

/// One commit status. A commit's later status for the same context supersedes earlier ones.
#[derive(Clone)]
pub struct CommitStatusRecord {
    /// Status id.
    pub id: i64,
    pub state: StatusState,
    pub context: String,
    pub description: Option<String>,
    pub target_url: Option<String>,
    /// The user who set the status, as a fixture described them.
    pub creator: Option<types::SimpleUser>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// The state of a commit status.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusState {
    Error,
    Failure,
    Pending,
    Success,
}

impl StatusState {
    const ALL: [Self; 4] = [Self::Error, Self::Failure, Self::Pending, Self::Success];

    /// The REST `state` value.
    pub fn name(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Failure => "failure",
            Self::Pending => "pending",
            Self::Success => "success",
        }
    }

    /// The state whose REST value is `name`.
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|state| state.name() == name)
    }
}

/// The combined state of a commit's latest status per context.
///
/// Source: <https://docs.github.com/en/rest/commits/statuses#get-the-combined-status-for-a-specific-reference>:
/// "failure if any of the contexts report as error or failure; pending if there are no statuses
/// or a context is pending; success if the latest status for all contexts is success".
pub fn combined_state(latest: &[&CommitStatusRecord]) -> StatusState {
    if latest
        .iter()
        .any(|status| matches!(status.state, StatusState::Error | StatusState::Failure))
    {
        StatusState::Failure
    } else if latest.is_empty()
        || latest
            .iter()
            .any(|status| status.state != StatusState::Success)
    {
        StatusState::Pending
    } else {
        StatusState::Success
    }
}

/// The latest status of each context in `statuses`, ordered by context.
pub fn latest_per_context(statuses: &[CommitStatusRecord]) -> Vec<&CommitStatusRecord> {
    let mut latest = std::collections::BTreeMap::new();
    for status in statuses {
        latest.insert(status.context.as_str(), status);
    }
    latest.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(context: &str, state: StatusState) -> CommitStatusRecord {
        CommitStatusRecord {
            id: 1,
            state,
            context: context.to_owned(),
            description: None,
            target_url: None,
            creator: None,
            created_at: DateTime::UNIX_EPOCH,
            updated_at: DateTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn combined_state_follows_the_documented_rules() {
        assert_eq!(combined_state(&[]), StatusState::Pending);
        let success = status("a", StatusState::Success);
        let pending = status("b", StatusState::Pending);
        let error = status("c", StatusState::Error);
        assert_eq!(combined_state(&[&success]), StatusState::Success);
        assert_eq!(combined_state(&[&success, &pending]), StatusState::Pending);
        assert_eq!(
            combined_state(&[&success, &pending, &error]),
            StatusState::Failure
        );
    }

    #[test]
    fn a_later_status_supersedes_its_context() {
        let statuses = [
            status("b", StatusState::Failure),
            status("a", StatusState::Pending),
            status("b", StatusState::Success),
        ];
        let latest = latest_per_context(&statuses);
        let states: Vec<_> = latest
            .iter()
            .map(|s| (s.context.as_str(), s.state))
            .collect();
        assert_eq!(
            states,
            [("a", StatusState::Pending), ("b", StatusState::Success)]
        );
    }
}
