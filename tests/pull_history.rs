//! Pull-request history: the issue timeline, pull-request commits and files, and the paging
//! of every list a history reader walks.

mod common;

use std::path::Path;

use common::{Api, TempDir, clone_url, git_ok};
use ninot_github::Server;
use ninot_github::fixtures::{
    GitIdentitySeed, IssueCommentSeed, PullRequestBranchSeed, PullRequestFileSeed, PullRequestSeed,
    ReviewSeed, SeedField, TimelineEventSeed, UserSeed, WorkflowRunSeed,
};
use ninot_github::server::Repository;
use serde_json::{Value, json};

fn identity(name: &str, date: &str) -> GitIdentitySeed {
    GitIdentitySeed {
        name: name.to_owned(),
        email: format!("{name}@example.com"),
        date: date.to_owned(),
    }
}

fn user(login: &str) -> UserSeed {
    UserSeed {
        login: SeedField::present(login.to_owned()),
        ..Default::default()
    }
}

fn event(name: &str, created_at: &str) -> TimelineEventSeed {
    TimelineEventSeed {
        event: name.to_owned(),
        created_at: created_at.to_owned(),
        ..Default::default()
    }
}

/// Commits `path` on `branch` as alice at `date` and returns the sha.
fn commit(repository: &Repository, branch: &str, path: &str, content: &str, date: &str) -> String {
    let alice = identity("alice", date);
    repository
        .commit_file_by(
            branch,
            Path::new(path),
            content.as_bytes(),
            &format!("Change {path}"),
            Some(&alice),
            None,
        )
        .unwrap()
        .sha()
        .to_owned()
}

/// The `event` field of every timeline entry.
fn kinds(timeline: &Value) -> Vec<String> {
    timeline
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["event"].as_str().unwrap().to_owned())
        .collect()
}

/// The URL in the `rel` entry of a `Link` header, relative to the server.
fn link(server: &Server, header: &str, rel: &str) -> Option<String> {
    header
        .split(", ")
        .find(|part| part.ends_with(&format!("rel=\"{rel}\"")))
        .map(|part| {
            let url = part.split(['<', '>']).nth(1).unwrap();
            url.strip_prefix(server.url()).unwrap().to_owned()
        })
}

