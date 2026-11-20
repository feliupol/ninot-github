//! Remaining `gh pr` commands: checkout, status, lock, revert, and the list, view,
//! merge, close, and review flags the other module does not drive.

mod fields;
mod write;

use std::path::Path;
use std::process::Command;

use ninot_github::fixtures::{CheckRunSeed, CommitStatusSeed, SeedField};
use serde_json::Value;

use crate::common::TempDir;
use crate::{
    api, gh, gh_json, gh_output, isolated, log_coverage, open_pull, repo_with_branches, test_name,
    tls_server,
};

/// Runs `gh` in a clone.
///
/// Git remotes expose `url.Hostname`, which drops the server port, while the API
/// needs that port. `GH_HOST` is the hostname alone so remote matching succeeds,
/// and `GH_REPO` is `host:port/owner/name` so API calls keep the port.
pub(super) fn gh_in(
    server: &ninot_github::Server,
    config: &Path,
    dir: &Path,
    args: &[&str],
) -> String {
    log_coverage(serde_json::json!({ "test": test_name(), "gh": args }));
    let endpoint = server.url().strip_prefix("https://").unwrap();
    let hostname = endpoint.split(':').next().unwrap_or(endpoint);
    let repo = format!("{endpoint}/admin/tool");
    let output = isolated(Command::new("gh"), server, config)
        .env("GH_HOST", hostname)
        .env("GH_REPO", &repo)
        .env("GH_ENTERPRISE_TOKEN", server.admin_token())
        .args(args)
        .current_dir(dir)
        .output()
        .expect("cannot run gh");
    assert!(
        output.status.success(),
        "gh {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

pub(super) fn graphql(server: &ninot_github::Server, config: &Path, query: &str) -> Value {
    let field = format!("query={query}");
    gh_json(server, config, &["api", "graphql", "-f", &field])
}

#[test]
fn pr_list_filters_author_base_head_label_draft_and_search() {
    let server = tls_server();
    let config = TempDir::new();
    repo_with_branches(&server, &config.0, &["feature", "draft"]);
    open_pull(&server, &config.0, "feature", false);
    open_pull(&server, &config.0, "draft", true);
    gh(
        &server,
        &config.0,
        &[
            "label",
            "create",
            "bug",
            "-R",
            "admin/tool",
            "--color",
            "d73a4a",
        ],
    );
    gh(
        &server,
        &config.0,
        &[
            "api",
            "-X",
            "POST",
            "repos/admin/tool/issues/1/labels",
            "-f",
            "labels[]=bug",
            "--silent",
        ],
    );

    let listed = gh_json(
        &server,
        &config.0,
        &[
            "pr",
            "list",
            "-R",
            "admin/tool",
            "--author",
            "admin",
            "--base",
            "main",
            "--head",
            "feature",
            "--json",
            "number,title",
        ],
    );
    assert_eq!(listed[0]["number"], 1);
    assert_eq!(listed[0]["title"], "Add feature");
    assert_eq!(listed.as_array().map(Vec::len), Some(1));

    let labeled = gh_json(
        &server,
        &config.0,
        &[
            "pr",
            "list",
            "-R",
            "admin/tool",
            "--label",
            "bug",
            "--json",
            "number",
        ],
    );
    assert_eq!(labeled[0]["number"], 1);

    let drafts = gh_json(
        &server,
        &config.0,
        &[
            "pr",
            "list",
            "-R",
            "admin/tool",
            "--draft",
            "--json",
            "number,isDraft",
        ],
    );
    assert_eq!(drafts[0]["number"], 2);
    assert_eq!(drafts[0]["isDraft"], true);

    let searched = gh_json(
        &server,
        &config.0,
        &[
            "pr",
            "list",
            "-R",
            "admin/tool",
            "--search",
            "feature",
            "--json",
            "number",
        ],
    );
    assert_eq!(searched[0]["number"], 1);
}

#[test]
fn pr_view_comments_reviews_files_commits_and_checks() {
    let server = tls_server();
    let config = TempDir::new();
    repo_with_branches(&server, &config.0, &["feature"]);
    open_pull(&server, &config.0, "feature", false);
    let head = api(&server, &config.0, "repos/admin/tool/pulls/1", ".head.sha");
    let commit = server
        .repository("admin", "tool")
        .unwrap()
        .commit(&head)
        .unwrap();
    commit
        .create_status(CommitStatusSeed {
            state: SeedField::present("success".to_owned()),
            context: SeedField::present("tests".to_owned()),
            ..Default::default()
        })
        .unwrap();
    commit
        .create_check_run(CheckRunSeed {
            name: SeedField::present("lint".to_owned()),
            status: SeedField::present("completed".to_owned()),
            conclusion: SeedField::present(Some("success".to_owned())),
            ..Default::default()
        })
        .unwrap();
    gh(
        &server,
        &config.0,
        &[
            "pr",
            "comment",
            "1",
            "-R",
            "admin/tool",
            "--body",
            "Looks fine.",
        ],
    );
    gh(
        &server,
        &config.0,
        &[
            "pr",
            "review",
            "1",
            "-R",
            "admin/tool",
            "--comment",
            "--body",
            "Ship it.",
        ],
    );

    let text = gh(
        &server,
        &config.0,
        &["pr", "view", "1", "-R", "admin/tool", "--comments"],
    );
    assert!(text.contains("Looks fine."), "{text}");

    let viewed = gh_json(
        &server,
        &config.0,
        &[
            "pr",
            "view",
            "1",
            "-R",
            "admin/tool",
            "--json",
            "reviews,files,commits,statusCheckRollup,comments",
        ],
    );
    let raw = viewed.to_string();
    assert!(raw.contains("Ship it."), "{viewed}");
    assert!(raw.contains("feature.txt"), "{viewed}");
    assert!(raw.contains("Add feature"), "{viewed}");
    assert!(raw.contains("tests"), "{viewed}");
    assert!(raw.contains("lint"), "{viewed}");
    assert!(raw.contains("Looks fine."), "{viewed}");
}

#[test]
fn pr_view_missing_number_is_not_found() {
    let server = tls_server();
    let config = TempDir::new();
    repo_with_branches(&server, &config.0, &["feature"]);
    let output = gh_output(
        &server,
        &config.0,
        &["pr", "view", "99", "-R", "admin/tool"],
    );
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Could not resolve") || stderr.to_lowercase().contains("not found"),
        "{stderr}"
    );
}
