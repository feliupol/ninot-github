//! The `check_run` event.

use chrono::{DateTime, Utc};

use super::{Ctx, PayloadError, model};
use crate::types;
use crate::urls::RepoUrls;
use crate::webhook_types as wt;

/// Queues `check_run` `created`.
pub(crate) fn created(ctx: &mut Ctx, check: &types::CheckRun) -> Result<(), PayloadError> {
    deliver(ctx, check, |check, urls, sender, now| {
        Ok(wt::CheckRunCreated {
            action: wt::CheckRunCreatedAction::Created,
            check_run: check_run!(CheckRunCreated, check, urls, sender, now, {
                completed_at: completed_at(check, now),
                status: model::parse("check_run.status", &run_status(&check.status))?,
            }),
            installation: None,
            organization: None,
            repository: urls.1,
            requested_action: None,
            sender: sender.clone(),
        })
    })
}

/// Queues `check_run` `completed`.
pub(crate) fn completed(ctx: &mut Ctx, check: &types::CheckRun) -> Result<(), PayloadError> {
    deliver(ctx, check, |check, urls, sender, now| {
        Ok(wt::CheckRunCompleted {
            action: wt::CheckRunCompletedAction::Completed,
            check_run: check_run!(CheckRunCompleted, check, urls, sender, now, {
                completed_at: check.completed_at.unwrap_or(now),
                status: wt::CheckRunCompletedCheckRunStatus::Completed,
            }),
            installation: None,
            organization: None,
            repository: urls.1,
            requested_action: None,
            sender: sender.clone(),
        })
    })
}

/// Queues `check_run` `rerequested`.
pub(crate) fn rerequested(ctx: &mut Ctx, check: &types::CheckRun) -> Result<(), PayloadError> {
    deliver(ctx, check, |check, urls, sender, now| {
        Ok(wt::CheckRunRerequested {
            action: wt::CheckRunRerequestedAction::Rerequested,
            check_run: check_run!(CheckRunRerequested, check, urls, sender, now, {
                completed_at: check.completed_at.unwrap_or(now),
                status: wt::CheckRunRerequestedCheckRunStatus::Completed,
            }),
            installation: None,
            organization: None,
            repository: urls.1,
            requested_action: None,
            sender: sender.clone(),
        })
    })
}

/// Builds the `check_run` object of the payload type `$payload`, whose per-action fields
/// (`completed_at` and `status`) are in `$extra`.
macro_rules! check_run {
    ($payload:ident, $check:expr, $urls:expr, $sender:expr, $now:expr, { $($extra:tt)* }) => {{
        let check: &types::CheckRun = $check;
        let (urls, _): &(RepoUrls, _) = &$urls;
        let (sender, now): (&wt::User, DateTime<Utc>) = ($sender, $now);
        let suite = suite_from_run(check, urls);
        paste_check_run!($payload, {
            app: model::app(check.app.as_ref(), sender, now)?,
            check_suite: suite!($payload, suite, model::app(check.app.as_ref(), sender, now)?, now),
            conclusion: check
                .conclusion
                .as_ref()
                .and_then(|conclusion| conclusion.to_string().parse().ok()),
            deployment: None,
            details_url: check.details_url.clone(),
            external_id: check.external_id.clone().unwrap_or_default(),
            head_sha: check.head_sha.clone(),
            html_url: check.html_url.clone().unwrap_or_else(|| check.url.clone()),
            id: check.id,
            name: check.name.clone(),
            node_id: Some(check.node_id.clone()),
            output: paste_output!($payload, check),
            pull_requests: check.pull_requests.iter().map(model::minimal_pull).collect(),
            started_at: check.started_at.unwrap_or(now),
            url: check.url.clone(),
            $($extra)*
        })
    }};
}
use check_run;

/// The check suite fields a check run carries: its id, status and conclusion.
pub(crate) struct RunSuite {
    pub(crate) id: i64,
    pub(crate) status: &'static str,
    pub(crate) conclusion: Option<String>,
    pub(crate) head_sha: String,
    pub(crate) url: String,
    pub(crate) pull_requests: Vec<wt::CheckRunPullRequest>,
}

/// The suite of `check`: a queued, in-progress or completed suite with the run's conclusion
/// when the suite schema knows it.
pub(crate) fn suite_from_run(check: &types::CheckRun, urls: &RepoUrls) -> RunSuite {
    let id = check
        .check_suite
        .as_ref()
        .map_or(check.id, |suite| suite.id);
    RunSuite {
        id,
        status: match check.status {
            types::CheckRunStatus::InProgress => "in_progress",
            types::CheckRunStatus::Completed => "completed",
            _ => "queued",
        },
        conclusion: check.conclusion.as_ref().map(ToString::to_string),
        head_sha: check.head_sha.clone(),
        url: urls.check_suite(id).api,
        pull_requests: check
            .pull_requests
            .iter()
            .map(model::minimal_pull)
            .collect(),
    }
}