#[test]
fn seeded_history_reads_back_through_the_timeline_in_order() {
    let server = Server::start().unwrap();
    let repository = server
        .create_repository("admin", "project", false, None, None, "main")
        .unwrap();
    let base = commit(
        &repository,
        "main",
        "base.txt",
        "base",
        "2026-01-01T00:00:00Z",
    );
    let first = commit(&repository, "feature", "a.txt", "1", "2026-01-02T09:00:00Z");
    let pull = repository
        .create_pull_request(
            1,
            PullRequestSeed {
                draft: SeedField::present(true),
                created_at: SeedField::present("2026-01-02T10:00:00Z".to_owned()),
                user: SeedField::present(user("alice")),
                base: SeedField::present(PullRequestBranchSeed {
                    branch_ref: SeedField::present("main".to_owned()),
                    sha: SeedField::present(base.clone()),
                    ..Default::default()
                }),
                head: SeedField::present(PullRequestBranchSeed {
                    branch_ref: SeedField::present("feature".to_owned()),
                    sha: SeedField::present(first.clone()),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .unwrap();
    pull.create_review(ReviewSeed {
        state: SeedField::present("COMMENTED".to_owned()),
        user: SeedField::present(Some(user("bob"))),
        submitted_at: SeedField::present(Some("2026-01-02T11:00:00Z".to_owned())),
        ..Default::default()
    })
    .unwrap();
    pull.create_timeline_event(event("ready_for_review", "2026-01-02T12:00:00Z"))
        .unwrap();
    pull.create_timeline_event(TimelineEventSeed {
        requested_reviewer: SeedField::present(user("carol")),
        ..event("review_requested", "2026-01-02T12:00:30Z")
    })
    .unwrap();
    pull.create_issue_comment(IssueCommentSeed {
        body: SeedField::present("Looks close.".to_owned()),
        user: SeedField::present(Some(user("carol"))),
        created_at: SeedField::present("2026-01-02T13:00:00Z".to_owned()),
        ..Default::default()
    })
    .unwrap();
    pull.create_timeline_event(event("convert_to_draft", "2026-01-02T14:00:00Z"))
        .unwrap();
    let rewritten = commit(&repository, "rewrite", "a.txt", "2", "2026-01-02T15:00:00Z");
    pull.push_head(&rewritten, Some("2026-01-02T15:05:00Z"), None)
        .unwrap();
    pull.create_timeline_event(event("ready_for_review", "2026-01-02T16:00:00Z"))
        .unwrap();
    pull.create_review(ReviewSeed {
        state: SeedField::present("APPROVED".to_owned()),
        user: SeedField::present(Some(user("carol"))),
        submitted_at: SeedField::present(Some("2026-01-02T17:00:00Z".to_owned())),
        ..Default::default()
    })
    .unwrap();
    pull.create_timeline_event(TimelineEventSeed {
        actor: SeedField::present(user("admin")),
        ..event("merged", "2026-01-02T18:00:00Z")
    })
    .unwrap();
    pull.create_timeline_event(TimelineEventSeed {
        actor: SeedField::present(user("admin")),
        ..event("closed", "2026-01-02T18:00:00Z")
    })
    .unwrap();

    let api = Api::admin(&server);
    let timeline = api.get("/repos/admin/project/issues/1/timeline");
    assert_eq!(timeline.status, 200, "{}", timeline.body);
    assert_eq!(
        kinds(&timeline.body),
        [
            "reviewed",
            "ready_for_review",
            "review_requested",
            "commented",
            "convert_to_draft",
            "committed",
            "head_ref_force_pushed",
            "ready_for_review",
            "reviewed",
            "merged",
            "closed",
        ]
    );
    let entries = timeline.body.as_array().unwrap();
    assert_eq!(entries[0]["state"], "commented");
    assert_eq!(entries[0]["user"]["login"], "bob");
    assert_eq!(entries[0]["submitted_at"], "2026-01-02T11:00:00Z");
    assert_eq!(entries[1]["actor"]["login"], "alice");
    assert_eq!(entries[1]["created_at"], "2026-01-02T12:00:00Z");
    assert_eq!(entries[2]["requested_reviewer"]["login"], "carol");
    assert_eq!(entries[2]["review_requester"]["login"], "alice");
    assert_eq!(entries[3]["created_at"], "2026-01-02T13:00:00Z");
    assert_eq!(entries[5]["sha"], rewritten);
    assert!(entries[5].get("created_at").is_none());
    assert_eq!(entries[5]["author"]["date"], "2026-01-02T15:00:00Z");
    assert_eq!(entries[5]["committer"]["email"], "alice@example.com");
    assert_eq!(entries[6]["commit_id"], rewritten);
    assert_eq!(entries[6]["created_at"], "2026-01-02T15:05:00Z");
    assert_eq!(entries[8]["state"], "approved");
    assert_eq!(entries[9]["commit_id"], rewritten);
    assert_eq!(entries[9]["actor"]["login"], "admin");
    assert_eq!(entries[10]["created_at"], "2026-01-02T18:00:00Z");
    let ids = entries
        .iter()
        .filter_map(|entry| entry["id"].as_i64())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(ids.len(), 10, "every non-commit entry has its own id");

    let read = api.get("/repos/admin/project/pulls/1");
    assert_eq!(read.body["state"], "closed");
    assert_eq!(read.body["merged"], true);
    assert_eq!(read.body["draft"], false);
    assert_eq!(read.body["merged_at"], "2026-01-02T18:00:00Z");
    assert_eq!(read.body["closed_at"], "2026-01-02T18:00:00Z");
    assert_eq!(read.body["merged_by"]["login"], "admin");
    assert_eq!(read.body["head"]["sha"], rewritten);

    let commits = api.get("/repos/admin/project/pulls/1/commits");
    assert_eq!(commits.status, 200, "{}", commits.body);
    let commits = commits.body.as_array().unwrap();
    assert_eq!(commits.len(), 1);
    assert_eq!(commits[0]["sha"], rewritten);
    assert_eq!(
        commits[0]["commit"]["committer"]["date"],
        "2026-01-02T15:00:00Z"
    );
}

#[test]
fn api_actions_append_their_timeline_events() {
    let server = Server::start().unwrap();
    server.add_user("bob");
    let bob = server.add_token("bob", &["repo"]);
    let repository = server
        .create_repository("admin", "project", false, None, None, "main")
        .unwrap();
    commit(
        &repository,
        "main",
        "base.txt",
        "base",
        "2026-01-01T00:00:00Z",
    );
    let head = commit(&repository, "feature", "a.txt", "1", "2026-01-02T00:00:00Z");
    let admin = Api::admin(&server);
    let created = admin.post(
        "/repos/admin/project/pulls",
        json!({"title": "Feature", "head": "feature", "base": "main", "draft": true}),
    );
    assert_eq!(created.status, 201, "{}", created.body);
    let ready = admin.post(
        "/api/graphql",
        json!({
            "query": "mutation($id: ID!){markPullRequestReadyForReview(input: {pullRequestId: $id}){pullRequest{id}}}",
            "variables": {"id": created.body["node_id"]},
        }),
    );
    assert!(ready.body.get("errors").is_none(), "{}", ready.body);
    let path = "/repos/admin/project/pulls/1";
    let reviewers = format!("{path}/requested_reviewers");
    assert_eq!(
        admin.post(&reviewers, json!({"reviewers": ["bob"]})).status,
        201
    );
    let removed = admin.delete_json(&reviewers, json!({"reviewers": ["bob"]}));
    assert_eq!(removed.status, 200, "{}", removed.body);
    assert!(
        removed.body["requested_reviewers"]
            .as_array()
            .is_none_or(Vec::is_empty),
        "{}",
        removed.body
    );
    admin.post(&reviewers, json!({"reviewers": ["bob"]}));
    let review =
        Api::new(&server, Some(&bob)).post(&format!("{path}/reviews"), json!({"event": "APPROVE"}));
    assert_eq!(review.status, 200, "{}", review.body);
    admin.post(
        "/repos/admin/project/issues/1/comments",
        json!({"body": "Merging."}),
    );
    assert_eq!(admin.patch(path, json!({"state": "closed"})).status, 200);
    assert_eq!(admin.patch(path, json!({"state": "open"})).status, 200);
    let merged = admin.put(&format!("{path}/merge"), json!({}));
    assert_eq!(merged.status, 200, "{}", merged.body);

    let timeline = admin.get("/repos/admin/project/issues/1/timeline");
    assert_eq!(timeline.status, 200, "{}", timeline.body);
    assert_eq!(
        kinds(&timeline.body),
        [
            "committed",
            "ready_for_review",
            "review_requested",
            "review_request_removed",
            "review_requested",
            "reviewed",
            "commented",
            "closed",
            "reopened",
            "merged",
            "closed",
        ]
    );
    let entries = timeline.body.as_array().unwrap();
    assert_eq!(entries[0]["sha"], head);
    assert_eq!(entries[3]["requested_reviewer"]["login"], "bob");
    assert_eq!(entries[5]["user"]["login"], "bob");
    assert_eq!(entries[9]["commit_id"], head);
    let read = admin.get(path);
    assert_eq!(read.body["merged_at"], entries[9]["created_at"]);
    assert_eq!(read.body["closed_at"], entries[10]["created_at"]);
    assert_eq!(read.body["merged_by"]["login"], "admin");
    assert_eq!(read.body["draft"], false);
}

#[test]
fn merged_and_closed_seeds_write_their_events() {
    let server = Server::start().unwrap();
    let repository = server
        .create_repository("admin", "project", false, None, None, "main")
        .unwrap();
    repository
        .create_pull_request(
            1,
            PullRequestSeed {
                user: SeedField::present(user("alice")),
                created_at: SeedField::present("2026-02-01T00:00:00Z".to_owned()),
                merged_at: SeedField::present("2026-02-03T00:00:00Z".to_owned()),
                merged_by: SeedField::present(user("bob")),
                ..Default::default()
            },
        )
        .unwrap();
    repository
        .create_pull_request(
            2,
            PullRequestSeed {
                user: SeedField::present(user("alice")),
                created_at: SeedField::present("2026-02-01T00:00:00Z".to_owned()),
                closed_at: SeedField::present("2026-02-02T00:00:00Z".to_owned()),
                ..Default::default()
            },
        )
        .unwrap();
    let api = Api::admin(&server);
    let merged = api.get("/repos/admin/project/pulls/1").body;
    assert_eq!(
        (
            &merged["state"],
            &merged["merged"],
            &merged["closed_at"],
            &merged["merged_by"]["login"]
        ),
        (
            &json!("closed"),
            &json!(true),
            &json!("2026-02-03T00:00:00Z"),
            &json!("bob")
        )
    );
    let timeline = api.get("/repos/admin/project/issues/1/timeline").body;
    assert_eq!(kinds(&timeline), ["merged", "closed"]);
    assert_eq!(timeline[0]["actor"]["login"], "bob");
    let closed = api.get("/repos/admin/project/pulls/2").body;
    assert_eq!(
        (&closed["state"], &closed["merged"], &closed["merged_at"]),
        (&json!("closed"), &json!(false), &Value::Null)
    );
    let timeline = api.get("/repos/admin/project/issues/2/timeline").body;
    assert_eq!(kinds(&timeline), ["closed"]);
    assert_eq!(timeline[0]["actor"]["login"], "alice");
}

#[test]
fn contradictory_history_seeds_are_refused() {
    let server = Server::start().unwrap();
    let repository = server
        .create_repository("admin", "project", false, None, None, "main")
        .unwrap();
    let refused = [
        PullRequestSeed {
            merged: SeedField::present(false),
            merged_at: SeedField::present("2026-02-03T00:00:00Z".to_owned()),
            ..Default::default()
        },
        PullRequestSeed {
            state: SeedField::present("open".to_owned()),
            closed_at: SeedField::present("2026-02-03T00:00:00Z".to_owned()),
            ..Default::default()
        },
        PullRequestSeed {
            draft: SeedField::present(true),
            merged_at: SeedField::present("2026-02-03T00:00:00Z".to_owned()),
            ..Default::default()
        },
        PullRequestSeed {
            merged_at: SeedField::present("2026-02-03T00:00:00Z".to_owned()),
            closed_at: SeedField::present("2026-02-04T00:00:00Z".to_owned()),
            ..Default::default()
        },
        PullRequestSeed {
            created_at: SeedField::present("2026-02-05T00:00:00Z".to_owned()),
            closed_at: SeedField::present("2026-02-04T00:00:00Z".to_owned()),
            ..Default::default()
        },
        PullRequestSeed {
            additions: SeedField::present(3),
            files: SeedField::present(vec![PullRequestFileSeed {
                filename: "a.txt".to_owned(),
                additions: SeedField::present(2),
                ..Default::default()
            }]),
            ..Default::default()
        },
    ];
    for (number, seed) in (1..).zip(refused) {
        let Err(error) = repository.create_pull_request(number, seed) else {
            panic!("seed {number} was accepted");
        };
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput, "{error}");
        assert!(repository.pull_request(number).unwrap().is_none());
    }

    let pull = repository
        .create_pull_request(
            10,
            PullRequestSeed {
                created_at: SeedField::present("2026-02-01T00:00:00Z".to_owned()),
                ..Default::default()
            },
        )
        .unwrap();
    for refused in [
        event("ready_for_review", "2026-02-02T00:00:00Z"),
        event("convert_to_draft", "2026-01-31T00:00:00Z"),
        event("head_ref_force_pushed", "2026-02-02T00:00:00Z"),
        event("reopened", "2026-02-02T00:00:00Z"),
        event("review_requested", "2026-02-02T00:00:00Z"),
        event("labeled", "2026-02-02T00:00:00Z"),
    ] {
        let name = refused.event.clone();
        let error = pull.create_timeline_event(refused).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput, "{name}");
    }
    pull.create_timeline_event(event("convert_to_draft", "2026-02-03T00:00:00Z"))
        .unwrap();
    let error = pull
        .create_timeline_event(event("ready_for_review", "2026-02-02T00:00:00Z"))
        .unwrap_err();
    assert!(error.to_string().contains("earlier"), "{error}");
}

#[test]
fn long_histories_page_with_links_that_keep_the_query() {
    let server = Server::start().unwrap();
    let repository = server
        .create_repository("admin", "project", false, None, None, "main")
        .unwrap();
    let pull = repository
        .create_pull_request(
            1,
            PullRequestSeed {
                created_at: SeedField::present("2026-03-01T00:00:00Z".to_owned()),
                ..Default::default()
            },
        )
        .unwrap();
    for minute in 0..130 {
        pull.create_issue_comment(IssueCommentSeed {
            body: SeedField::present(format!("comment {minute}")),
            created_at: SeedField::present(format!(
                "2026-03-01T{:02}:{:02}:00Z",
                1 + minute / 60,
                minute % 60
            )),
            ..Default::default()
        })
        .unwrap();
    }
    let api = Api::admin(&server);
    let mut bodies = Vec::new();
    let mut next = Some("/repos/admin/project/issues/1/timeline?per_page=100".to_owned());
    while let Some(path) = next {
        let page = api.get(&path);
        assert_eq!(page.status, 200, "{}", page.body);
        bodies.extend(page.body.as_array().unwrap().iter().cloned());
        next = page
            .header("link")
            .and_then(|header| link(&server, header, "next"));
    }
    assert_eq!(bodies.len(), 130);
    assert_eq!(bodies[129]["body"], "comment 129");
    let ids = bodies
        .iter()
        .map(|entry| entry["id"].as_i64().unwrap())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(ids.len(), 130);

    let comments = api.get("/repos/admin/project/issues/1/comments?per_page=50&page=3");
    assert_eq!(comments.body.as_array().unwrap().len(), 30);
    assert_eq!(
        link(&server, comments.header("link").unwrap(), "prev").as_deref(),
        Some("/repos/admin/project/issues/1/comments?per_page=50&page=2")
    );

    for (number, created, updated) in [
        (2, "2026-03-02T00:00:00Z", "2026-03-09T00:00:00Z"),
        (3, "2026-03-03T00:00:00Z", "2026-03-04T00:00:00Z"),
    ] {
        repository
            .create_pull_request(
                number,
                PullRequestSeed {
                    created_at: SeedField::present(created.to_owned()),
                    updated_at: SeedField::present(updated.to_owned()),
                    closed_at: SeedField::present(updated.to_owned()),
                    ..Default::default()
                },
            )
            .unwrap();
    }
    let numbers = |reply: &common::Reply| {
        reply
            .body
            .as_array()
            .unwrap()
            .iter()
            .map(|pull| pull["number"].as_i64().unwrap())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        numbers(&api.get("/repos/admin/project/pulls?state=all")),
        [3, 2, 1]
    );
    let first = api.get("/repos/admin/project/pulls?state=closed&sort=updated&per_page=1");
    // GitHub sorts ascending by default for every sort but `created`.
    assert_eq!(numbers(&first), [3]);
    let second = link(&server, first.header("link").unwrap(), "next").unwrap();
    assert!(
        second.contains("state=closed&sort=updated&per_page=1&page=2"),
        "{second}"
    );
    assert_eq!(numbers(&api.get(&second)), [2]);
    assert_eq!(
        numbers(&api.get("/repos/admin/project/pulls?state=all&sort=updated&direction=desc")),
        [1, 2, 3]
    );
}

#[test]
fn workflow_runs_filter_by_head_sha_newest_first() {
    let server = Server::start().unwrap();
    let repository = server
        .create_repository("admin", "project", false, None, None, "main")
        .unwrap();
    for (id, sha, event, created_at) in [
        (1, "a".repeat(40), "pull_request", "2026-04-01T10:00:00Z"),
        (2, "b".repeat(40), "pull_request", "2026-04-01T09:00:00Z"),
        (3, "a".repeat(40), "push", "2026-04-01T11:00:00Z"),
        (4, "a".repeat(40), "pull_request", "2026-04-01T08:00:00Z"),
    ] {
        repository
            .create_workflow_run(
                WorkflowRunSeed {
                    id: SeedField::present(id),
                    head_sha: SeedField::present(sha),
                    head_branch: SeedField::present(Some("feature".to_owned())),
                    event: SeedField::present(event.to_owned()),
                    created_at: SeedField::present(created_at.to_owned()),
                    ..Default::default()
                },
                Vec::new(),
            )
            .unwrap();
    }
    let api = Api::admin(&server);
    let sha = "a".repeat(40);
    let page = api.get(&format!(
        "/repos/admin/project/actions/runs?branch=feature&event=pull_request&head_sha={sha}&per_page=1"
    ));
    assert_eq!(page.status, 200, "{}", page.body);
    assert_eq!(page.body["total_count"], 2);
    assert_eq!(page.body["workflow_runs"][0]["id"], 1);
    let next = link(&server, page.header("link").unwrap(), "next").unwrap();
    assert!(next.contains(&format!("head_sha={sha}")), "{next}");
    let page = api.get(&next);
    assert_eq!(page.body["workflow_runs"][0]["id"], 4);
    assert_eq!(
        page.body["workflow_runs"][0]["created_at"],
        "2026-04-01T08:00:00Z"
    );
}

#[test]
fn seeded_files_list_and_size_the_pull_request() {
    let server = Server::start().unwrap();
    let repository = server
        .create_repository("admin", "project", false, None, None, "main")
        .unwrap();
    repository
        .create_pull_request(
            1,
            PullRequestSeed {
                files: SeedField::present(vec![
                    PullRequestFileSeed {
                        filename: "poetry.lock".to_owned(),
                        additions: SeedField::present(10),
                        deletions: SeedField::present(4),
                        ..Default::default()
                    },
                    PullRequestFileSeed {
                        filename: "docs/new.md".to_owned(),
                        status: SeedField::present("added".to_owned()),
                        additions: SeedField::present(3),
                        ..Default::default()
                    },
                ]),
                ..Default::default()
            },
        )
        .unwrap();
    let api = Api::admin(&server);
    let files = api.get("/repos/admin/project/pulls/1/files");
    assert_eq!(files.status, 200, "{}", files.body);
    assert_eq!(files.body[0]["filename"], "poetry.lock");
    assert_eq!(files.body[0]["changes"], 14);
    assert_eq!(files.body[1]["status"], "added");
    let pull = api.get("/repos/admin/project/pulls/1").body;
    assert_eq!(
        (
            &pull["changed_files"],
            &pull["additions"],
            &pull["deletions"]
        ),
        (&json!(2), &json!(13), &json!(4))
    );
}

#[test]
fn git_pushes_move_the_head_and_record_force_pushes() {
    let server = Server::start().unwrap();
    let repository = server
        .create_repository("admin", "project", false, None, None, "main")
        .unwrap();
    commit(
        &repository,
        "main",
        "base.txt",
        "base",
        "2026-01-01T00:00:00Z",
    );
    let first = commit(&repository, "feature", "a.txt", "1", "2026-01-02T00:00:00Z");
    repository
        .create_pull_request(
            1,
            PullRequestSeed {
                head: SeedField::present(PullRequestBranchSeed {
                    branch_ref: SeedField::present("feature".to_owned()),
                    sha: SeedField::present(first.clone()),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .unwrap();
    let url = clone_url(&server, "admin", "project", Some(server.admin_token()));
    let work = TempDir::new();
    git_ok(&work.0, &["clone", "-q", "-b", "feature", &url, "c"]);
    let clone = work.join("c");
    std::fs::write(clone.join("b.txt"), "2").unwrap();
    git_ok(&clone, &["add", "b.txt"]);
    git_ok(&clone, &["commit", "-q", "-m", "Add b"]);
    git_ok(&clone, &["push", "-q", "origin", "feature"]);
    let fast_forward = git_ok(&clone, &["rev-parse", "HEAD"]);
    git_ok(&clone, &["commit", "-q", "--amend", "-m", "Add b again"]);
    git_ok(&clone, &["push", "-q", "-f", "origin", "feature"]);
    let forced = git_ok(&clone, &["rev-parse", "HEAD"]);
    assert_ne!(fast_forward, forced);

    let api = Api::admin(&server);
    let read = api.get("/repos/admin/project/pulls/1").body;
    assert_eq!(read["head"]["sha"], forced);
    assert_eq!(
        read["commits"], 2,
        "the count agrees with the commits git lists"
    );
    let timeline = api.get("/repos/admin/project/issues/1/timeline").body;
    assert_eq!(
        kinds(&timeline),
        ["committed", "committed", "head_ref_force_pushed"]
    );
    assert_eq!(timeline[0]["sha"], first);
    assert_eq!(timeline[1]["sha"], forced);
    assert_eq!(timeline[2]["commit_id"], forced);
    assert_eq!(timeline[2]["actor"]["login"], "admin");
}

#[test]
fn untimed_closed_seeds_keep_their_updated_at() {
    let server = Server::start().unwrap();
    let repository = server
        .create_repository("admin", "project", false, None, None, "main")
        .unwrap();
    repository
        .create_pull_request(
            1,
            PullRequestSeed {
                state: SeedField::present("closed".to_owned()),
                merged: SeedField::present(true),
                updated_at: SeedField::present("2026-01-01T00:00:00Z".to_owned()),
                ..Default::default()
            },
        )
        .unwrap();
    let pull = Api::admin(&server).get("/repos/admin/project/pulls/1").body;
    assert_eq!(
        (&pull["updated_at"], &pull["merged_at"], &pull["closed_at"]),
        (
            &json!("2026-01-01T00:00:00Z"),
            &json!("2026-01-01T00:00:00Z"),
            &json!("2026-01-01T00:00:00Z")
        )
    );
}

#[test]
fn team_review_requests_appear_in_the_timeline() {
    let server = Server::start().unwrap();
    server.add_org("acme", &["admin"]);
    let repository = server
        .create_repository("acme", "project", false, None, None, "main")
        .unwrap();
    let pull = repository
        .create_pull_request(
            1,
            PullRequestSeed {
                created_at: SeedField::present("2026-05-01T00:00:00Z".to_owned()),
                ..Default::default()
            },
        )
        .unwrap();
    pull.create_timeline_event(TimelineEventSeed {
        requested_team: SeedField::present(ninot_github::fixtures::TeamSeed {
            slug: "reviewers".to_owned(),
        }),
        ..event("review_requested", "2026-05-01T01:00:00Z")
    })
    .unwrap();
    pull.create_timeline_event(TimelineEventSeed {
        requested_team: SeedField::present(ninot_github::fixtures::TeamSeed {
            slug: "reviewers".to_owned(),
        }),
        ..event("review_request_removed", "2026-05-01T02:00:00Z")
    })
    .unwrap();
    let timeline = Api::admin(&server).get("/repos/acme/project/issues/1/timeline");
    assert_eq!(timeline.status, 200, "{}", timeline.body);
    assert_eq!(
        kinds(&timeline.body),
        ["review_requested", "review_request_removed"]
    );
    assert_eq!(timeline.body[0]["requested_team"]["slug"], "reviewers");
    assert!(timeline.body[0].get("requested_reviewer").is_none());
}

#[test]
fn review_comments_belong_to_the_seeded_review() {
    let server = Server::start().unwrap();
    let repository = server
        .create_repository("admin", "project", false, None, None, "main")
        .unwrap();
    let pull = repository
        .create_pull_request(1, PullRequestSeed::default())
        .unwrap();
    let review = pull.create_review(ReviewSeed::default()).unwrap();
    for _ in 0..2 {
        pull.create_review_comment(ninot_github::fixtures::ReviewCommentSeed {
            pull_request_review_id: SeedField::present(review.id().unwrap()),
            ..Default::default()
        })
        .unwrap();
    }
    let comments = Api::admin(&server)
        .get("/repos/admin/project/pulls/1/comments")
        .body;
    let comments = comments.as_array().unwrap();
    assert_eq!(comments.len(), 2);
    assert_ne!(comments[0]["id"], comments[1]["id"]);
    for comment in comments {
        assert_eq!(comment["pull_request_review_id"], review.id().unwrap());
    }
}

#[test]
fn thread_replies_are_reviews_but_not_timeline_entries() {
    let server = Server::start().unwrap();
    let repository = server
        .create_repository("admin", "project", false, None, None, "main")
        .unwrap();
    let pull = repository
        .create_pull_request(1, PullRequestSeed::default())
        .unwrap();
    let mut review_ids = Vec::new();
    for (login, at) in [
        ("bob", "2026-01-02T11:00:00Z"),
        ("carol", "2026-01-02T12:00:00Z"),
    ] {
        let review = pull
            .create_review(ReviewSeed {
                state: SeedField::present("COMMENTED".to_owned()),
                user: SeedField::present(Some(user(login))),
                submitted_at: SeedField::present(Some(at.to_owned())),
                ..Default::default()
            })
            .unwrap();
        review_ids.push(review.id().unwrap());
    }
    pull.create_review_comment(ninot_github::fixtures::ReviewCommentSeed {
        pull_request_review_id: SeedField::present(review_ids[0]),
        ..Default::default()
    })
    .unwrap();
    let api = Api::admin(&server);
    let first = api.get("/repos/admin/project/pulls/1/comments").body[0]["id"]
        .as_i64()
        .unwrap();
    pull.create_review_comment(ninot_github::fixtures::ReviewCommentSeed {
        pull_request_review_id: SeedField::present(review_ids[1]),
        in_reply_to_id: SeedField::present(first),
        ..Default::default()
    })
    .unwrap();

    let reply = &api.get("/repos/admin/project/pulls/1/comments").body[1];
    assert_eq!(reply["in_reply_to_id"], first);
    assert_eq!(
        api.get("/repos/admin/project/pulls/1/reviews")
            .body
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let timeline = api.get("/repos/admin/project/issues/1/timeline").body;
    assert_eq!(kinds(&timeline), ["reviewed"]);
    assert_eq!(timeline[0]["id"], review_ids[0]);
}
