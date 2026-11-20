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

/// A workflow file on the default branch, or one synthesized for a seeded run.
#[derive(Clone, Debug)]
pub struct WorkflowRecord {
    /// Workflow id. Stable across reads.
    pub id: u64,
    /// Name from the file's top-level `name:`, or the run that synthesized it.
    pub name: String,
    /// Repo-relative path, such as `.github/workflows/ci.yml`.
    pub path: String,
    /// `active`, `disabled_manually`, or `deleted`.
    pub state: String,
    /// Set when the record was read from the default branch. Seeded runs leave this false.
    pub on_branch: bool,
    /// Workflow YAML. Empty for a workflow synthesized from a run.
    pub content: String,
    /// When the workflow was first recorded.
    pub created_at: DateTime<Utc>,
    /// When the workflow last changed.
    pub updated_at: DateTime<Utc>,
}

/// An Actions artifact attached to one workflow run. `zip` is the archive `download` serves.
#[derive(Clone, Debug)]
pub struct ArtifactRecord {
    /// Artifact id.
    pub id: u64,
    /// Artifact name.
    pub name: String,
    /// Workflow run the artifact belongs to.
    pub run_id: u64,
    /// Uncompressed size reported by the API.
    pub size_in_bytes: i64,
    /// Whether the artifact is past its expiry.
    pub expired: bool,
    /// When the artifact was created.
    pub created_at: DateTime<Utc>,
    /// When the artifact expires.
    pub expires_at: Option<DateTime<Utc>>,
    /// When the artifact last changed.
    pub updated_at: DateTime<Utc>,
    /// Zip archive whose single entry is named `name`.
    pub zip: Vec<u8>,
}

/// One GitHub Actions cache entry.
#[derive(Clone, Debug)]
pub struct CacheRecord {
    /// Cache id.
    pub id: u64,
    /// Cache key.
    pub key: String,
    /// Full git ref, such as `refs/heads/main`.
    pub git_ref: String,
    /// Cache version token.
    pub version: String,
    /// Size in bytes.
    pub size_in_bytes: i64,
    /// When the cache was created.
    pub created_at: DateTime<Utc>,
    /// When the cache was last accessed.
    pub last_accessed_at: DateTime<Utc>,
}

/// A previous attempt of a workflow run, kept so reruns can still be read.
#[derive(Clone, Debug)]
pub struct AttemptRecord {
    /// The run as it was at the end of that attempt.
    pub run: WorkflowRunRecord,
    /// Jobs as they were at the end of that attempt.
    pub jobs: Vec<JobRecord>,
}
