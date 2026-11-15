//! `gh run` and `gh workflow` against the fake.

use ninot_github::Server;
use ninot_github::fixtures::{JobSeed, SeedField, WorkflowRunSeed};

use crate::common::TempDir;
use crate::{gh, gh_json, tls_server};

/// Creates `admin/tool` with the completed run 42 of `CI` and its job 91.
fn repo_with_run(server: &Server) {
    let repo = server
        .create_repository("admin", "tool", false, None, None, "main")
        .unwrap();
    repo.create_workflow_run(
        WorkflowRunSeed {
            id: SeedField::present(42),
            name: SeedField::present(Some("CI".to_owned())),
            head_branch: SeedField::present(Some("main".to_owned())),
            event: SeedField::present("push".to_owned()),
            status: SeedField::present(Some("completed".to_owned())),
            conclusion: SeedField::present(Some("success".to_owned())),
            ..Default::default()
        },
        vec![JobSeed {
            id: SeedField::present(91),
            name: SeedField::present("test".to_owned()),
            status: SeedField::present("completed".to_owned()),
            conclusion: SeedField::present(Some("success".to_owned())),
            ..Default::default()
        }],
    )
    .unwrap();
}

#[test]
fn run_list_of_a_repository_without_runs() {
    let server = tls_server();
    let config = TempDir::new();
    gh(
        &server,
        &config.0,
        &["repo", "create", "admin/tool", "--public"],
    );

    let runs = gh_json(
        &server,
        &config.0,
        &[
            "run",
            "list",
            "-R",
            "admin/tool",
            "--json",
            "databaseId,status",
        ],
    );
    assert_eq!(runs, serde_json::json!([]));
}

/// `gh run list` names each run's workflow from `GET /repos/{owner}/{repo}/actions/workflows`
/// (cli/cli v2.101.0 pkg/cmd/run/shared/shared.go:446).
#[test]
#[ignore = "not implemented: REST actions/list-repo-workflows"]
fn run_list_shows_runs() {
    let server = tls_server();
    let config = TempDir::new();
    repo_with_run(&server);

    let runs = gh_json(
        &server,
        &config.0,
        &[
            "run",
            "list",
            "-R",
            "admin/tool",
            "--json",
            "databaseId,status,conclusion,headBranch,event",
        ],
    );
    assert_eq!(
        runs,
        serde_json::json!([{
            "databaseId": 42,
            "status": "completed",
            "conclusion": "success",
            "headBranch": "main",
            "event": "push",
        }])
    );
}

/// `gh run view` names the run's workflow from `GET /repos/{owner}/{repo}/actions/workflows/{id}`
/// (cli/cli v2.101.0 pkg/cmd/run/shared/shared.go:588).
#[test]
#[ignore = "not implemented: REST actions/get-workflow"]
fn run_view_shows_the_run_and_its_jobs() {
    let server = tls_server();
    let config = TempDir::new();
    repo_with_run(&server);

    let run = gh_json(
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
    assert_eq!(run["databaseId"], 42);
    assert_eq!(run["conclusion"], "success");
    assert_eq!(run["jobs"][0]["name"], "test");
}

#[test]
#[ignore = "not implemented: REST actions/list-repo-workflows"]
fn workflow_list_and_view() {
    let server = tls_server();
    let config = TempDir::new();
    repo_with_run(&server);

    let workflows = gh_json(
        &server,
        &config.0,
        &[
            "workflow",
            "list",
            "-R",
            "admin/tool",
            "--all",
            "--json",
            "name,state",
        ],
    );
    assert_eq!(workflows[0]["name"], "CI");
    let viewed = gh(
        &server,
        &config.0,
        &["workflow", "view", "CI", "-R", "admin/tool"],
    );
    assert!(viewed.contains("CI"), "{viewed}");
}
