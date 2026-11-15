//! Actions workflow runs and their jobs.

use chrono::{DateTime, Utc};

use crate::types::{JobConclusion, JobStatus, JobStepsItem};

/// A workflow run. URLs and the embedded repository are derived when it is rendered.
#[derive(Clone, Debug)]
pub struct WorkflowRunRecord {
    /// Run id.
    pub id: u64,
    /// Workflow name.
    pub name: Option<String>,
    /// Title shown for the run, such as a commit message.
    pub display_title: String,
    /// Workflow file path, such as `.github/workflows/ci.yml`.
    pub path: String,
    /// Workflow id.
    pub workflow_id: u64,
    /// Run number within the workflow.
    pub run_number: u64,
    /// Attempt number, starting at 1.
    pub run_attempt: u64,
    /// Event that triggered the run, such as `push`.
    pub event: String,
    /// Status, such as `queued` or `completed`.
    pub status: Option<String>,
    /// Conclusion of a completed run, such as `success`.
    pub conclusion: Option<String>,
    /// Branch the run checked out.
    pub head_branch: Option<String>,
    /// Commit the run checked out.
    pub head_sha: String,
    /// Browser URL a fixture set; derived from the repository otherwise.
    pub html_url: Option<String>,
    /// When the run was created.
    pub created_at: DateTime<Utc>,
    /// When the run last changed.
    pub updated_at: DateTime<Utc>,
    /// When the current attempt started; absent while it is queued again.
    pub run_started_at: Option<DateTime<Utc>>,
}

/// A job of a workflow run. Its branch, commit and workflow name are its run's.
#[derive(Clone, Debug)]
pub struct JobRecord {
    /// Job id.
    pub id: u64,
    /// Job name.
    pub name: String,
    /// Status.
    pub status: JobStatus,
    /// Conclusion of a completed job.
    pub conclusion: Option<JobConclusion>,
    /// Attempt of the run this job belongs to.
    pub run_attempt: u64,
    /// Runner labels the job asks for.
    pub labels: Vec<String>,
    /// Runner that took the job.
    pub runner_id: Option<u64>,
    /// Name of the runner that took the job.
    pub runner_name: Option<String>,
    /// Group of the runner that took the job.
    pub runner_group_id: Option<u64>,
    /// Steps, in order.
    pub steps: Vec<JobStepsItem>,
    /// When the job was created.
    pub created_at: DateTime<Utc>,
    /// When the job started.
    pub started_at: DateTime<Utc>,
    /// When the job completed.
    pub completed_at: Option<DateTime<Utc>>,
}