/// The check suite object nested in a check run of the payload type `$payload`.
macro_rules! suite {
    ($payload:ident, $suite:expr, $app:expr, $now:expr) => {{
        let suite: RunSuite = $suite;
        pastey_suite!($payload, {
            after: None,
            app: $app,
            before: None,
            conclusion: suite_conclusion!($payload, suite.conclusion),
            created_at: $now,
            deployment: None,
            head_branch: None,
            head_sha: suite.head_sha,
            id: suite.id,
            node_id: None,
            pull_requests: suite.pull_requests,
            status: model::parse("check_suite.status", suite.status)?,
            updated_at: $now,
            url: suite.url,
        })
    }};
}
use suite;

/// The suite conclusion of a check run of `$payload`: the run's, when the suite schema knows
/// it. A rerequested run has finished, so its suite has a conclusion, `neutral` by default.
macro_rules! suite_conclusion {
    (CheckRunRerequested, $conclusion:expr) => {
        $conclusion
            .as_deref()
            .and_then(|text| text.parse().ok())
            .unwrap_or(wt::CheckRunRerequestedCheckRunCheckSuiteConclusion::Neutral)
    };
    ($payload:ident, $conclusion:expr) => {
        $conclusion.as_deref().and_then(|text| text.parse().ok())
    };
}
use suite_conclusion;

/// `$payload`'s check run struct with `$fields`.
macro_rules! paste_check_run {
    (CheckRunCreated, { $($fields:tt)* }) => { wt::CheckRunCreatedCheckRun { $($fields)* } };
    (CheckRunCompleted, { $($fields:tt)* }) => { wt::CheckRunCompletedCheckRun { $($fields)* } };
    (CheckRunRerequested, { $($fields:tt)* }) => { wt::CheckRunRerequestedCheckRun { $($fields)* } };
}
use paste_check_run;

/// `$payload`'s check suite struct with `$fields`.
macro_rules! pastey_suite {
    (CheckRunCreated, { $($fields:tt)* }) => { wt::CheckRunCreatedCheckRunCheckSuite { $($fields)* } };
    (CheckRunCompleted, { $($fields:tt)* }) => { wt::CheckRunCompletedCheckRunCheckSuite { $($fields)* } };
    (CheckRunRerequested, { $($fields:tt)* }) => {
        wt::CheckRunRerequestedCheckRunCheckSuite { $($fields)* }
    };
}
use pastey_suite;

/// `$payload`'s check run output for `$check`.
macro_rules! paste_output {
    ($payload:ident, $check:expr) => {{
        let output = &$check.output;
        paste_output_struct!($payload, {
            annotations_count: output.annotations_count,
            annotations_url: output.annotations_url.clone(),
            summary: output.summary.clone(),
            text: output.text.clone(),
            title: output.title.clone(),
        })
    }};
}
use paste_output;

/// `$payload`'s check run output struct with `$fields`.
macro_rules! paste_output_struct {
    (CheckRunCreated, { $($fields:tt)* }) => { wt::CheckRunCreatedCheckRunOutput { $($fields)* } };
    (CheckRunCompleted, { $($fields:tt)* }) => {
        wt::CheckRunCompletedCheckRunOutput { $($fields)* }
    };
    (CheckRunRerequested, { $($fields:tt)* }) => {
        wt::CheckRunRerequestedCheckRunOutput { $($fields)* }
    };
}
use paste_output_struct;

/// When `check` completed: its time, or `now` for a completed run without one.
fn completed_at(check: &types::CheckRun, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    match check.status {
        types::CheckRunStatus::Completed => Some(check.completed_at.unwrap_or(now)),
        _ => check.completed_at,
    }
}

/// The webhook status of a run: the REST `requested` and `pending` are `queued`.
fn run_status(status: &types::CheckRunStatus) -> String {
    match status {
        types::CheckRunStatus::Requested | types::CheckRunStatus::Pending => "queued".to_owned(),
        other => other.to_string(),
    }
}

/// Builds the payload from `check`, the repository URLs and webhook repository, the sender and
/// the current time, and queues it.
fn deliver<P: serde::Serialize>(
    ctx: &mut Ctx,
    check: &types::CheckRun,
    payload: impl FnOnce(
        &types::CheckRun,
        (RepoUrls, wt::Repository),
        &wt::User,
        DateTime<Utc>,
    ) -> Result<P, PayloadError>,
) -> Result<(), PayloadError> {
    if !ctx.wants("check_run") {
        return Ok(());
    }
    let Some((repository, sender)) = ctx.parties("check_run", None)? else {
        return Ok(());
    };
    let payload = payload(check, (ctx.urls(), repository), &sender, model::now());
    ctx.enqueue_built("check_run", payload)
}
