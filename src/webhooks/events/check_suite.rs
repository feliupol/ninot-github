//! The `check_suite` event.

use super::check_run::{RunSuite, suite_from_run};
use super::{Ctx, PayloadError, model};
use crate::types;
use crate::webhook_types as wt;

/// Queues `check_suite` `requested` for the first check run on a commit.
pub(crate) fn requested(ctx: &mut Ctx, check: &types::CheckRun) -> Result<(), PayloadError> {
    if !ctx.wants("check_suite") {
        return Ok(());
    }
    let Some((repository, sender)) = ctx.parties("check_suite", None)? else {
        return Ok(());
    };
    let now = model::now();
    let RunSuite {
        id,
        status,
        conclusion,
        head_sha,
        url,
        pull_requests,
    } = suite_from_run(check, &ctx.urls());
    let status = match model::parse("check_suite.status", status) {
        Ok(status) => status,
        Err(error) => return ctx.enqueue_built("check_suite", Err::<(), _>(error)),
    };
    let app = match model::app(check.app.as_ref(), &sender, now) {
        Ok(app) => app,
        Err(error) => return ctx.enqueue_built("check_suite", Err::<(), _>(error)),
    };
    let check_suite = wt::CheckSuiteRequestedCheckSuite {
        after: None,
        app,
        before: None,
        check_runs_url: ctx.urls().check_suite(id).check_runs(),
        conclusion: conclusion.as_deref().and_then(|text| text.parse().ok()),
        created_at: now,
        head_branch: None,
        head_commit: model::commit_simple(ctx, &head_sha),
        head_sha,
        id,
        latest_check_runs_count: 1,
        node_id: format!("CS_{id}"),
        pull_requests,
        rerequestable: None,
        runs_rerequestable: None,
        status: Some(status),
        updated_at: now,
        url,
    };
    let payload = wt::CheckSuiteRequested {
        action: wt::CheckSuiteRequestedAction::Requested,
        check_suite,
        installation: None,
        organization: None,
        repository,
        sender,
    };
    ctx.enqueue_built("check_suite", Ok(payload))
}

/// Queues `check_suite` `requested` for a suite created through the Checks API.
pub(crate) fn requested_suite(
    ctx: &mut Ctx,
    suite: &types::CheckSuite,
) -> Result<(), PayloadError> {
    deliver(ctx, suite, |check_suite, repository, sender| {
        let rerequestable = check_suite.0.rerequestable;
        let runs_rerequestable = check_suite.0.runs_rerequestable;
        wt::CheckSuiteRequested {
            action: wt::CheckSuiteRequestedAction::Requested,
            check_suite: suite_object!(CheckSuiteRequestedCheckSuite, check_suite, {
                rerequestable,
                runs_rerequestable,
            }),
            installation: None,
            organization: None,
            repository,
            sender,
        }
    })
}

/// Queues `check_suite` `rerequested`.
pub(crate) fn rerequested(ctx: &mut Ctx, suite: &types::CheckSuite) -> Result<(), PayloadError> {
    deliver(ctx, suite, |check_suite, repository, sender| {
        wt::CheckSuiteRerequested {
            action: wt::CheckSuiteRerequestedAction::Rerequested,
            check_suite: suite_object!(CheckSuiteRerequestedCheckSuite, check_suite, {}),
            installation: None,
            organization: None,
            repository,
            sender,
        }
    })
}

/// The REST suite with the parts of it the webhook builds from state.
pub(crate) struct SuiteParts<'a>(&'a types::CheckSuite, wt::App, wt::CommitSimple, String);

/// Builds the check suite struct `$type` from `$parts` and the per-action `$extra` fields.
macro_rules! suite_object {
    ($type:ident, $parts:expr, { $($extra:tt)* }) => {{
        let SuiteParts(suite, app, head_commit, url) = $parts;
        let now = model::now();
        wt::$type {
            after: suite.after.clone(),
            app,
            before: suite.before.clone(),
            check_runs_url: suite.check_runs_url.clone(),
            conclusion: suite
                .conclusion
                .as_ref()
                .and_then(|conclusion| conclusion.to_string().parse().ok()),
            created_at: suite.created_at.unwrap_or(now),
            head_branch: suite.head_branch.clone(),
            head_commit,
            head_sha: suite.head_sha.clone(),
            id: suite.id,
            latest_check_runs_count: suite.latest_check_runs_count,
            node_id: suite.node_id.clone(),
            pull_requests: suite
                .pull_requests
                .iter()
                .flatten()
                .map(model::minimal_pull)
                .collect(),
            status: suite
                .status
                .as_ref()
                .and_then(|status| status.to_string().parse().ok()),
            updated_at: suite.updated_at.unwrap_or(now),
            url,
            $($extra)*
        }
    }};
}
use suite_object;

/// Builds the payload from the suite parts, repository and sender, and queues it.
fn deliver<P: serde::Serialize>(
    ctx: &mut Ctx,
    suite: &types::CheckSuite,
    payload: impl FnOnce(SuiteParts, wt::Repository, wt::User) -> P,
) -> Result<(), PayloadError> {
    if !ctx.wants("check_suite") {
        return Ok(());
    }
    let Some((repository, sender)) = ctx.parties("check_suite", None)? else {
        return Ok(());
    };
    let app = match model::app(suite.app.as_ref(), &sender, model::now()) {
        Ok(app) => app,
        Err(error) => return ctx.enqueue_built("check_suite", Err::<(), _>(error)),
    };
    let parts = SuiteParts(
        suite,
        app,
        model::commit_simple(ctx, &suite.head_sha),
        suite
            .url
            .clone()
            .unwrap_or_else(|| ctx.urls().check_suite(suite.id).api),
    );
    ctx.enqueue_built("check_suite", Ok(payload(parts, repository, sender)))
}
