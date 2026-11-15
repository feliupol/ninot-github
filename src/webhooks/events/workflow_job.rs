//! The `workflow_job` event.

use chrono::{DateTime, Utc};

use super::{Ctx, model};
use crate::types;
use crate::webhook_types as wt;

/// Queues `workflow_job` for `job`.
///
/// The action is the job status: `completed`, `in_progress`, `waiting`, or `queued`.
pub(crate) fn deliver(ctx: &mut Ctx, run: &types::WorkflowRun, job: &types::Job) {
    if !ctx.wants("workflow_job") {
        return;
    }
    let (Some(repository), Some(sender)) = (ctx.repository(), ctx.sender(None)) else {
        return;
    };
    let workflow_job = workflow_job(run, job);
    macro_rules! payload {
        ($type:ident, $action:ident::$variant:ident) => {
            ctx.enqueue(
                "workflow_job",
                &wt::$type {
                    action: wt::$action::$variant,
                    deployment: None,
                    installation: None,
                    organization: None,
                    repository,
                    sender,
                    workflow_job,
                },
            )
        };
    }
    match workflow_job.status {
        wt::WorkflowJobStatus::Completed => {
            payload!(WorkflowJobCompleted, WorkflowJobCompletedAction::Completed)
        }
        wt::WorkflowJobStatus::InProgress => {
            payload!(
                WorkflowJobInProgress,
                WorkflowJobInProgressAction::InProgress
            )
        }
        wt::WorkflowJobStatus::Waiting => {
            payload!(WorkflowJobWaiting, WorkflowJobWaitingAction::Waiting)
        }
        wt::WorkflowJobStatus::Queued => {
            payload!(WorkflowJobQueued, WorkflowJobQueuedAction::Queued)
        }
    }
}

/// The webhook job for `job` of `run`. A job the schema has no status for is queued, and a
/// completed job without a conclusion it knows succeeded.
fn workflow_job(run: &types::WorkflowRun, job: &types::Job) -> wt::WorkflowJob {
    let now = model::now();
    let status = match job.status {
        types::JobStatus::Completed => wt::WorkflowJobStatus::Completed,
        types::JobStatus::InProgress => wt::WorkflowJobStatus::InProgress,
        types::JobStatus::Waiting => wt::WorkflowJobStatus::Waiting,
        _ => wt::WorkflowJobStatus::Queued,
    };
    let conclusion = job
        .conclusion
        .as_ref()
        .and_then(|conclusion| conclusion.to_string().parse().ok())
        .or((status == wt::WorkflowJobStatus::Completed)
            .then_some(wt::WorkflowJobConclusion::Success));
    wt::WorkflowJob {
        check_run_url: job.check_run_url.clone(),
        completed_at: job.completed_at,
        conclusion,
        created_at: job.created_at,
        head_branch: Some(job.head_branch.clone().unwrap_or_else(|| "main".to_owned())),
        head_sha: job.head_sha.clone(),
        html_url: job.html_url.clone().unwrap_or_else(|| job.url.clone()),
        id: job.id,
        labels: job.labels.clone(),
        name: job.name.clone(),
        node_id: job.node_id.clone(),
        run_attempt: job.run_attempt.unwrap_or(1),
        run_id: job.run_id,
        run_url: job.run_url.clone(),
        runner_group_id: job.runner_group_id,
        runner_group_name: job.runner_group_name.clone(),
        runner_id: job.runner_id,
        runner_name: job.runner_name.clone(),
        started_at: job.started_at,
        status,
        steps: job
            .steps
            .iter()
            .map(|step| workflow_step(step, now))
            .collect(),
        url: job.url.clone(),
        workflow_name: Some(
            job.workflow_name
                .clone()
                .or_else(|| run.name.clone())
                .unwrap_or_else(|| "CI".to_owned()),
        ),
    }
}

/// The webhook step for `step`. A completed step without a conclusion the schema knows
/// succeeded, and a started step without times started `now`.
fn workflow_step(step: &types::JobStepsItem, now: DateTime<Utc>) -> wt::WorkflowStep {
    match step.status {
        types::JobStepsItemStatus::Completed => {
            wt::WorkflowStep::Completed(wt::WorkflowStepCompleted {
                completed_at: step.completed_at.unwrap_or(now),
                conclusion: step
                    .conclusion
                    .as_deref()
                    .and_then(|conclusion| conclusion.to_ascii_lowercase().parse().ok())
                    .unwrap_or(wt::WorkflowStepCompletedConclusion::Success),
                name: step.name.clone(),
                number: step.number,
                started_at: step.started_at.unwrap_or(now),
                status: wt::WorkflowStepCompletedStatus::Completed,
            })
        }
        types::JobStepsItemStatus::InProgress => {
            wt::WorkflowStep::InProgress(wt::WorkflowStepInProgress {
                completed_at: (),
                conclusion: (),
                name: step.name.clone(),
                number: step.number,
                started_at: step.started_at.unwrap_or(now),
                status: wt::WorkflowStepInProgressStatus::InProgress,
            })
        }
        types::JobStepsItemStatus::Queued => wt::WorkflowStep::Queued(wt::WorkflowStepQueued {
            completed_at: (),
            conclusion: (),
            name: step.name.clone(),
            number: step.number,
            started_at: (),
            status: wt::WorkflowStepQueuedStatus::Queued,
        }),
    }
}
