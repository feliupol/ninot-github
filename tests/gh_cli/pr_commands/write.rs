//! Mutating `gh pr` commands.

use super::gh_in;
use crate::common::TempDir;
use crate::{
    api, gh, gh_as, gh_output, gh_output_as, git, open_pull, repo_with_branches, tls_server,
};

#[test]
fn pr_checkout_switches_to_the_head_and_status_lists_it() {
    let server = tls_server();
    let config = TempDir::new();
    let work = repo_with_branches(&server, &config.0, &["feature"]);
    open_pull(&server, &config.0, "feature", false);

    gh_in(&server, &config.0, &work, &["pr", "checkout", "1"]);
    let branch = git(
        &server,
        &config.0,
        &work,
        &["rev-parse", "--abbrev-ref", "HEAD"],
    );
    assert_eq!(branch.trim(), "feature");
    let status = gh_in(&server, &config.0, &work, &["pr", "status"]);
    assert!(status.contains("Add feature"), "{status}");
    assert!(status.contains('1'), "{status}");
}

#[test]
fn pr_lock_and_unlock() {
    let server = tls_server();
    let config = TempDir::new();
    repo_with_branches(&server, &config.0, &["feature"]);
    open_pull(&server, &config.0, "feature", false);

    gh(
        &server,
        &config.0,
        &[
            "pr",
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
            "repos/admin/tool/pulls/1",
            "[.locked, .active_lock_reason] | join(\",\")"
        ),
        "true,resolved"
    );
    gh(
        &server,
        &config.0,
        &["pr", "unlock", "1", "-R", "admin/tool"],
    );
    assert_eq!(
        api(&server, &config.0, "repos/admin/tool/pulls/1", ".locked"),
        "false"
    );
}

#[test]
fn pr_revert_opens_a_pull_request() {
    let server = tls_server();
    let config = TempDir::new();
    repo_with_branches(&server, &config.0, &["feature"]);
    open_pull(&server, &config.0, "feature", false);
    gh(
        &server,
        &config.0,
        &["pr", "merge", "1", "-R", "admin/tool", "--merge"],
    );

    let url = gh(
        &server,
        &config.0,
        &["pr", "revert", "1", "-R", "admin/tool"],
    );
    assert!(url.contains("/admin/tool/pull/2"), "{url}");
    assert_eq!(
        api(
            &server,
            &config.0,
            "repos/admin/tool/pulls/2",
            "[.state, .title] | join(\",\")"
        ),
        "open,Revert \"Add feature\""
    );
}

#[test]
fn pr_merge_auto_merges_a_clean_request_and_delete_branch_removes_the_ref() {
    let server = tls_server();
    let config = TempDir::new();
    repo_with_branches(&server, &config.0, &["feature"]);
    open_pull(&server, &config.0, "feature", false);

    gh(
        &server,
        &config.0,
        &[
            "pr",
            "merge",
            "1",
            "-R",
            "admin/tool",
            "--squash",
            "--auto",
            "--delete-branch",
        ],
    );
    assert_eq!(
        api(&server, &config.0, "repos/admin/tool/pulls/1", ".merged"),
        "true"
    );
    let gone = gh_output(
        &server,
        &config.0,
        &["api", "repos/admin/tool/git/ref/heads/feature"],
    );
    assert!(!gone.status.success());
    assert!(String::from_utf8_lossy(&gone.stderr).contains("404"));
}

#[test]
fn pr_close_comment_and_delete_branch() {
    let server = tls_server();
    let config = TempDir::new();
    repo_with_branches(&server, &config.0, &["feature"]);
    open_pull(&server, &config.0, "feature", false);

    gh(
        &server,
        &config.0,
        &[
            "pr",
            "close",
            "1",
            "-R",
            "admin/tool",
            "--comment",
            "Not now.",
            "--delete-branch",
        ],
    );
    assert_eq!(
        api(&server, &config.0, "repos/admin/tool/pulls/1", ".state"),
        "closed"
    );
    assert_eq!(
        api(
            &server,
            &config.0,
            "repos/admin/tool/issues/1/comments",
            ".[].body"
        ),
        "Not now."
    );
    let gone = gh_output(
        &server,
        &config.0,
        &["api", "repos/admin/tool/git/ref/heads/feature"],
    );
    assert!(!gone.status.success());
}

#[test]
fn pr_review_body_is_stored() {
    let server = tls_server();
    let config = TempDir::new();
    repo_with_branches(&server, &config.0, &["feature"]);
    open_pull(&server, &config.0, "feature", false);
    let bob = server.add_user("bob").add_token(&["repo"]);

    gh_as(
        &server,
        &config.0,
        &bob,
        &[
            "pr",
            "review",
            "1",
            "-R",
            "admin/tool",
            "--approve",
            "--body",
            "Good to land.",
        ],
    );
    assert_eq!(
        api(
            &server,
            &config.0,
            "repos/admin/tool/pulls/1/reviews",
            ".[0].body"
        ),
        "Good to land."
    );
}

#[test]
fn pr_merge_without_push_is_forbidden() {
    let server = tls_server();
    let config = TempDir::new();
    repo_with_branches(&server, &config.0, &["feature"]);
    open_pull(&server, &config.0, "feature", false);
    let bob = server.add_user("bob").add_token(&["repo"]);

    let output = gh_output_as(
        &server,
        &config.0,
        &bob,
        &["pr", "merge", "1", "-R", "admin/tool", "--merge"],
    );
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not accessible") || stderr.contains("Forbidden") || stderr.contains("403"),
        "{stderr}"
    );
    assert_eq!(
        api(&server, &config.0, "repos/admin/tool/pulls/1", ".merged"),
        "false"
    );
}
