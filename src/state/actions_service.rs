//! The Actions service the official runner talks to once it is registered: the access tokens it
//! exchanges, its broker sessions, and the queue of jobs it is offered.

use std::collections::HashMap;

use super::State;
use crate::fixtures::current_timestamp;
use crate::types;

/// Where a queued job is in its life.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum JobPhase {
    /// Waiting for a runner to take the offered message.
    Queued,
    /// A runner acquired it and is running it.
    Acquired,
    Completed,
}

/// A workflow job whose steps are shell scripts, waiting for or running on a self-hosted runner.
#[derive(Clone, Debug)]
pub(crate) struct ServiceJob {
    pub message_id: u64,
    pub owner: String,
    pub repo: String,
    pub run_id: u64,
    pub job_id: u64,
    pub labels: Vec<String>,
    pub scripts: Vec<String>,
    pub phase: JobPhase,
    pub runner_id: Option<u64>,
}

/// One step the runner reports when it completes a job.
#[derive(Clone, Debug)]
pub(crate) struct StepOutcome {
    pub name: String,
    pub number: i64,
    pub conclusion: String,
}

#[derive(Default)]
pub(crate) struct ActionsService {
    /// Runner id by access token.
    access_tokens: HashMap<String, u64>,
    /// Runner id by broker session id.
    sessions: HashMap<String, u64>,
    jobs: Vec<ServiceJob>,
    /// Message ids already handed to a runner.
    offered: Vec<u64>,
}

/// A GUID derived from a counter, so the same scenario yields the same identifiers.
pub(crate) fn guid(kind: u32, n: u64) -> String {
    format!("{kind:08x}-0000-4000-8000-{n:012x}")
}

impl State {
    /// Queues a job of a new workflow run in `owner/repo` for a runner that carries `labels`.
    ///
    /// Returns the workflow run id and the job id.
    pub(crate) fn queue_script_job(
        &mut self,
        request: &crate::api::ApiRequest,
        owner: &str,
        repo: &str,
        labels: &[String],
        scripts: &[String],
    ) -> std::io::Result<(u64, u64)> {
        let run_id = self.next_id();
        let job_id = self.next_id();
        let message_id = self.next_id();
        let account = self
            .account(owner)
            .ok_or_else(|| std::io::Error::other("repository owner does not exist"))?;
        let repository = account
            .repo(repo)
            .ok_or_else(|| std::io::Error::other("repository does not exist"))?;
        let head_sha = "0".repeat(40);
        let mut run = crate::fixtures::workflow_run(
            request,
            account,
            repository,
            i64::try_from(run_id).unwrap(),
            head_sha,
        );
        run.status = Some("queued".to_owned());
        let mut job = crate::fixtures::job(request, &run, i64::try_from(job_id).unwrap());
        job.status = types::JobStatus::Queued;
        job.completed_at = None;
        job.name = "job".to_owned();
        job.labels = labels.to_vec();
        self.add_workflow_run(owner, repo, run, vec![job]);
        self.actions_service.jobs.push(ServiceJob {
            message_id,
            owner: owner.to_owned(),
            repo: repo.to_owned(),
            run_id,
            job_id,
            labels: labels.to_vec(),
            scripts: scripts.to_vec(),
            phase: JobPhase::Queued,
            runner_id: None,
        });
        Ok((run_id, job_id))
    }

    /// Accepts `token` as the access token of the runner `runner_id` on the Actions service.
    pub(crate) fn register_access_token(&mut self, runner_id: u64, token: String) {
        self.actions_service.access_tokens.insert(token, runner_id);
    }

    /// A number no earlier call returned, for naming what a token or session identifies.
    pub(crate) fn next_service_id(&mut self) -> u64 {
        self.next_id()
    }

    pub(crate) fn access_token_runner(&self, token: &str) -> Option<u64> {
        self.actions_service.access_tokens.get(token).copied()
    }

    /// The organization and name of the runner with `runner_id`.
    pub(crate) fn runner_by_id(&self, runner_id: u64) -> Option<(String, super::Runner)> {
        self.accounts.iter().find_map(|(login, account)| {
            account
                .runners
                .get(&runner_id)
                .map(|runner| (login.clone(), runner.clone()))
        })
    }

    fn set_runner_state(&mut self, runner_id: u64, status: &str, busy: bool) {
        for account in self.accounts.values_mut() {
            if let Some(runner) = account.runners.get_mut(&runner_id) {
                status.clone_into(&mut runner.status);
                runner.busy = busy;
            }
        }
    }

    /// Opens a broker session: the runner is online until the session is deleted.
    pub(crate) fn open_session(&mut self, runner_id: u64) -> String {
        let id = guid(1, self.next_id());
        self.actions_service.sessions.insert(id.clone(), runner_id);
        self.set_runner_state(runner_id, "online", false);
        id
    }

    pub(crate) fn session_runner(&self, session_id: &str) -> Option<u64> {
        self.actions_service.sessions.get(session_id).copied()
    }

