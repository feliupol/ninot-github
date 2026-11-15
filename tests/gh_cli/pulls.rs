//! `gh pr` against the fake.

use ninot_github::fixtures::{CommitStatusSeed, SeedField};

use crate::common::TempDir;
use crate::{api, gh, gh_as, gh_json, open_pull, repo_with_branches, tls_server};

#[test]
#[ignore = "not implemented: GraphQL Repository.pullRequests, which `gh pr create` reads to find an open pull request for the head"]
fn pr_create_opens_ready_and_draft_pull_requests() {
    let server = tls_server();
    let config = TempDir::new();
    repo_with_branches(&server, &config.0, &["feature", "draft"]);

    let url = gh(
        &server,
        &config.0,
        &[
            "pr",
            "create",
            "--repo",
            "admin/tool",
            "--head",
            "feature",
            "--base",
            "main",
            "--title",
            "Add feature",
            "--body",
            "Adds a file.",
        ],
    );
    assert!(url.trim().ends_with("/admin/tool/pull/1"), "{url}");
    let url = gh(
        &server,
        &config.0,
        &[
            "pr",
            "create",
            "-R",
            "admin/tool",
            "-H",
            "draft",
            "-B",
            "main",
            "-t",
            "Draft",
            "-b",
            "",
            "--draft",
        ],
    );
    assert!(url.trim().ends_with("/admin/tool/pull/2"), "{url}");
    assert_eq!(
        api(&server, &config.0, "repos/admin/tool/pulls/2", ".draft"),
        "true"
    );
}

#[test]
fn pr_view_reads_the_number_and_node_id() {
    let server = tls_server();
    let config = TempDir::new();
    repo_with_branches(&server, &config.0, &["feature"]);
    let number = open_pull(&server, &config.0, "feature", false);

    let viewed = gh_json(
        &server,
        &config.0,
        &[
            "pr",
            "view",
            &number,
            "-R",
            "admin/tool",
            "--json",
            "id,number",
        ],
    );
    assert_eq!(viewed["number"], 1);
    assert!(viewed["id"].as_str().is_some_and(|id| !id.is_empty()));
}

/// GitHub's REST `node_id` is the GraphQL `id` of the same object:
/// <https://docs.github.com/en/graphql/guides/using-global-node-ids>.
#[test]
#[ignore = "fake bug: REST node_id `PR_admin_tool_1` differs from the GraphQL id of the same pull request"]
fn pr_node_id_matches_the_rest_node_id() {
    let server = tls_server();
    let config = TempDir::new();
    repo_with_branches(&server, &config.0, &["feature"]);
    let number = open_pull(&server, &config.0, "feature", false);

    let graphql = gh(
        &server,
        &config.0,
        &[
            "pr",
            "view",
            &number,
            "-R",
            "admin/tool",
            "--json",
            "id",
            "--jq",
            ".id",
        ],
    );
    let rest = api(&server, &config.0, "repos/admin/tool/pulls/1", ".node_id");
    assert_eq!(graphql.trim(), rest);
}

#[test]
#[ignore = "not implemented: GraphQL PullRequest.title, state, body, author, headRefName and the other fields `gh pr view` selects"]
fn pr_view_shows_the_pull_request() {
    let server = tls_server();
    let config = TempDir::new();
    repo_with_branches(&server, &config.0, &["feature"]);
    open_pull(&server, &config.0, "feature", false);

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
            "title,body,state,isDraft,headRefName,baseRefName,author",
        ],
    );
    assert_eq!(viewed["title"], "Add feature");
    assert_eq!(viewed["body"], "Adds a file.");
    assert_eq!(viewed["state"], "OPEN");
    assert_eq!(viewed["isDraft"], false);
    assert_eq!(viewed["headRefName"], "feature");
    assert_eq!(viewed["baseRefName"], "main");
    assert_eq!(viewed["author"]["login"], "admin");
    let text = gh(&server, &config.0, &["pr", "view", "1", "-R", "admin/tool"]);
    assert!(text.contains("Add feature"), "{text}");
}

