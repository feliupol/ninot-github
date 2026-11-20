//! Remaining `gh issue` and `gh label clone` commands against the fake.

use std::path::Path;

use ninot_github::Server;
use serde_json::json;

use crate::common::TempDir;
use crate::{api, gh, gh_json, tls_server};

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
fn issue_list_filters_by_assignee_author_and_json_fields() {
    let server = tls_server();
    server.add_user("bob");
    let config = TempDir::new();
    repo_with_issue(&server, &config.0);
    gh(
        &server,
        &config.0,
        &[
            "api",
            "repos/admin/tool/issues",
            "-f",
            "title=Assigned",
            "-f",
            "assignees[]=admin",
            "--silent",
        ],
    );
    gh(
        &server,
        &config.0,
        &[
            "api",
            "-X",
            "PATCH",
            "repos/admin/tool/issues/1",
            "-f",
            "state=closed",
            "--silent",
        ],
    );

    let mine = gh_json(
        &server,
        &config.0,
        &[
            "issue",
            "list",
            "-R",
            "admin/tool",
            "--assignee",
            "admin",
            "--json",
            "number,title,assignees",
        ],
    );
    assert_eq!(mine[0]["number"], 2);
    assert_eq!(mine[0]["assignees"][0]["login"], "admin");

    let authored = gh_json(
        &server,
        &config.0,
        &[
            "issue",
            "list",
            "-R",
            "admin/tool",
            "--author",
            "admin",
            "--state",
            "all",
            "--json",
            "number,author,state,createdAt,updatedAt,url",
        ],
    );
    assert_eq!(authored.as_array().unwrap().len(), 2);
    assert_eq!(authored[0]["author"]["login"], "admin");
}

#[test]
fn issue_list_search_and_milestone() {
    let server = tls_server();
    let config = TempDir::new();
    repo_with_issue(&server, &config.0);
    gh(
        &server,
        &config.0,
        &[
            "api",
            "repos/admin/tool/milestones",
            "-f",
            "title=Ship",
            "--silent",
        ],
    );
    gh(
        &server,
        &config.0,
        &[
            "api",
            "repos/admin/tool/issues",
            "-f",
            "title=Milestone work",
            "-F",
            "milestone=1",
            "--silent",
        ],
    );

    let searched = gh_json(
        &server,
        &config.0,
        &[
            "issue",
            "list",
            "-R",
            "admin/tool",
            "--search",
            "Milestone",
            "--json",
            "number,title",
        ],
    );
    assert_eq!(
        searched,
        json!([{ "number": 2, "title": "Milestone work" }])
    );

    let milestoned = gh_json(
        &server,
        &config.0,
        &[
            "issue",
            "list",
            "-R",
            "admin/tool",
            "--milestone",
            "Ship",
            "--json",
            "number,milestone",
        ],
    );
    assert_eq!(milestoned[0]["number"], 2);
    assert_eq!(milestoned[0]["milestone"]["title"], "Ship");
}

#[test]
fn issue_view_comments_and_json() {
    let server = tls_server();
    let config = TempDir::new();
    repo_with_issue(&server, &config.0);
    gh(
        &server,
        &config.0,
        &[
            "api",
            "repos/admin/tool/issues/1/comments",
            "-f",
            "body=Seen it.",
            "--silent",
        ],
    );

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
            "number,title,comments,author,createdAt,url,isPinned",
        ],
    );
    assert_eq!(viewed["number"], 1);
    assert_eq!(viewed["comments"][0]["body"], "Seen it.");
    assert_eq!(viewed["isPinned"], false);
    assert_eq!(
        api(&server, &config.0, "repos/admin/tool/issues/1", ".locked"),
        "false"
    );

    let comments = gh(
        &server,
        &config.0,
        &["issue", "view", "1", "-R", "admin/tool", "--comments"],
    );
    assert!(comments.contains("Seen it."), "{comments}");
}

#[test]
fn issue_status_groups_assigned_and_authored() {
    let server = tls_server();
    let config = TempDir::new();
    repo_with_issue(&server, &config.0);
    gh(
        &server,
        &config.0,
        &[
            "api",
            "-X",
            "PATCH",
            "repos/admin/tool/issues/1",
            "-f",
            "assignees[]=admin",
            "--silent",
        ],
    );
    let status = gh(&server, &config.0, &["issue", "status", "-R", "admin/tool"]);
    assert!(status.contains("Broken"), "{status}");
    assert!(
        status.to_lowercase().contains("assigned") || status.contains("#1"),
        "{status}"
    );
}

mod mutate;
