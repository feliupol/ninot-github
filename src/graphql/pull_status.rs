//! Status and check-run rollups on a pull request head.

use std::collections::BTreeMap;

use super::objects::{Commit, StatusCheckRollup};
use super::pull_objects::{CheckSuiteNode, RollupNode};
use super::types::{CheckConclusionState, CheckRunState, CheckStatusState, StatusState};
use crate::render::node_id;
use crate::state::{Account, Repo};
use crate::time;

/// Contexts on `sha` and the combined rollup state. Empty when nothing is reported.
pub(super) struct Rollup {
    pub nodes: Vec<RollupNode>,
    pub state: Option<StatusState>,
    pub status_counts: BTreeMap<StatusState, usize>,
    pub check_counts: BTreeMap<CheckRunState, usize>,
}

/// Status and check-run contexts on `sha`.
pub(super) fn rollup_nodes(repo: &Repo, sha: &str) -> Rollup {
    let mut nodes = Vec::new();
    let mut status_counts: BTreeMap<StatusState, usize> = BTreeMap::new();
    for status in super::read::status_contexts(repo, sha) {
        *status_counts.entry(status.state).or_default() += 1;
        nodes.push(RollupNode::StatusContext {
            id: status.id,
            context: status.context,
            state: status.state,
            target_url: status.target_url,
            created_at: status.created_at,
            updated_at: status.updated_at,
            description: status.description,
        });
    }
    let mut check_counts: BTreeMap<CheckRunState, usize> = BTreeMap::new();
    let mut checks: BTreeMap<&str, &crate::state::CheckRunRecord> = BTreeMap::new();
    for run in repo.check_runs.iter().filter(|run| run.head_sha == sha) {
        checks.insert(run.name.as_str(), run);
    }
    for run in checks.into_values() {
        let status = check_status(run.status);
        let conclusion = run.conclusion.map(check_conclusion);
        let bucket = conclusion
            .map(conclusion_state)
            .unwrap_or_else(|| status_state(status));
        *check_counts.entry(bucket).or_default() += 1;
        nodes.push(RollupNode::CheckRun {
            id: node_id("CheckRun", u64::try_from(run.id).unwrap_or(0)),
            name: run.name.clone(),
            status,
            conclusion,
            started_at: run.started_at.as_ref().map(time::format),
            completed_at: run.completed_at.as_ref().map(time::format),
            details_url: run.details_url.clone(),
            check_suite: CheckSuiteNode {
                typename: "CheckSuite",
                workflow_run: None,
            },
        });
    }
    let state = combined(&status_counts, &check_counts, nodes.is_empty());
    Rollup {
        nodes,
        state,
        status_counts,
        check_counts,
    }
}

/// Combined status rollup of `sha` when a status or check run is reported.
pub(super) fn status_rollup(
    account: &Account,
    repo: &Repo,
    sha: &str,
) -> Option<StatusCheckRollup> {
    let state = rollup_nodes(repo, sha).state?;
    Some(StatusCheckRollup {
        typename: "StatusCheckRollup",
        id: node_id("StatusCheckRollup", sha_id(sha)),
        state,
        commit: Commit {
            typename: "Commit",
            oid: sha.to_owned(),
            message_headline: None,
            message_body: None,
        },
        owner: account.login.clone(),
        name: repo.name.clone(),
    })
}

fn combined(
    status_counts: &BTreeMap<StatusState, usize>,
    check_counts: &BTreeMap<CheckRunState, usize>,
    empty: bool,
) -> Option<StatusState> {
    if empty {
        return None;
    }
    let failed = status_counts
        .keys()
        .any(|state| matches!(state, StatusState::Failure | StatusState::Error))
        || check_counts.keys().any(|state| {
            matches!(
                state,
                CheckRunState::Failure
                    | CheckRunState::Cancelled
                    | CheckRunState::TimedOut
                    | CheckRunState::ActionRequired
                    | CheckRunState::StartupFailure
            )
        });
    if failed {
        return Some(StatusState::Failure);
    }
    let pending = status_counts
        .keys()
        .any(|state| *state == StatusState::Pending)
        || check_counts.keys().any(|state| {
            !matches!(
                state,
                CheckRunState::Success | CheckRunState::Neutral | CheckRunState::Skipped
            )
        });
    Some(if pending {
        StatusState::Pending
    } else {
        StatusState::Success
    })
}

fn sha_id(sha: &str) -> u64 {
    sha.bytes().fold(0u64, |acc, byte| {
        acc.wrapping_mul(33).wrapping_add(u64::from(byte))
    })
}

fn check_status(status: crate::types::CheckRunStatus) -> CheckStatusState {
    match status {
        crate::types::CheckRunStatus::Queued => CheckStatusState::Queued,
        crate::types::CheckRunStatus::InProgress => CheckStatusState::InProgress,
        crate::types::CheckRunStatus::Completed => CheckStatusState::Completed,
        crate::types::CheckRunStatus::Waiting => CheckStatusState::Waiting,
        crate::types::CheckRunStatus::Requested => CheckStatusState::Requested,
        crate::types::CheckRunStatus::Pending => CheckStatusState::Pending,
    }
}

fn check_conclusion(conclusion: crate::types::CheckRunConclusion) -> CheckConclusionState {
    match conclusion {
        crate::types::CheckRunConclusion::ActionRequired => CheckConclusionState::ActionRequired,
        crate::types::CheckRunConclusion::Cancelled => CheckConclusionState::Cancelled,
        crate::types::CheckRunConclusion::Failure => CheckConclusionState::Failure,
        crate::types::CheckRunConclusion::Neutral => CheckConclusionState::Neutral,
        crate::types::CheckRunConclusion::Skipped => CheckConclusionState::Skipped,
        crate::types::CheckRunConclusion::Success => CheckConclusionState::Success,
        crate::types::CheckRunConclusion::TimedOut => CheckConclusionState::TimedOut,
    }
}

fn conclusion_state(conclusion: CheckConclusionState) -> CheckRunState {
    match conclusion {
        CheckConclusionState::ActionRequired => CheckRunState::ActionRequired,
        CheckConclusionState::Cancelled => CheckRunState::Cancelled,
        CheckConclusionState::Failure => CheckRunState::Failure,
        CheckConclusionState::Neutral => CheckRunState::Neutral,
        CheckConclusionState::Skipped => CheckRunState::Skipped,
        CheckConclusionState::Stale => CheckRunState::Stale,
        CheckConclusionState::StartupFailure => CheckRunState::StartupFailure,
        CheckConclusionState::Success => CheckRunState::Success,
        CheckConclusionState::TimedOut => CheckRunState::TimedOut,
    }
}

fn status_state(status: CheckStatusState) -> CheckRunState {
    match status {
        CheckStatusState::Completed => CheckRunState::Completed,
        CheckStatusState::InProgress => CheckRunState::InProgress,
        CheckStatusState::Pending => CheckRunState::Pending,
        CheckStatusState::Queued | CheckStatusState::Requested => CheckRunState::Queued,
        CheckStatusState::Waiting => CheckRunState::Waiting,
    }
}