#[test]
#[ignore = "not implemented: GraphQL Repository.pullRequests"]
fn pr_list_filters_by_state() {
    let server = tls_server();
    let config = TempDir::new();
    repo_with_branches(&server, &config.0, &["one", "two"]);
    open_pull(&server, &config.0, "one", false);
    open_pull(&server, &config.0, "two", false);
    gh(
        &server,
        &config.0,
        &[
            "api",
            "-X",
            "PATCH",
            "repos/admin/tool/pulls/2",
            "-f",
            "state=closed",
            "--silent",
        ],
    );

    let open = gh_json(
        &server,
        &config.0,
        &["pr", "list", "-R", "admin/tool", "--json", "number,title"],
    );
    assert_eq!(
        open,
        serde_json::json!([{ "number": 1, "title": "Add one" }])
    );
    let all = gh_json(
        &server,
        &config.0,
        &[
            "pr",
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

/// `gh pr diff` asks for `application/vnd.github.v3.diff` or `.patch`
/// (cli/cli v2.101.0 pkg/cmd/pr/diff/diff.go:217), which GitHub answers with the diff text:
/// <https://docs.github.com/en/rest/pulls/pulls#get-a-pull-request>.
#[test]
#[ignore = "fake bug: GET /repos/{owner}/{repo}/pulls/{number} ignores the diff and patch media types and answers JSON"]
fn pr_diff_prints_the_diff_and_patch() {
    let server = tls_server();
    let config = TempDir::new();
    repo_with_branches(&server, &config.0, &["feature"]);
    open_pull(&server, &config.0, "feature", false);

    let diff = gh(
        &server,
        &config.0,
        &["pr", "diff", "1", "-R", "admin/tool", "--color", "never"],
    );
    assert!(
        diff.starts_with("diff --git a/feature.txt b/feature.txt"),
        "{diff}"
    );
    assert!(diff.contains("+feature"), "{diff}");
    let patch = gh(
        &server,
        &config.0,
        &["pr", "diff", "1", "-R", "admin/tool", "--patch"],
    );
    assert!(patch.contains("Subject: [PATCH] Add feature"), "{patch}");
    let names = gh(
        &server,
        &config.0,
        &["pr", "diff", "1", "-R", "admin/tool", "--name-only"],
    );
    assert_eq!(names.trim(), "feature.txt");
}

#[test]
#[ignore = "not implemented: GraphQL PullRequest.headRefName and statusCheckRollup, which `gh pr checks` reads"]
fn pr_checks_lists_statuses_on_the_head() {
    let server = tls_server();
    let config = TempDir::new();
    repo_with_branches(&server, &config.0, &["feature"]);
    open_pull(&server, &config.0, "feature", false);
    let head = api(&server, &config.0, "repos/admin/tool/pulls/1", ".head.sha");
    server
        .repository("admin", "tool")
        .unwrap()
        .commit(&head)
        .unwrap()
        .create_status(CommitStatusSeed {
            state: SeedField::present("success".to_owned()),
            context: SeedField::present("tests".to_owned()),
            ..Default::default()
        })
        .unwrap();

    let checks = gh_json(
        &server,
        &config.0,
        &[
            "pr",
            "checks",
            "1",
            "-R",
            "admin/tool",
            "--json",
            "name,state,bucket",
        ],
    );
    assert_eq!(
        checks,
        serde_json::json!([{ "name": "tests", "state": "SUCCESS", "bucket": "pass" }])
    );
}

#[test]
#[ignore = "not implemented: GraphQL Mutation.addPullRequestReview"]
fn pr_review_approves_requests_changes_and_comments() {
    let server = tls_server();
    let config = TempDir::new();
    repo_with_branches(&server, &config.0, &["feature"]);
    open_pull(&server, &config.0, "feature", false);
    // GitHub rejects reviews that approve or request changes on your own pull request.
    let bob = server.add_user("bob").add_token(&["repo"]);

    for args in [
        ["--comment", "-b", "A note."],
        ["--request-changes", "-b", "Fix it."],
        ["--approve", "-b", "Good."],
    ] {
        let mut command = vec!["pr", "review", "1", "-R", "admin/tool"];
        command.extend(args);
        gh_as(&server, &config.0, &bob, &command);
    }
    let states = api(
        &server,
        &config.0,
        "repos/admin/tool/pulls/1/reviews",
        "[.[].state] | join(\",\")",
    );
    assert_eq!(states, "COMMENTED,CHANGES_REQUESTED,APPROVED");
}

#[test]
#[ignore = "not implemented: GraphQL PullRequest.url, read by `gh pr comment`, and IssueComment.url, read from Mutation.addComment"]
fn pr_comment_adds_an_issue_comment() {
    let server = tls_server();
    let config = TempDir::new();
    repo_with_branches(&server, &config.0, &["feature"]);
    open_pull(&server, &config.0, "feature", false);

    let url = gh(
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
    assert!(url.contains("/admin/tool/pull/1#issuecomment-"), "{url}");
    assert_eq!(
        api(
            &server,
            &config.0,
            "repos/admin/tool/issues/1/comments",
            ".[].body"
        ),
        "Looks fine."
    );
}

#[test]
#[ignore = "not implemented: GraphQL PullRequest.title, body, baseRefName and the other fields `gh pr edit` reads first"]
fn pr_edit_changes_the_title_body_and_base() {
    let server = tls_server();
    let config = TempDir::new();
    repo_with_branches(&server, &config.0, &["feature", "base"]);
    open_pull(&server, &config.0, "feature", false);

    gh(
        &server,
        &config.0,
        &[
            "pr",
            "edit",
            "1",
            "-R",
            "admin/tool",
            "--title",
            "Renamed",
            "--body",
            "New body.",
            "--base",
            "base",
        ],
    );
    assert_eq!(
        api(
            &server,
            &config.0,
            "repos/admin/tool/pulls/1",
            "[.title, .body, .base.ref] | join(\",\")"
        ),
        "Renamed,New body.,base"
    );
}

#[test]
#[ignore = "not implemented: GraphQL PullRequest.state and isDraft, which `gh pr ready` reads first"]
fn pr_ready_toggles_draft() {
    let server = tls_server();
    let config = TempDir::new();
    repo_with_branches(&server, &config.0, &["feature"]);
    open_pull(&server, &config.0, "feature", true);

    gh(
        &server,
        &config.0,
        &["pr", "ready", "1", "-R", "admin/tool"],
    );
    assert_eq!(
        api(&server, &config.0, "repos/admin/tool/pulls/1", ".draft"),
        "false"
    );
    gh(
        &server,
        &config.0,
        &["pr", "ready", "1", "-R", "admin/tool", "--undo"],
    );
    assert_eq!(
        api(&server, &config.0, "repos/admin/tool/pulls/1", ".draft"),
        "true"
    );
}

#[test]
#[ignore = "not implemented: GraphQL PullRequest.state, title, isCrossRepository and headRefName, which `gh pr close` reads first"]
fn pr_close_and_reopen() {
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
        ],
    );
    assert_eq!(
        api(&server, &config.0, "repos/admin/tool/pulls/1", ".state"),
        "closed"
    );
    gh(
        &server,
        &config.0,
        &["pr", "reopen", "1", "-R", "admin/tool"],
    );
    assert_eq!(
        api(&server, &config.0, "repos/admin/tool/pulls/1", ".state"),
        "open"
    );
}

#[test]
#[ignore = "not implemented: GraphQL PullRequest.state, mergeStateStatus, headRefOid and the other fields `gh pr merge` reads first"]
fn pr_merge_with_each_method() {
    let server = tls_server();
    let config = TempDir::new();
    repo_with_branches(&server, &config.0, &["one", "two", "three"]);
    for (branch, method) in [
        ("one", "--merge"),
        ("two", "--squash"),
        ("three", "--rebase"),
    ] {
        let number = open_pull(&server, &config.0, branch, false);
        gh(
            &server,
            &config.0,
            &[
                "pr",
                "merge",
                &number,
                "-R",
                "admin/tool",
                method,
                "--delete-branch",
            ],
        );
        assert_eq!(
            api(
                &server,
                &config.0,
                &format!("repos/admin/tool/pulls/{number}"),
                ".merged"
            ),
            "true"
        );
    }
    let files = api(
        &server,
        &config.0,
        "repos/admin/tool/contents/?ref=main",
        "[.[].name] | sort | join(\",\")",
    );
    assert_eq!(files, "README.md,one.txt,three.txt,two.txt");
}

#[test]
#[ignore = "not implemented: GraphQL PullRequest.headRefName, headRefOid, headRepositoryOwner and mergeable, which `gh pr update-branch` reads first"]
fn pr_update_branch_merges_the_base_into_the_head() {
    let server = tls_server();
    let config = TempDir::new();
    repo_with_branches(&server, &config.0, &["feature"]);
    open_pull(&server, &config.0, "feature", false);
    gh(
        &server,
        &config.0,
        &[
            "api",
            "-X",
            "PUT",
            "repos/admin/tool/contents/later.txt",
            "-f",
            "message=Later",
            "-f",
            "content=bGF0ZXIK",
            "-f",
            "branch=main",
            "--silent",
        ],
    );
    assert_eq!(
        api(
            &server,
            &config.0,
            "repos/admin/tool/compare/feature...main",
            ".ahead_by"
        ),
        "1"
    );

    gh(
        &server,
        &config.0,
        &["pr", "update-branch", "1", "-R", "admin/tool"],
    );
    assert_eq!(
        api(
            &server,
            &config.0,
            "repos/admin/tool/compare/feature...main",
            ".ahead_by"
        ),
        "0"
    );
}
