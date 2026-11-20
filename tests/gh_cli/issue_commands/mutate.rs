use super::repo_with_issue;
use crate::common::TempDir;
use crate::{api, gh, gh_json, gh_output, gh_output_as, repo_with_branches, tls_server};

#[test]
fn issue_lock_and_unlock_with_reason() {
    let server = tls_server();
    let config = TempDir::new();
    repo_with_issue(&server, &config.0);

    gh(
        &server,
        &config.0,
        &[
            "issue",
            "lock",
            "1",
            "-R",
            "admin/tool",
            "--reason",
            "resolved",
        ],
    );
    assert_eq!(
        api(
            &server,
            &config.0,
            "repos/admin/tool/issues/1",
            "[.locked, .active_lock_reason] | join(\",\")"
        ),
        "true,resolved"
    );
    gh(
        &server,
        &config.0,
        &["issue", "unlock", "1", "-R", "admin/tool"],
    );
    assert_eq!(
        api(&server, &config.0, "repos/admin/tool/issues/1", ".locked"),
        "false"
    );
}

#[test]
fn issue_pin_and_unpin() {
    let server = tls_server();
    let config = TempDir::new();
    repo_with_issue(&server, &config.0);

    gh(
        &server,
        &config.0,
        &["issue", "pin", "1", "-R", "admin/tool"],
    );
    let pinned = gh_json(
        &server,
        &config.0,
        &[
            "issue",
            "view",
            "1",
            "-R",
            "admin/tool",
            "--json",
            "isPinned",
        ],
    );
    assert_eq!(pinned["isPinned"], true);
    gh(
        &server,
        &config.0,
        &["issue", "unpin", "1", "-R", "admin/tool"],
    );
    let unpinned = gh_json(
        &server,
        &config.0,
        &[
            "issue",
            "view",
            "1",
            "-R",
            "admin/tool",
            "--json",
            "isPinned",
        ],
    );
    assert_eq!(unpinned["isPinned"], false);
}

#[test]
fn issue_delete_yes() {
    let server = tls_server();
    let config = TempDir::new();
    repo_with_issue(&server, &config.0);

    gh(
        &server,
        &config.0,
        &["issue", "delete", "1", "-R", "admin/tool", "--yes"],
    );
    let missing = gh_output(
        &server,
        &config.0,
        &["issue", "view", "1", "-R", "admin/tool"],
    );
    assert!(!missing.status.success());
}

#[test]
fn issue_transfer_to_another_repo_of_the_same_owner() {
    let server = tls_server();
    let config = TempDir::new();
    repo_with_issue(&server, &config.0);
    gh(
        &server,
        &config.0,
        &["repo", "create", "admin/other", "--public"],
    );

    let url = gh(
        &server,
        &config.0,
        &["issue", "transfer", "1", "admin/other", "-R", "admin/tool"],
    );
    assert!(url.trim().ends_with("/admin/other/issues/1"), "{url}");
    assert_eq!(
        api(&server, &config.0, "repos/admin/other/issues/1", ".title"),
        "Broken"
    );
}

#[test]
fn issue_develop_lists_and_creates_a_linked_branch() {
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
            "title=Broken",
            "--silent",
        ],
    );

    let created = gh(
        &server,
        &config.0,
        &[
            "issue",
            "develop",
            "1",
            "-R",
            "admin/tool",
            "--name",
            "fix-broken",
        ],
    );
    assert!(created.contains("fix-broken"), "{created}");
    let listed = gh(
        &server,
        &config.0,
        &["issue", "develop", "1", "-R", "admin/tool", "--list"],
    );
    assert!(listed.contains("fix-broken"), "{listed}");
}

#[test]
fn issue_close_with_comment_and_not_planned() {
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
            "--comment",
            "Won't fix.",
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
    assert_eq!(
        api(
            &server,
            &config.0,
            "repos/admin/tool/issues/1/comments",
            ".[-1].body"
        ),
        "Won't fix."
    );
}

#[test]
fn label_clone_copies_labels_into_another_repository() {
    let server = tls_server();
    let config = TempDir::new();
    gh(
        &server,
        &config.0,
        &["repo", "create", "admin/tool", "--public"],
    );
    gh(
        &server,
        &config.0,
        &["repo", "create", "admin/other", "--public"],
    );
    gh(
        &server,
        &config.0,
        &[
            "label",
            "create",
            "bug",
            "-R",
            "admin/tool",
            "-c",
            "d73a4a",
            "-d",
            "Broken",
        ],
    );

    gh(
        &server,
        &config.0,
        &["label", "clone", "admin/tool", "-R", "admin/other"],
    );
    assert_eq!(
        api(
            &server,
            &config.0,
            "repos/admin/other/labels/bug",
            "[.name, .color, .description] | join(\",\")"
        ),
        "bug,d73a4a,Broken"
    );
}

#[test]
fn issue_view_missing_issue_is_not_found() {
    let server = tls_server();
    let config = TempDir::new();
    repo_with_issue(&server, &config.0);
    let missing = gh_output(
        &server,
        &config.0,
        &["issue", "view", "99", "-R", "admin/tool"],
    );
    assert!(!missing.status.success());
}

#[test]
fn issue_lock_missing_issue_is_not_found() {
    let server = tls_server();
    let config = TempDir::new();
    repo_with_issue(&server, &config.0);
    let missing = gh_output(
        &server,
        &config.0,
        &[
            "issue",
            "lock",
            "99",
            "-R",
            "admin/tool",
            "--reason",
            "spam",
        ],
    );
    assert!(!missing.status.success());
}

#[test]
fn issue_lock_without_push_is_denied() {
    let server = tls_server();
    let config = TempDir::new();
    repo_with_issue(&server, &config.0);
    let bob = server.add_user("bob").add_token(&["repo"]);
    let denied = gh_output_as(
        &server,
        &config.0,
        &bob,
        &["issue", "lock", "1", "-R", "admin/tool", "--reason", "spam"],
    );
    assert!(!denied.status.success());
    let stderr = String::from_utf8_lossy(&denied.stderr);
    assert!(
        stderr.to_lowercase().contains("forbidden") || stderr.to_lowercase().contains("push"),
        "{stderr}"
    );
}