    pub(crate) fn close_session(&mut self, session_id: &str) {
        if let Some(runner_id) = self.actions_service.sessions.remove(session_id) {
            self.set_runner_state(runner_id, "offline", false);
        }
    }

    /// The next queued job `runner_id` can take and has not been offered yet.
    pub(crate) fn offer_job(&mut self, runner_id: u64) -> Option<ServiceJob> {
        let (org, runner) = self.runner_by_id(runner_id)?;
        let job = self
            .actions_service
            .jobs
            .iter()
            .find(|job| {
                job.phase == JobPhase::Queued
                    && !self.actions_service.offered.contains(&job.message_id)
                    && job.owner.eq_ignore_ascii_case(&org)
                    && job.labels.iter().all(|wanted| {
                        runner
                            .labels
                            .iter()
                            .any(|label| label.name.eq_ignore_ascii_case(wanted))
                    })
            })?
            .clone();
        self.actions_service.offered.push(job.message_id);
        Some(job)
    }

    /// Marks the offered job `message_id` as running on `runner_id`.
    pub(crate) fn acquire_job(
        &mut self,
        message_id: u64,
        runner_id: u64,
    ) -> Result<ServiceJob, AcquireError> {
        let (_, runner) = self.runner_by_id(runner_id).ok_or(AcquireError::NotFound)?;
        let found = self
            .actions_service
            .jobs
            .iter_mut()
            .find(|job| job.message_id == message_id)
            .ok_or(AcquireError::NotFound)?;
        if found.phase != JobPhase::Queued {
            return Err(AcquireError::AlreadyAcquired);
        }
        found.phase = JobPhase::Acquired;
        found.runner_id = Some(runner_id);
        let job = found.clone();
        self.set_runner_state(runner_id, "online", true);
        self.update_job(&job, |run, stored| {
            run.status = Some("in_progress".to_owned());
            stored.status = types::JobStatus::InProgress;
            stored.started_at = current_timestamp();
            stored.runner_id = Some(i64::try_from(runner.id).unwrap());
            stored.runner_name = Some(runner.name.clone());
            stored.runner_group_id = Some(i64::try_from(runner.group_id).unwrap());
        });
        Ok(job)
    }

    /// The acquired job with `job_id`, which the runner renews and completes.
    pub(crate) fn acquired_job(&self, job_id: u64) -> Option<ServiceJob> {
        self.actions_service
            .jobs
            .iter()
            .find(|job| job.job_id == job_id && job.phase == JobPhase::Acquired)
            .cloned()
    }

    /// Records the runner's verdict on a job and its steps.
    pub(crate) fn complete_job(
        &mut self,
        job_id: u64,
        conclusion: types::JobConclusion,
        steps: &[StepOutcome],
    ) -> Option<ServiceJob> {
        let found = self
            .actions_service
            .jobs
            .iter_mut()
            .find(|job| job.job_id == job_id && job.phase == JobPhase::Acquired)?;
        found.phase = JobPhase::Completed;
        let job = found.clone();
        if let Some(runner_id) = job.runner_id {
            self.set_runner_state(runner_id, "online", false);
            // GitHub removes an ephemeral runner once its one job is done.
            for account in self.accounts.values_mut() {
                if account
                    .runners
                    .get(&runner_id)
                    .is_some_and(|runner| runner.ephemeral)
                {
                    account.runners.remove(&runner_id);
                }
            }
        }
        let now = current_timestamp();
        self.update_job(&job, |run, stored| {
            run.status = Some("completed".to_owned());
            run.conclusion = Some(conclusion.to_string());
            run.updated_at = now;
            stored.status = types::JobStatus::Completed;
            stored.conclusion = Some(conclusion);
            stored.completed_at = Some(now);
            stored.steps = steps
                .iter()
                .map(|step| types::JobStepsItem {
                    completed_at: Some(now),
                    conclusion: Some(step.conclusion.clone()),
                    name: step.name.clone(),
                    number: step.number,
                    started_at: Some(stored.started_at),
                    status: types::JobStepsItemStatus::Completed,
                })
                .collect();
        });
        Some(job)
    }

    fn update_job(
        &mut self,
        job: &ServiceJob,
        change: impl FnOnce(&mut types::WorkflowRun, &mut types::Job),
    ) {
        let Some(repo) = self
            .account_mut(&job.owner)
            .and_then(|account| account.repo_mut(&job.repo))
        else {
            return;
        };
        let run_id = i64::try_from(job.run_id).unwrap();
        let job_id = i64::try_from(job.job_id).unwrap();
        let Some(run) = repo.workflow_runs.iter_mut().find(|run| run.id == run_id) else {
            return;
        };
        let Some(stored) = repo
            .workflow_jobs
            .get_mut(&job.run_id)
            .and_then(|jobs| jobs.iter_mut().find(|stored| stored.id == job_id))
        else {
            return;
        };
        change(run, stored);
    }
}

pub(crate) enum AcquireError {
    NotFound,
    AlreadyAcquired,
}
