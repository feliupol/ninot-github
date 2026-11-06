//! Pull request review and comment read models.

mod common;

use ninot_github::Server;
use ninot_github::fixtures::{
    CheckRunOutputSeed, CheckRunSeed, CommitStatusSeed, IssueCommentSeed, IssueSeed, JobSeed,
    LabelSeed, PullRequestBranchSeed, PullRequestSeed, ReviewCommentSeed, ReviewSeed, SeedField,
    TeamSeed, UserSeed, WorkflowRunSeed,
};
use serde_json::json;

#[test]
fn pull_requests_reviews_and_comments_are_seeded_in_server_state() {
    let server = Server::start().unwrap();
    let repository = server
        .create_repository("admin", "project", false, None, None, "main")
        .unwrap();
    let pull = repository
        .create_pull_request(
            9,
            PullRequestSeed {
                title: SeedField::present("Adopt Ninot".to_owned()),
                draft: SeedField::present(true),
                state: SeedField::present("open".to_owned()),
                mergeable: SeedField::present(Some(false)),
                base: SeedField::present(PullRequestBranchSeed {
                    branch_ref: SeedField::present("main".to_owned()),
                    ..Default::default()
                }),
                head: SeedField::present(PullRequestBranchSeed {
                    sha: SeedField::present("0123456789012345678901234567890123456789".to_owned()),
                    ..Default::default()
                }),
                labels: SeedField::present(vec![LabelSeed {
                    id: SeedField::present(1),
                    name: SeedField::present("automation".to_owned()),
                    ..Default::default()
                }]),
                ..Default::default()
            },
        )
        .expect("pull-request fixture matches the API model");
    pull.create_review(ReviewSeed {
        state: SeedField::present("CHANGES_REQUESTED".to_owned()),
        user: SeedField::present(Some(UserSeed {
            login: SeedField::present("reviewer".to_owned()),
            account_type: SeedField::present("Bot".to_owned()),
            ..Default::default()
        })),
        submitted_at: SeedField::present(Some("2026-01-02T03:04:05Z".to_owned())),
        ..Default::default()
    })
    .expect("review fixture matches the API model");
    pull.create_issue_comment(IssueCommentSeed {
        body: SeedField::present("Please update this.".to_owned()),
        user: SeedField::present(Some(UserSeed {
            login: SeedField::present("reviewer".to_owned()),
            account_type: SeedField::present("Bot".to_owned()),
            ..Default::default()
        })),
        created_at: SeedField::present("2026-01-02T03:04:05Z".to_owned()),
        ..Default::default()
    })
    .expect("issue-comment fixture matches the API model");
    pull.create_review_comment(ReviewCommentSeed {
        path: SeedField::present("src/main.rs".to_owned()),
        line: SeedField::present(Some(7)),
        body: SeedField::present("This line needs a guard.".to_owned()),
        user: SeedField::present(Some(UserSeed {
            login: SeedField::present("reviewer".to_owned()),
            account_type: SeedField::present("Bot".to_owned()),
            ..Default::default()
        })),
        created_at: SeedField::present("2026-01-02T03:04:05Z".to_owned()),
        ..Default::default()
    })
    .expect("review-comment fixture matches the API model");

    let api = common::Api::admin(&server);
    let pull = api.get("/repos/admin/project/pulls/9");
    assert_eq!(pull.status, 200, "{:?}", pull.body);
    assert_eq!(pull.body["draft"], true);
    assert_eq!(
        pull.body["head"]["sha"],
        "0123456789012345678901234567890123456789"
    );
    assert_eq!(pull.body["base"]["ref"], "main");
    assert_eq!(pull.body["labels"][0]["name"], "automation");

    let reviews = api.get("/repos/admin/project/pulls/9/reviews");
    assert_eq!(reviews.status, 200, "{:?}", reviews.body);
    assert_eq!(reviews.body[0]["state"], "CHANGES_REQUESTED");
    assert_eq!(reviews.body[0]["user"]["type"], "Bot");

    let issue_comments = api.get("/repos/admin/project/issues/9/comments");
    assert_eq!(issue_comments.status, 200, "{:?}", issue_comments.body);
    assert_eq!(issue_comments.body[0]["body"], "Please update this.");
    assert_eq!(issue_comments.body[0]["user"]["type"], "Bot");

    let review_comments = api.get("/repos/admin/project/pulls/9/comments");
    assert_eq!(review_comments.status, 200, "{:?}", review_comments.body);
    assert_eq!(review_comments.body[0]["path"], "src/main.rs");
    assert_eq!(review_comments.body[0]["line"], 7);

    let list = api.get("/repos/admin/project/pulls?state=open");
    assert_eq!(list.status, 200, "{:?}", list.body);
    assert_eq!(list.body[0]["number"], 9);
}

