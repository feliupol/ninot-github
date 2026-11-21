//! `gh run list`, `view`, `cancel`, `rerun`, `delete`, `download`, and `watch`.

use ninot_github::fixtures::{JobSeed, SeedField, WorkflowRunSeed};
use ninot_github::server::Repository;

use super::{assert_http, file_texts};
use crate::common::TempDir;
use crate::{api, gh, gh_json, gh_output, gh_output_as, tls_server};

fn job(id: u64, name: &str, status: &str, conclusion: Option<&str>) -> JobSeed {
    JobSeed {
        id: SeedField::present(id),
        name: SeedField::present(name.to_owned()),
        status: SeedField::present(status.to_owned()),
        conclusion: SeedField::present(conclusion.map(str::to_owned)),
        ..Default::default()
    }
}

struct RunSpec {
    id: u64,
    workflow_id: u64,
    name: String,
    path: String,
    branch: String,
    event: String,
    status: String,
    conclusion: Option<String>,
    jobs: Vec<JobSeed>,
}

fn seed(repo: &Repository, spec: RunSpec) -> ninot_github::server::WorkflowRun {
    repo.add_workflow_run(
        WorkflowRunSeed {
            id: SeedField::present(spec.id),
            workflow_id: SeedField::present(spec.workflow_id),
            name: SeedField::present(Some(spec.name)),
            path: SeedField::present(spec.path),
            head_branch: SeedField::present(Some(spec.branch)),
            event: SeedField::present(spec.event),
            status: SeedField::present(Some(spec.status)),
            conclusion: SeedField::present(spec.conclusion),
            ..Default::default()
        },
        spec.jobs,
    )
    .unwrap()
}

