//! `gh issue` against the fake.

use std::path::Path;

use ninot_github::Server;

use crate::common::TempDir;
use crate::{api, gh, gh_json, open_pull, repo_with_branches, tls_server};

/// Creates `admin/tool`, the labels `bug` and `docs`, and issue 1 through REST.
fn repo_with_issue(server: &Server, config: &Path) {
    gh(
        server,
        config,
        &["repo", "create", "admin/tool", "--public", "--add-readme"],
    );
    for label in ["bug", "docs"] {
        gh(
            server,
            config,
            &["label", "create", label, "-R", "admin/tool"],
        );
    }
    gh(
        server,
        config,
        &[
            "api",
            "repos/admin/tool/issues",
            "-f",
            "title=Broken",
            "-f",
            "body=It fails.",
            "-f",
            "labels[]=bug",
            "--silent",
        ],
    );
}

#[test]
#[ignore = "not implemented: GraphQL Repository.labels, which `gh issue create --label` reads, and Mutation.createIssue"]
fn issue_create_with_labels() {
    let server = tls_server();
    let config = TempDir::new();
    gh(
        &server,
        &config.0,
        &["repo", "create", "admin/tool", "--public", "--add-readme"],
    );
    gh(
        &server,
        &config.0,
        &["label", "create", "bug", "-R", "admin/tool"],
    );

    let url = gh(
        &server,
        &config.0,
        &[
            "issue",
            "create",
            "-R",
            "admin/tool",
            "--title",
            "Broken",
            "--body",
            "It fails.",
            "--label",
            "bug",
        ],
    );
    assert!(url.trim().ends_with("/admin/tool/issues/1"), "{url}");
    assert_eq!(
        api(
            &server,
            &config.0,
            "repos/admin/tool/issues/1",
            "[.title, .labels[0].name] | join(\",\")"
        ),
        "Broken,bug"
    );
}

#[test]
#[ignore = "not implemented: GraphQL Repository.issueOrPullRequest"]
fn issue_view_shows_the_issue() {
    let server = tls_server();
    let config = TempDir::new();
    repo_with_issue(&server, &config.0);

    let viewed = gh_json(
        &server,
        &config.0,
        &[
            "issue",
            "view",
            "1",
            "-R",
            "admin/tool",
            "--json",
            "number,title,body,state,labels",
        ],
    );
    assert_eq!(viewed["title"], "Broken");
    assert_eq!(viewed["body"], "It fails.");
    assert_eq!(viewed["state"], "OPEN");
    assert_eq!(viewed["labels"][0]["name"], "bug");
}

#[test]
#[ignore = "not implemented: REST meta/get, which gh reads to detect search features of an Enterprise host, and GraphQL Repository.issues"]
fn issue_list_filters_by_state_and_label() {
    let server = tls_server();
    let config = TempDir::new();
    repo_with_issue(&server, &config.0);
    gh(
        &server,
        &config.0,
        &[
            "api",
            "repos/admin/tool/issues",
            "-f",
            "title=Typo",
            "-f",
            "labels[]=docs",
            "--silent",
        ],
    );

    let listed = gh_json(
        &server,
        &config.0,
        &[
            "issue",
            "list",
            "-R",
            "admin/tool",
            "--label",
            "docs",
            "--json",
            "number,title",
        ],
    );
    assert_eq!(
        listed,
        serde_json::json!([{ "number": 2, "title": "Typo" }])
    );
    let all = gh_json(
        &server,
        &config.0,
        &[
            "issue",
            "list",
            "-R",
            "admin/tool",
            "--state",
            "all",
            "--json",
            "number",
        ],
    );
    assert_eq!(all.as_array().unwrap().len(), 2);
}

#[test]
#[ignore = "not implemented: GraphQL Repository.issueOrPullRequest"]
fn issue_comment_adds_a_comment() {
    let server = tls_server();
    let config = TempDir::new();
    repo_with_issue(&server, &config.0);

    gh(
        &server,
        &config.0,
        &[
            "issue",
            "comment",
            "1",
            "-R",
            "admin/tool",
            "--body",
            "Seen it.",
        ],
    );
    assert_eq!(
        api(
            &server,
            &config.0,
            "repos/admin/tool/issues/1/comments",
            ".[].body"
        ),
        "Seen it."
    );
}

#[test]
#[ignore = "not implemented: GraphQL Repository.issueOrPullRequest"]
fn issue_edit_changes_title_body_and_labels() {
    let server = tls_server();
    let config = TempDir::new();
    repo_with_issue(&server, &config.0);

    gh(
        &server,
        &config.0,
        &[
            "issue",
            "edit",
            "1",
            "-R",
            "admin/tool",
            "--title",
            "Still broken",
            "--body",
            "Worse.",
            "--add-label",
            "docs",
            "--remove-label",
            "bug",
        ],
    );
    assert_eq!(
        api(
            &server,
            &config.0,
            "repos/admin/tool/issues/1",
            "[.title, .body, (.labels | map(.name) | join(\"+\"))] | join(\",\")"
        ),
        "Still broken,Worse.,docs"
    );
}

#[test]
#[ignore = "not implemented: GraphQL Repository.issueOrPullRequest"]
fn issue_close_and_reopen() {
    let server = tls_server();
    let config = TempDir::new();
    repo_with_issue(&server, &config.0);

    gh(
        &server,
        &config.0,
        &[
            "issue",
            "close",
            "1",
            "-R",
            "admin/tool",
            "--reason",
            "not planned",
        ],
    );
    assert_eq!(
        api(
            &server,
            &config.0,
            "repos/admin/tool/issues/1",
            "[.state, .state_reason] | join(\",\")"
        ),
        "closed,not_planned"
    );
    gh(
        &server,
        &config.0,
        &["issue", "reopen", "1", "-R", "admin/tool"],
    );
    assert_eq!(
        api(&server, &config.0, "repos/admin/tool/issues/1", ".state"),
        "open"
    );
}

/// Issues and pull requests share one number sequence, and every pull request is an issue:
/// <https://docs.github.com/en/rest/issues/issues#list-repository-issues>.
#[test]
#[ignore = "fake bug: a pull request opened after an issue reuses the issue's number"]
fn pull_requests_and_issues_share_numbers() {
    let server = tls_server();
    let config = TempDir::new();
    repo_with_branches(&server, &config.0, &["feature"]);
    gh(
        &server,
        &config.0,
        &[
            "api",
            "repos/admin/tool/issues",
            "-f",
            "title=First",
            "--silent",
        ],
    );

    assert_eq!(open_pull(&server, &config.0, "feature", false), "2");
    assert_eq!(
        api(
            &server,
            &config.0,
            "repos/admin/tool/issues?state=all",
            "[.[] | [.number, (.pull_request != null)] | map(tostring) | join(\":\")] | join(\",\")"
        ),
        "2:true,1:false"
    );
}