#[test]
fn sync_routes_read_git_state_and_mutate_the_same_pull_request_world() {
    use std::path::Path;

    let server = Server::start().unwrap();
    server.add_user("alice");
    server.set_user_name("alice", Some("Alice Example".to_owned()));
    let repository = server
        .create_repository(
            "alice",
            "project",
            false,
            Some("Sync fixture".to_owned()),
            Some("Rust".to_owned()),
            "main",
        )
        .unwrap();
    repository
        .commit_file(
            "main",
            Path::new(".github/github-automations.yml"),
            b"enabled: true\n",
            "Add config",
        )
        .unwrap();
    repository
        .commit_file(
            "feature",
            Path::new("change.txt"),
            b"change\n",
            "Add change",
        )
        .unwrap();
    let feature_sha = repository
        .commit_file(
            "feature",
            Path::new("follow-up.txt"),
            b"follow-up\n",
            "Add follow-up",
        )
        .unwrap()
        .sha()
        .to_owned();

    let api = common::Api::admin(&server);
    let user = api.get("/users/alice");
    assert_eq!(user.body["name"], "Alice Example");
    let repository = api.get("/repos/alice/project");
    assert_eq!(repository.body["description"], "Sync fixture");
    assert_eq!(repository.body["language"], "Rust");
    assert_eq!(repository.body["default_branch"], "main");
    let contents = api.get("/repos/alice/project/contents/.github/github-automations.yml?ref=main");
    assert_eq!(contents.status, 200, "{:?}", contents.body);

    let compare = api.get("/repos/alice/project/compare/main...feature?per_page=1");
    assert_eq!(compare.status, 200, "{:?}", compare.body);
    assert_eq!(compare.body["status"], "ahead");
    assert_eq!(compare.body["ahead_by"], 2);

    let created = api.post(
        "/repos/alice/project/pulls",
        json!({"title": "Sync", "body": "Automated sync", "head": "alice:feature", "base": "main", "maintainer_can_modify": false}),
    );
    assert_eq!(created.status, 201, "{:?}", created.body);
    let number = created.body["number"].as_u64().unwrap();
    assert_eq!(created.body["head"]["ref"], "feature");
    assert_eq!(created.body["head"]["sha"], feature_sha);
    assert_eq!(created.body["base"]["ref"], "main");
    assert_eq!(created.body["maintainer_can_modify"], false);
    let listed = api
        .get("/repos/alice/project/pulls?state=open&base=main&head=alice%3Afeature&per_page=100");
    assert_eq!(listed.status, 200, "{:?}", listed.body);
    assert_eq!(listed.body[0]["number"], number);
    let labeled = api.post(
        &format!("/repos/alice/project/issues/{number}/labels"),
        json!({"labels": ["automation"]}),
    );
    assert_eq!(labeled.status, 200, "{:?}", labeled.body);
    let comment = api.post(
        &format!("/repos/alice/project/issues/{number}/comments"),
        json!({"body": "Conflict found"}),
    );
    assert_eq!(comment.status, 201, "{:?}", comment.body);
    let reread = api.get(&format!("/repos/alice/project/pulls/{number}"));
    assert_eq!(reread.body["labels"][0]["name"], "automation");
    assert_eq!(reread.body["comments"], 1);
    let comments = api.get(&format!("/repos/alice/project/issues/{number}/comments"));
    assert_eq!(comments.body[0]["body"], "Conflict found");
}