#[test]
fn run_filters_logs_watch_cancel_rerun_download_and_delete() {
    let server = tls_server();
    let config = TempDir::new();
    let repo = server
        .add_repository("admin", "tool", false, None, None, "main")
        .unwrap();
    let ci = ".github/workflows/ci.yml";
    let nightly = ".github/workflows/nightly.yml";
    let ci_run = seed(
        &repo,
        RunSpec {
            id: 42,
            workflow_id: 7,
            name: "CI".to_owned(),
            path: ci.to_owned(),
            branch: "main".to_owned(),
            event: "push".to_owned(),
            status: "completed".to_owned(),
            conclusion: Some("success".to_owned()),
            jobs: vec![job(91, "test", "completed", Some("success"))],
        },
    );
    ci_run
        .set_job_log(91, "hello from the job\n".to_owned())
        .unwrap();
    let nightly_run = seed(
        &repo,
        RunSpec {
            id: 43,
            workflow_id: 8,
            name: "Nightly".to_owned(),
            path: nightly.to_owned(),
            branch: "feature".to_owned(),
            event: "schedule".to_owned(),
            status: "completed".to_owned(),
            conclusion: Some("failure".to_owned()),
            jobs: vec![
                job(92, "fail", "completed", Some("failure")),
                job(93, "pass", "completed", Some("success")),
            ],
        },
    );
    nightly_run
        .set_job_log(92, "failure-log-line\n".to_owned())
        .unwrap();
    nightly_run
        .set_job_log(93, "success-log-line\n".to_owned())
        .unwrap();
    seed(
        &repo,
        RunSpec {
            id: 44,
            workflow_id: 7,
            name: "CI".to_owned(),
            path: ci.to_owned(),
            branch: "main".to_owned(),
            event: "workflow_dispatch".to_owned(),
            status: "queued".to_owned(),
            conclusion: None,
            jobs: vec![],
        },
    );
    seed(
        &repo,
        RunSpec {
            id: 45,
            workflow_id: 7,
            name: "CI".to_owned(),
            path: ci.to_owned(),
            branch: "main".to_owned(),
            event: "workflow_dispatch".to_owned(),
            status: "in_progress".to_owned(),
            conclusion: None,
            jobs: vec![job(95, "wait", "in_progress", None)],
        },
    );
    seed(
        &repo,
        RunSpec {
            id: 46,
            workflow_id: 7,
            name: "CI".to_owned(),
            path: ci.to_owned(),
            branch: "other".to_owned(),
            event: "workflow_dispatch".to_owned(),
            status: "completed".to_owned(),
            conclusion: Some("success".to_owned()),
            jobs: vec![job(94, "build", "completed", Some("success"))],
        },
    );
    let listed = gh_json(
        &server,
        &config.0,
        &[
            "run",
            "list",
            "-R",
            "admin/tool",
            "--workflow",
            "CI",
            "--branch",
            "main",
            "--status",
            "success",
            "--event",
            "push",
            "--json",
            "databaseId,status,conclusion,headBranch,event",
        ],
    );
    assert_eq!(
        listed,
        serde_json::json!([{
            "databaseId": 42,
            "status": "completed",
            "conclusion": "success",
            "headBranch": "main",
            "event": "push",
        }])
    );

    let viewed = gh_json(
        &server,
        &config.0,
        &[
            "run",
            "view",
            "42",
            "-R",
            "admin/tool",
            "--json",
            "databaseId,conclusion,jobs",
        ],
    );
    assert_eq!(viewed["databaseId"], 42);
    assert_eq!(viewed["conclusion"], "success");
    assert_eq!(viewed["jobs"][0]["name"], "test");

    let log = gh(
        &server,
        &config.0,
        &["run", "view", "42", "-R", "admin/tool", "--log"],
    );
    assert!(log.contains("hello from the job"), "{log}");
    let failed = gh(
        &server,
        &config.0,
        &["run", "view", "43", "-R", "admin/tool", "--log-failed"],
    );
    assert!(failed.contains("failure-log-line"), "{failed}");
    assert!(!failed.contains("success-log-line"), "{failed}");

    let watched = gh(
        &server,
        &config.0,
        &["run", "watch", "42", "-R", "admin/tool"],
    );
    assert!(watched.contains("already completed"), "{watched}");
    assert!(watched.contains("success"), "{watched}");

    gh(
        &server,
        &config.0,
        &["run", "cancel", "44", "-R", "admin/tool"],
    );
    assert_eq!(
        api(
            &server,
            &config.0,
            "repos/admin/tool/actions/runs/44",
            ".conclusion"
        ),
        "cancelled"
    );
    gh(
        &server,
        &config.0,
        &["run", "cancel", "45", "--force", "-R", "admin/tool"],
    );
    assert_eq!(
        api(
            &server,
            &config.0,
            "repos/admin/tool/actions/runs/45",
            ".conclusion"
        ),
        "cancelled"
    );
    let completed = gh_output(
        &server,
        &config.0,
        &["run", "cancel", "42", "-R", "admin/tool"],
    );
    assert_http(&completed, "Cannot cancel a workflow run that is completed");

    gh(
        &server,
        &config.0,
        &["run", "rerun", "42", "-R", "admin/tool"],
    );
    let rerun = gh_json(
        &server,
        &config.0,
        &[
            "run",
            "view",
            "42",
            "-R",
            "admin/tool",
            "--json",
            "attempt,status",
        ],
    );
    assert_eq!(rerun["attempt"], 2);
    assert_eq!(rerun["status"], "queued");
    assert_eq!(
        api(
            &server,
            &config.0,
            "repos/admin/tool/actions/runs/42/attempts/1",
            ".status"
        ),
        "completed"
    );

    gh(
        &server,
        &config.0,
        &["run", "rerun", "43", "--failed", "-R", "admin/tool"],
    );
    assert_eq!(
        api(
            &server,
            &config.0,
            "repos/admin/tool/actions/runs/43/attempts/1/jobs",
            "[.jobs[] | select(.name == \"fail\") | .conclusion][0]"
        ),
        "failure"
    );
    assert_eq!(
        api(
            &server,
            &config.0,
            "repos/admin/tool/actions/runs/43",
            ".run_attempt"
        ),
        "2"
    );

    gh(
        &server,
        &config.0,
        &["run", "rerun", "--job", "94", "-R", "admin/tool"],
    );
    assert_eq!(
        api(
            &server,
            &config.0,
            "repos/admin/tool/actions/runs/46",
            ".run_attempt"
        ),
        "2"
    );
    assert_eq!(
        api(
            &server,
            &config.0,
            "repos/admin/tool/actions/jobs/94",
            ".status"
        ),
        "queued"
    );

    let artifact = repo.add_artifact(42, "report", b"artifact-bytes").unwrap();
    let extra = repo.add_artifact(42, "extra", b"extra-bytes").unwrap();
    assert_eq!(
        api(
            &server,
            &config.0,
            &format!("repos/admin/tool/actions/artifacts/{artifact}"),
            ".name"
        ),
        "report"
    );
    gh(
        &server,
        &config.0,
        &[
            "run",
            "download",
            "42",
            "-R",
            "admin/tool",
            "-n",
            "report",
            "-D",
            "out",
        ],
    );
    let texts = file_texts(&config.0.join("out"));
    assert!(
        texts.iter().any(|text| text.contains("artifact-bytes")),
        "{texts:?}"
    );
    // Without a run id, gh picks the artifact from the repository's list.
    gh(
        &server,
        &config.0,
        &[
            "run",
            "download",
            "-R",
            "admin/tool",
            "-n",
            "report",
            "-D",
            "repo-out",
        ],
    );
    let texts = file_texts(&config.0.join("repo-out"));
    assert!(
        texts.iter().any(|text| text.contains("artifact-bytes")),
        "{texts:?}"
    );
    gh(
        &server,
        &config.0,
        &[
            "api",
            "--method",
            "DELETE",
            &format!("repos/admin/tool/actions/artifacts/{extra}"),
            "--silent",
        ],
    );
    let gone = gh_output(
        &server,
        &config.0,
        &[
            "api",
            &format!("repos/admin/tool/actions/artifacts/{extra}"),
        ],
    );
    assert_http(&gone, "404");

    let deleted = gh(
        &server,
        &config.0,
        &["run", "delete", "46", "-R", "admin/tool"],
    );
    assert!(deleted.contains("delete"), "{deleted}");
    let missing = gh_output(
        &server,
        &config.0,
        &["run", "view", "46", "-R", "admin/tool"],
    );
    assert_http(&missing, "404");
}

#[test]
fn run_cancel_is_not_found_or_forbidden() {
    let server = tls_server();
    let config = TempDir::new();
    let repo = server
        .add_repository("admin", "tool", false, None, None, "main")
        .unwrap();
    seed(
        &repo,
        RunSpec {
            id: 42,
            workflow_id: 7,
            name: "CI".to_owned(),
            path: ".github/workflows/ci.yml".to_owned(),
            branch: "main".to_owned(),
            event: "push".to_owned(),
            status: "queued".to_owned(),
            conclusion: None,
            jobs: vec![],
        },
    );
    let missing = gh_output(
        &server,
        &config.0,
        &["run", "view", "999", "-R", "admin/tool"],
    );
    assert_http(&missing, "404");
    let missing_cancel = gh_output(
        &server,
        &config.0,
        &["run", "cancel", "999", "-R", "admin/tool"],
    );
    assert_http(&missing_cancel, "Could not find any workflow run");
    let bob = server.add_user("bob").add_token(&["repo"]);
    let forbidden = gh_output_as(
        &server,
        &config.0,
        &bob,
        &["run", "cancel", "42", "-R", "admin/tool"],
    );
    assert_http(&forbidden, "403");
}
