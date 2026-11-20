//! `gh workflow list`, `view`, `enable`, `disable`, and `run`.

use serde_json::Value;

use super::{assert_http, push_workflows};
use crate::common::TempDir;
use crate::{api, gh, gh_json, gh_output, gh_output_as, tls_server};

fn named<'a>(workflows: &'a Value, name: &str) -> &'a Value {
    workflows
        .as_array()
        .unwrap()
        .iter()
        .find(|workflow| workflow["name"] == name)
        .unwrap_or_else(|| panic!("no workflow {name} in {workflows}"))
}

#[test]
fn workflow_list_view_enable_disable_and_dispatch() {
    let server = tls_server();
    let config = TempDir::new();
    push_workflows(&server, &config.0);

    let workflows = gh_json(
        &server,
        &config.0,
        &[
            "workflow",
            "list",
            "-R",
            "admin/tool",
            "--json",
            "id,name,path,state",
        ],
    );
    let ci = named(&workflows, "CI");
    assert_eq!(ci["state"], "active");
    assert_eq!(ci["path"], ".github/workflows/ci.yml");
    assert!(named(&workflows, "Nightly")["id"].is_number());
    let viewed = gh(
        &server,
        &config.0,
        &["workflow", "view", "CI", "-R", "admin/tool"],
    );
    assert!(viewed.contains("CI"), "{viewed}");
    assert!(viewed.contains("ci.yml"), "{viewed}");

    let workflow = api(
        &server,
        &config.0,
        "repos/admin/tool/actions/workflows/ci.yml",
        "{id: .id, state: .state, badge: .badge_url, html: .html_url}",
    );
    assert!(workflow.contains("active"), "{workflow}");
    assert!(workflow.contains("/badge.svg"), "{workflow}");
    assert!(workflow.contains(".github/workflows/ci.yml"), "{workflow}");

    let printed = gh(
        &server,
        &config.0,
        &[
            "workflow",
            "run",
            "ci.yml",
            "-f",
            "name=ada",
            "--ref",
            "feature",
            "-R",
            "admin/tool",
        ],
    );
    assert!(printed.contains("/actions/runs/"), "{printed}");
    let runs = gh_json(
        &server,
        &config.0,
        &[
            "run",
            "list",
            "-R",
            "admin/tool",
            "--workflow",
            "CI",
            "--json",
            "event,headBranch,status",
        ],
    );
    assert_eq!(runs.as_array().unwrap().len(), 1, "{runs}");
    assert_eq!(runs[0]["event"], "workflow_dispatch");
    assert_eq!(runs[0]["headBranch"], "feature");
    assert_eq!(runs[0]["status"], "queued");

    gh(
        &server,
        &config.0,
        &["workflow", "disable", "Nightly", "-R", "admin/tool"],
    );
    let active = gh_json(
        &server,
        &config.0,
        &[
            "workflow",
            "list",
            "-R",
            "admin/tool",
            "--json",
            "name,state",
        ],
    );
    assert!(named(&active, "CI")["name"].is_string());
    assert!(
        active
            .as_array()
            .unwrap()
            .iter()
            .all(|workflow| workflow["name"] != "Nightly"),
        "{active}"
    );
    let all = gh_json(
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
    assert_eq!(named(&all, "Nightly")["state"], "disabled_manually");
    gh(
        &server,
        &config.0,
        &["workflow", "enable", "Nightly", "-R", "admin/tool"],
    );
    let restored = gh_json(
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
    assert_eq!(named(&restored, "Nightly")["state"], "active");
}

#[test]
fn workflow_failures_are_not_found_disabled_and_forbidden() {
    let server = tls_server();
    let config = TempDir::new();
    push_workflows(&server, &config.0);

    let missing = gh_output(
        &server,
        &config.0,
        &["workflow", "view", "999999", "-R", "admin/tool"],
    );
    assert_http(&missing, "404");

    let unknown_ref = gh_output(
        &server,
        &config.0,
        &[
            "workflow",
            "run",
            "CI",
            "--ref",
            "no-such-branch",
            "-R",
            "admin/tool",
        ],
    );
    assert_http(&unknown_ref, "422");

    let no_dispatch = gh_output(
        &server,
        &config.0,
        &[
            "api",
            "--method",
            "POST",
            "repos/admin/tool/actions/workflows/lint.yml/dispatches",
            "-f",
            "ref=main",
        ],
    );
    assert_http(&no_dispatch, "422");

    gh(
        &server,
        &config.0,
        &["workflow", "disable", "Nightly", "-R", "admin/tool"],
    );
    let disabled = gh_output(
        &server,
        &config.0,
        &[
            "api",
            "--method",
            "POST",
            "repos/admin/tool/actions/workflows/nightly.yml/dispatches",
            "-f",
            "ref=main",
        ],
    );
    assert_http(&disabled, "422");

    let bob = server.add_user("bob").add_token(&["repo"]);
    let forbidden = gh_output_as(
        &server,
        &config.0,
        &bob,
        &["workflow", "disable", "CI", "-R", "admin/tool"],
    );
    assert_http(&forbidden, "403");
}
