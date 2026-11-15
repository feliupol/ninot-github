//! The `workflow_run` event.

use super::{Ctx, model};
use crate::types;
use crate::urls::RepoUrls;
use crate::webhook_types as wt;

/// Queues `workflow_run` for `run`.
///
/// The action follows the run's status: `completed` and `in_progress` keep that name, and every
/// other status, including `queued`, is delivered as `requested`.
pub(crate) fn deliver(ctx: &mut Ctx, run: &types::WorkflowRun) {
    if !ctx.wants("workflow_run") {
        return;
    }
    let (Some(repository), Some(sender)) = (ctx.repository(), ctx.sender(None)) else {
        return;
    };
    let workflow_run = workflow_run(ctx, run, &repository, &sender);
    let workflow = workflow(run, &ctx.urls());
    macro_rules! payload {
        ($type:ident, $action:ident::$variant:ident) => {
            ctx.enqueue(
                "workflow_run",
                &wt::$type {
                    action: wt::$action::$variant,
                    installation: None,
                    organization: None,
                    repository,
                    sender,
                    workflow,
                    workflow_run,
                },
            )
        };
    }
    match workflow_run.status {
        wt::WorkflowRunStatus::Completed => {
            payload!(WorkflowRunCompleted, WorkflowRunCompletedAction::Completed)
        }
        wt::WorkflowRunStatus::InProgress => {
            payload!(
                WorkflowRunInProgress,
                WorkflowRunInProgressAction::InProgress
            )
        }
        _ => payload!(WorkflowRunRequested, WorkflowRunRequestedAction::Requested),
    }
}

/// The webhook run for `run`, acted on by `sender` in `repository`. A status the schema does not
/// know is `queued`, and a completed run without a conclusion it knows is `neutral`.
fn workflow_run(
    ctx: &Ctx,
    run: &types::WorkflowRun,
    repository: &wt::Repository,
    sender: &wt::User,
) -> wt::WorkflowRun {
    let status = run
        .status
        .as_deref()
        .and_then(|status| status.to_ascii_lowercase().parse().ok())
        .unwrap_or(wt::WorkflowRunStatus::Queued);
    let conclusion = run
        .conclusion
        .as_deref()
        .and_then(|conclusion| conclusion.to_ascii_lowercase().parse().ok())
        .or((status == wt::WorkflowRunStatus::Completed)
            .then_some(wt::WorkflowRunConclusion::Neutral));
    let lite = model::repository_lite(repository);
    let name = run.name.clone().unwrap_or_else(|| "CI".to_owned());
    wt::WorkflowRun {
        actor: sender.clone(),
        artifacts_url: run.artifacts_url.clone(),
        cancel_url: run.cancel_url.clone(),
        check_suite_id: run.check_suite_id.unwrap_or(run.id),
        check_suite_node_id: run
            .check_suite_node_id
            .clone()
            .unwrap_or_else(|| format!("CS_{}", run.id)),
        check_suite_url: run.check_suite_url.clone(),
        conclusion,
        created_at: run.created_at,
        display_title: run.display_title.clone(),
        event: run.event.clone(),
        head_branch: run.head_branch.clone().unwrap_or_else(|| "main".to_owned()),
        head_commit: model::commit_simple(ctx, &run.head_sha),
        head_repository: lite.clone(),
        head_sha: run.head_sha.clone(),
        html_url: run.html_url.clone(),
        id: run.id,
        jobs_url: run.jobs_url.clone(),
        logs_url: run.logs_url.clone(),
        name,
        node_id: run.node_id.clone(),
        path: run.path.clone(),
        previous_attempt_url: run.previous_attempt_url.clone(),
        pull_requests: run
            .pull_requests
            .iter()
            .flatten()
            .map(|pull| {
                let minimal = model::minimal_pull(pull);
                wt::WorkflowRunPullRequestsItem {
                    base: wt::WorkflowRunPullRequestsItemBase {
                        ref_: minimal.base.ref_,
                        repo: minimal.base.repo,
                        sha: minimal.base.sha,
                    },
                    head: wt::WorkflowRunPullRequestsItemHead {
                        ref_: minimal.head.ref_,
                        repo: minimal.head.repo,
                        sha: minimal.head.sha,
                    },
                    id: minimal.id,
                    number: minimal.number,
                    url: minimal.url,
                }
            })
            .collect(),
        referenced_workflows: run
            .referenced_workflows
            .iter()
            .flatten()
            .map(|workflow| wt::ReferencedWorkflow {
                path: workflow.path.clone(),
                ref_: workflow.ref_.clone(),
                sha: workflow.sha.clone(),
            })
            .collect(),
        repository: lite,
        rerun_url: run.rerun_url.clone(),
        run_attempt: run.run_attempt.unwrap_or(1),
        run_number: run.run_number,
        run_started_at: run.run_started_at.unwrap_or(run.created_at),
        status,
        triggering_actor: sender.clone(),
        updated_at: run.updated_at,
        url: run.url.clone(),
        workflow_id: run.workflow_id,
        workflow_url: run.workflow_url.clone(),
    }
}

/// The workflow `run` belongs to.
fn workflow(run: &types::WorkflowRun, repo: &RepoUrls) -> wt::Workflow {
    let id = run.workflow_id;
    wt::Workflow {
        badge_url: repo.workflow_badge(id),
        created_at: run.created_at,
        html_url: repo.blob_html("HEAD", &run.path),
        id,
        name: run.name.clone().unwrap_or_else(|| "CI".to_owned()),
        node_id: format!("W_{id}"),
        path: run.path.clone(),
        state: "active".to_owned(),
        updated_at: run.updated_at,
        url: repo.workflow(id),
    }
}