#[test]
fn workflow_runs_jobs_and_commit_checks_share_repository_state() {
    let server = Server::start().unwrap();
    let repo_handle = server
        .create_repository("admin", "ci", false, None, None, "main")
        .unwrap();
    let api = common::Api::admin(&server);
    let run = repo_handle
        .create_workflow_run(
            WorkflowRunSeed {
                id: SeedField::present(42),
                head_branch: SeedField::present(Some("main".to_owned())),
                event: SeedField::present("push".to_owned()),
                status: SeedField::present(Some("completed".to_owned())),
                conclusion: SeedField::present(Some("success".to_owned())),
                head_sha: SeedField::present("abc".to_owned()),
                name: SeedField::present(Some("CI".to_owned())),
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
        .expect("workflow-run and job fixtures match the API models");
    let commit = repo_handle.commit("abc").expect("server is still running");
    commit
        .create_check_run(CheckRunSeed {
            id: SeedField::present(100),
            name: SeedField::present("tests".to_owned()),
            status: SeedField::present("completed".to_owned()),
            conclusion: SeedField::present(Some("success".to_owned())),
            output: SeedField::present(CheckRunOutputSeed {
                title: SeedField::present(Some("tests".to_owned())),
                summary: SeedField::present(Some(String::new())),
                text: SeedField::present(Some(String::new())),
                ..Default::default()
            }),
            ..Default::default()
        })
        .expect("check-run fixture matches the API model");
    commit
        .create_status(CommitStatusSeed {
            id: SeedField::present(1),
            state: SeedField::present("success".to_owned()),
            context: SeedField::present("ci/build".to_owned()),
            creator: SeedField::present(Some(UserSeed {
                login: SeedField::present("admin".to_owned()),
                ..Default::default()
            })),
            ..Default::default()
        })
        .expect("commit-status fixture matches the API model");

    let runs =
        api.get("/repos/admin/ci/actions/runs?branch=main&event=push&status=completed&per_page=30");
    assert_eq!(runs.status, 200, "{:?}", runs.body);
    assert_eq!(runs.body["total_count"], 1);
    assert_eq!(runs.body["workflow_runs"][0]["id"], 42);
    let jobs = api.get("/repos/admin/ci/actions/runs/42/jobs?per_page=100");
    assert_eq!(jobs.status, 200, "{:?}", jobs.body);
    assert_eq!(jobs.body["jobs"][0]["name"], "test");
    let checks = api.get("/repos/admin/ci/commits/abc/check-runs");
    assert_eq!(checks.body["check_runs"][0]["name"], "tests");
    let statuses = api.get("/repos/admin/ci/commits/abc/statuses");
    assert_eq!(statuses.body[0]["context"], "ci/build");
    assert_eq!(run.id(), 42);
}

#[test]
fn issue_creation_persists_labels_and_comments_for_later_reads() {
    let server = Server::start().unwrap();
    server
        .create_repository("admin", "suggestions", false, None, None, "main")
        .unwrap();
    let api = common::Api::admin(&server);
    server.start_profile();
    let created = api.post(
        "/repos/admin/suggestions/issues",
        json!({
            "title": "Feature Request: jenkins - jenkins_cancel_build",
            "body": "Add the tool.",
            "labels": ["enhancement", "area:mcp", "needs-review"]
        }),
    );
    assert_eq!(created.status, 201, "{:?}", created.body);
    assert_eq!(created.body["number"], 1);
    assert_eq!(created.body["labels"][0]["name"], "enhancement");
    let reread = api.get("/repos/admin/suggestions/issues/1");
    assert_eq!(reread.body["title"], created.body["title"]);
    let listed = api.get("/repos/admin/suggestions/issues?state=open");
    assert_eq!(listed.body[0]["number"], 1);
    assert_eq!(server.stop_profile().hits().len(), 3);
}

#[test]
fn fork_comparison_and_pull_creation_resolve_owner_branch_heads() {
    let server = Server::start().unwrap();
    server.add_user("upstream");
    server.add_user("alice");
    let alice_token = server.add_token("alice", &["repo", "public_repo"]);
    let upstream = server
        .create_repository("upstream", "project", false, None, None, "main")
        .unwrap();
    let base = upstream
        .commit_file("main", std::path::Path::new("README.md"), b"base\n", "base")
        .unwrap()
        .sha()
        .to_owned();
    let head = upstream
        .commit_file(
            "topic",
            std::path::Path::new("change.txt"),
            b"change\n",
            "change",
        )
        .unwrap()
        .sha()
        .to_owned();
    let fork = server
        .create_repository("alice", "project-fork", false, None, None, "main")
        .unwrap();
    fork.fork_from(&upstream).unwrap();
    let api = common::Api::new(&server, Some(&alice_token));

    let compare = api.get("/repos/alice/project-fork/compare/main...upstream:topic?per_page=1");
    assert_eq!(compare.status, 200, "{:?}", compare.body);
    assert_eq!(compare.body["status"], "ahead");
    assert_eq!(compare.body["ahead_by"], 1);
    assert_eq!(compare.body["commits"][0]["sha"], head);

    let created = api.post(
        "/repos/alice/project-fork/pulls",
        json!({"title": "Upstream topic", "head": "upstream:topic", "base": "main"}),
    );
    assert_eq!(created.status, 201, "{:?}", created.body);
    assert_eq!(created.body["head"]["sha"], head);
    assert_eq!(created.body["head"]["user"]["login"], "upstream");
    assert_eq!(created.body["base"]["sha"], base);
    let listed =
        api.get("/repos/alice/project-fork/pulls?state=open&base=main&head=upstream%3Atopic");
    assert_eq!(listed.status, 200, "{:?}", listed.body);
    assert_eq!(listed.body[0]["number"], 1);
}

#[test]
fn fixture_commits_are_reproducible_across_server_instances() {
    use std::path::Path;

    let commit = || {
        let server = Server::start().unwrap();
        server.add_user("alice");
        let repository = server
            .create_repository("alice", "project", false, None, None, "main")
            .unwrap();
        repository
            .commit_file(
                "main",
                Path::new("README.md"),
                b"stable fixture\n",
                "Seed project",
            )
            .unwrap()
            .sha()
            .to_owned()
    };

    assert_eq!(commit(), commit());
}

#[test]
fn gist_creation_is_authenticated_and_persists_for_list_reads() {
    let server = Server::start().unwrap();
    server.add_user("alice");
    let token = server.add_token("alice", &["gist"]);
    let api = common::Api::new(&server, Some(&token));

    let created = api.post(
        "/gists",
        json!({
            "description": "test gist",
            "files": {"test.txt": {"content": "hello"}}
        }),
    );
    assert_eq!(created.status, 201, "{:?}", created.body);
    assert_eq!(created.body["description"], "test gist");
    assert_eq!(created.body["files"]["test.txt"]["content"], "hello");
    let listed = api.get("/gists");
    assert_eq!(listed.status, 200, "{:?}", listed.body);
    assert_eq!(listed.body[0]["id"], created.body["id"]);
}

#[test]
fn combined_commit_status_uses_latest_status_per_context() {
    let server = Server::start().unwrap();
    server.add_user("alice");
    let repository = server
        .create_repository("alice", "status", false, None, None, "main")
        .unwrap();
    let api = common::Api::admin(&server);
    let sha = "0123456789012345678901234567890123456789";
    let commit = repository.commit(sha).expect("server is still running");
    commit
        .create_status(CommitStatusSeed {
            id: SeedField::present(1),
            state: SeedField::present("failure".to_owned()),
            context: SeedField::present("ci/build".to_owned()),
            creator: SeedField::present(Some(UserSeed {
                login: SeedField::present("alice".to_owned()),
                ..Default::default()
            })),
            ..Default::default()
        })
        .expect("commit-status fixture matches the API model");
    commit
        .create_status(CommitStatusSeed {
            id: SeedField::present(2),
            state: SeedField::present("success".to_owned()),
            context: SeedField::present("ci/build".to_owned()),
            creator: SeedField::present(Some(UserSeed {
                login: SeedField::present("alice".to_owned()),
                ..Default::default()
            })),
            ..Default::default()
        })
        .expect("commit-status fixture matches the API model");
    commit
        .create_status(CommitStatusSeed {
            id: SeedField::present(3),
            state: SeedField::present("pending".to_owned()),
            context: SeedField::present("ci/lint".to_owned()),
            creator: SeedField::present(Some(UserSeed {
                login: SeedField::present("alice".to_owned()),
                ..Default::default()
            })),
            ..Default::default()
        })
        .expect("commit-status fixture matches the API model");

    let combined = api.get(&format!("/repos/alice/status/commits/{sha}/status"));
    assert_eq!(combined.status, 200, "{:?}", combined.body);
    assert_eq!(combined.body["state"], "pending");
    assert_eq!(combined.body["total_count"], 2);
    assert_eq!(combined.body["statuses"].as_array().unwrap().len(), 2);
    assert_eq!(combined.body["sha"], sha);
}

#[test]
fn pull_request_updates_edit_metadata_and_close_then_reopen() {
    use std::path::Path;

    let server = Server::start().unwrap();
    let repository = server
        .create_repository("admin", "project", false, None, None, "main")
        .unwrap();
    for branch in ["main", "release", "feature"] {
        repository
            .commit_file(
                branch,
                Path::new(&format!("{branch}.txt")),
                b"content\n",
                "Add file",
            )
            .unwrap();
    }
    let api = common::Api::admin(&server);
    let created = api.post(
        "/repos/admin/project/pulls",
        json!({"title": "Draft", "body": "Old", "head": "feature", "base": "main"}),
    );
    assert_eq!(created.status, 201, "{:?}", created.body);
    let path = format!(
        "/repos/admin/project/pulls/{}",
        created.body["number"].as_u64().unwrap()
    );
    let release = api.get("/repos/admin/project/branches/release");

    let updated = api.patch(
        &path,
        json!({"title": "Ready", "body": "New", "base": "release"}),
    );
    assert_eq!(updated.status, 200, "{:?}", updated.body);
    assert_eq!(updated.body["title"], "Ready");
    assert_eq!(updated.body["body"], "New");
    assert_eq!(updated.body["base"]["ref"], "release");
    assert_eq!(updated.body["base"]["sha"], release.body["commit"]["sha"]);
    assert_eq!(updated.body["state"], "open");

    let missing_base = api.patch(&path, json!({"base": "missing"}));
    assert_eq!(missing_base.status, 422, "{:?}", missing_base.body);
    assert_eq!(api.get(&path).body["base"]["ref"], "release");

    let closed = api.patch(&path, json!({"state": "closed"}));
    assert_eq!(closed.status, 200, "{:?}", closed.body);
    let reread = api.get(&path);
    assert_eq!(reread.body["state"], "closed");
    assert!(reread.body["closed_at"].is_string(), "{:?}", reread.body);
    assert_eq!(reread.body["merged"], false);
    assert_eq!(reread.body["title"], "Ready");
    let open = api.get("/repos/admin/project/pulls?state=open");
    assert_eq!(open.body, json!([]));

    let reopened = api.patch(&path, json!({"state": "open"}));
    assert_eq!(reopened.status, 200, "{:?}", reopened.body);
    assert_eq!(reopened.body["state"], "open");
    assert!(reopened.body["closed_at"].is_null(), "{:?}", reopened.body);
}

#[test]
fn merged_pull_requests_cannot_be_reopened() {
    use std::path::Path;

    let server = Server::start().unwrap();
    let repository = server
        .create_repository("admin", "project", false, None, None, "main")
        .unwrap();
    for branch in ["main", "feature"] {
        repository
            .commit_file(
                branch,
                Path::new(&format!("{branch}.txt")),
                b"content\n",
                "Add file",
            )
            .unwrap();
    }
    let api = common::Api::admin(&server);
    let created = api.post(
        "/repos/admin/project/pulls",
        json!({"title": "Ship", "head": "feature", "base": "main"}),
    );
    let path = format!(
        "/repos/admin/project/pulls/{}",
        created.body["number"].as_u64().unwrap()
    );
    let merged = api.put(&format!("{path}/merge"), json!({}));
    assert_eq!(merged.status, 200, "{:?}", merged.body);

    let reopened = api.patch(&path, json!({"state": "open"}));
    assert_eq!(reopened.status, 422, "{:?}", reopened.body);
    assert_eq!(api.get(&path).body["state"], "closed");
}

#[test]
fn requesting_reviewers_answers_with_the_pull_request() {
    use std::path::Path;

    let server = Server::start().unwrap();
    server.add_user("alice");
    let repository = server
        .create_repository("admin", "project", false, None, None, "main")
        .unwrap();
    for branch in ["main", "feature"] {
        repository
            .commit_file(
                branch,
                Path::new(&format!("{branch}.txt")),
                b"content\n",
                "Add file",
            )
            .unwrap();
    }
    let api = common::Api::admin(&server);
    let created = api.post(
        "/repos/admin/project/pulls",
        json!({"title": "Review me", "head": "feature", "base": "main"}),
    );
    let path = format!(
        "/repos/admin/project/pulls/{}",
        created.body["number"].as_u64().unwrap()
    );

    let requested = api.post(
        &format!("{path}/requested_reviewers"),
        json!({"reviewers": ["alice"]}),
    );
    assert_eq!(requested.status, 201, "{:?}", requested.body);
    assert_eq!(requested.body["number"], created.body["number"]);
    assert_eq!(requested.body["requested_reviewers"][0]["login"], "alice");

    let listed = api.get(&format!("{path}/requested_reviewers"));
    assert_eq!(listed.status, 200, "{:?}", listed.body);
    assert_eq!(listed.body["users"][0]["login"], "alice");
}

fn searched_numbers(api: &common::Api, query: &str) -> Vec<u64> {
    let path = format!(
        "/search/issues?q={}",
        url::form_urlencoded::byte_serialize(query.as_bytes()).collect::<String>()
    );
    let reply = api.get(&path);
    assert_eq!(reply.status, 200, "{query}: {}", reply.body);
    let mut numbers: Vec<u64> = reply.body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["number"].as_u64().unwrap())
        .collect();
    numbers.sort_unstable();
    numbers
}

#[test]
fn review_requested_search_matches_requests_through_teams() {
    let server = Server::start().unwrap();
    server.add_org("org", &["me", "alice"]);
    server.add_team("org", "reviewers", &["me"]).unwrap();
    let repository = server
        .create_repository("org", "project", false, None, None, "main")
        .unwrap();
    let user = |login: &str| UserSeed {
        login: SeedField::present(login.to_owned()),
        ..Default::default()
    };
    let team = PullRequestSeed {
        requested_teams: SeedField::present(vec![TeamSeed {
            slug: "reviewers".to_owned(),
        }]),
        ..Default::default()
    };
    let direct = PullRequestSeed {
        requested_reviewers: SeedField::present(vec![user("me")]),
        ..Default::default()
    };
    let other = PullRequestSeed {
        requested_reviewers: SeedField::present(vec![user("alice")]),
        ..Default::default()
    };
    for (number, seed) in [(1, team), (2, direct), (3, other)] {
        repository.create_pull_request(number, seed).unwrap();
    }
    repository
        .create_issue(
            4,
            IssueSeed {
                title: SeedField::present("Not a pull request".to_owned()),
                ..Default::default()
            },
        )
        .unwrap();
    let me = common::Api::new(&server, Some(&server.add_token("me", &["repo"])));

    assert_eq!(
        searched_numbers(&me, "is:open is:pr review-requested:@me"),
        [1, 2]
    );
    assert_eq!(searched_numbers(&me, "review-requested:alice"), [3]);
    assert_eq!(searched_numbers(&me, "user-review-requested:@me"), [2]);
    assert_eq!(
        searched_numbers(&me, "team-review-requested:org/reviewers"),
        [1]
    );
    assert_eq!(
        searched_numbers(&me, "team-review-requested:other/reviewers"),
        Vec::<u64>::new()
    );
    let pull = me.get("/repos/org/project/pulls/1");
    assert_eq!(pull.body["requested_teams"][0]["slug"], "reviewers");

    let graphql = me.post(
        "/api/graphql",
        json!({ "query": "{search(query: \"is:pr review-requested:@me\", type: ISSUE, first: 10){nodes{... on PullRequest{number}}}}" }),
    );
    assert_eq!(graphql.status, 200, "{}", graphql.body);
    let mut numbers: Vec<u64> = graphql.body["data"]["search"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|node| node["number"].as_u64().unwrap())
        .collect();
    numbers.sort_unstable();
    assert_eq!(numbers, [1, 2]);
}

#[test]
fn requesting_team_reviewers_resolves_the_organization_team() {
    let server = Server::start().unwrap();
    server.add_org("org", &["admin"]);
    server.add_team("org", "reviewers", &["admin"]).unwrap();
    let repository = server
        .create_repository("org", "project", false, None, None, "main")
        .unwrap();
    repository
        .create_pull_request(4, PullRequestSeed::default())
        .unwrap();
    let api = common::Api::admin(&server);

    let requested = api.post(
        "/repos/org/project/pulls/4/requested_reviewers",
        json!({"team_reviewers": ["reviewers"]}),
    );
    assert_eq!(requested.status, 201, "{:?}", requested.body);
    let listed = api.get("/repos/org/project/pulls/4/requested_reviewers");
    assert_eq!(listed.body["teams"][0]["slug"], "reviewers");
    let missing = api.post(
        "/repos/org/project/pulls/4/requested_reviewers",
        json!({"team_reviewers": ["nobody"]}),
    );
    assert_eq!(missing.status, 422, "{:?}", missing.body);
}

#[test]
fn teams_belong_to_organizations() {
    let server = Server::start().unwrap();
    server.add_user("alice");
    assert!(server.add_team("alice", "reviewers", &[]).is_err());
}

#[test]
fn labels_added_to_issues_and_pull_requests_are_listed_on_them() {
    let server = Server::start().unwrap();
    let repository = server
        .create_repository("admin", "project", false, None, None, "main")
        .unwrap();
    repository
        .create_pull_request(2, PullRequestSeed::default())
        .unwrap();
    let api = common::Api::admin(&server);
    let issue = api.post("/repos/admin/project/issues", json!({ "title": "Bug" }));
    let issue = issue.body["number"].as_u64().unwrap();
    assert_eq!(
        api.get("/repos/admin/project/issues/2/labels").body,
        json!([])
    );
    for number in [issue, 2] {
        let path = format!("/repos/admin/project/issues/{number}/labels");
        assert_eq!(
            api.post(&path, json!({ "labels": ["automerge"] })).status,
            200
        );
        let listed = api.get(&path);
        assert_eq!(listed.status, 200, "{}", listed.body);
        assert_eq!(listed.body[0]["name"], "automerge");
    }
    assert_eq!(api.get("/repos/admin/project/issues/9/labels").status, 404);
}

#[test]
fn repeated_repo_qualifiers_search_any_of_the_repositories() {
    let server = Server::start().unwrap();
    for (number, name) in [(1, "api"), (2, "web"), (3, "docs")] {
        server
            .create_repository("admin", name, false, None, None, "main")
            .unwrap()
            .create_pull_request(number, PullRequestSeed::default())
            .unwrap();
    }
    let api = common::Api::admin(&server);

    assert_eq!(
        searched_numbers(&api, "is:pr repo:admin/api repo:admin/web"),
        [1, 2]
    );
    let graphql = api.post(
        "/api/graphql",
        json!({ "query": "{search(query: \"is:pr repo:admin/api repo:admin/web\", type: ISSUE, first: 10){nodes{... on PullRequest{number}}}}" }),
    );
    assert_eq!(graphql.status, 200, "{}", graphql.body);
    let mut numbers: Vec<u64> = graphql.body["data"]["search"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|node| node["number"].as_u64().unwrap())
        .collect();
    numbers.sort_unstable();
    assert_eq!(numbers, [1, 2]);
}
