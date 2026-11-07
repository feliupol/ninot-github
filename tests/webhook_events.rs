//! Webhook events beyond ping and push, checked through the HTTP API and recorded deliveries.

mod common;

use std::path::Path;
use std::time::Duration;

use common::{Api, Receiver};
use ninot_github::{Delivery, Server};
use serde_json::{Value, json};

fn subscribe(admin: &Api, receiver: &Receiver, events: &[&str]) {
    let created = admin.post(
        "/repos/admin/tool/hooks",
        json!({
            "config": { "url": receiver.url, "content_type": "json" },
            "events": events,
        }),
    );
    assert_eq!(created.status, 201, "{}", created.body);
}

/// Waits for `count` finished deliveries and fails when any payload was rejected.
fn finished(server: &Server, count: usize) -> Vec<Delivery> {
    let deliveries = server.wait_for_deliveries(count, Duration::from_secs(5));
    assert_eq!(deliveries.len(), count, "{:?}", events_of(&deliveries));
    for delivery in &deliveries {
        assert!(
            delivery.error.is_none(),
            "{} {}: {}",
            delivery.event,
            delivery.payload()["action"],
            delivery.error.as_deref().unwrap_or("")
        );
    }
    deliveries
}

fn events_of(deliveries: &[Delivery]) -> Vec<String> {
    deliveries
        .iter()
        .map(|delivery| format!("{}:{}", delivery.event, delivery.payload()["action"]))
        .collect()
}

fn owned_action(deliveries: &[Delivery], event: &str, name: &str) -> Value {
    deliveries
        .iter()
        .find(|delivery| delivery.event == event && delivery.payload()["action"] == name)
        .unwrap_or_else(|| {
            panic!(
                "missing {event} {name} in {}",
                events_of(deliveries).join(", ")
            )
        })
        .payload()
}

fn branches(server: &Server) -> ninot_github::Repository {
    let repository = server
        .create_repository("admin", "tool", false, None, None, "main")
        .unwrap();
    for branch in ["main", "feature"] {
        repository
            .commit_file(branch, Path::new("file.txt"), b"one\n", "Add file")
            .unwrap();
    }
    repository
}

#[test]
fn pull_request_actions_are_delivered() {
    let server = Server::start().unwrap();
    server.add_user("alice");
    let repository = branches(&server);
    let admin = Api::admin(&server);
    let receiver = Receiver::start(200);
    subscribe(&admin, &receiver, &["pull_request"]);

    let created = admin.post(
        "/repos/admin/tool/pulls",
        json!({"title": "Draft", "head": "feature", "base": "main"}),
    );
    assert_eq!(created.status, 201, "{}", created.body);
    let number = created.body["number"].as_u64().unwrap();
    let path = format!("/repos/admin/tool/pulls/{number}");
    assert_eq!(admin.patch(&path, json!({"title": "Ready"})).status, 200);
    assert_eq!(
        admin
            .post(
                &format!("/repos/admin/tool/issues/{number}/labels"),
                json!({"labels": ["bug"]}),
            )
            .status,
        200
    );
    assert_eq!(
        admin
            .post(
                &format!("{path}/requested_reviewers"),
                json!({"reviewers": ["alice"]}),
            )
            .status,
        201
    );
    repository
        .commit_file("feature", Path::new("file.txt"), b"two\n", "Update file")
        .unwrap();
    assert_eq!(admin.patch(&path, json!({"state": "closed"})).status, 200);
    assert_eq!(admin.patch(&path, json!({"state": "open"})).status, 200);
    let merged = admin.put(&format!("{path}/merge"), json!({}));
    assert_eq!(merged.status, 200, "{}", merged.body);

    let deliveries = finished(&server, 9);
    assert_eq!(deliveries[0].event, "ping");
    let opened = owned_action(&deliveries, "pull_request", "opened");
    assert_eq!(opened["number"], number);
    assert_eq!(
        owned_action(&deliveries, "pull_request", "edited")["changes"]["title"]["from"],
        "Draft"
    );
    assert_eq!(
        owned_action(&deliveries, "pull_request", "labeled")["label"]["name"],
        "bug"
    );
    assert_eq!(
        owned_action(&deliveries, "pull_request", "review_requested")["requested_reviewer"]["login"],
        "alice"
    );
    let synchronized = owned_action(&deliveries, "pull_request", "synchronize");
    assert_ne!(synchronized["before"], synchronized["after"]);
    let closed: Vec<Value> = deliveries
        .iter()
        .filter(|delivery| {
            delivery.event == "pull_request" && delivery.payload()["action"] == "closed"
        })
        .map(Delivery::payload)
        .collect();
    assert_eq!(closed.len(), 2);
    assert_eq!(closed[0]["pull_request"]["merged"], false);
    assert_eq!(closed[1]["pull_request"]["merged"], true);
    assert_eq!(
        owned_action(&deliveries, "pull_request", "reopened")["pull_request"]["state"],
        "open"
    );
}

#[test]
fn pull_request_review_submitted_is_delivered() {
    let server = Server::start().unwrap();
    branches(&server);
    let admin = Api::admin(&server);
    let receiver = Receiver::start(200);
    subscribe(&admin, &receiver, &["pull_request_review"]);
    let created = admin.post(
        "/repos/admin/tool/pulls",
        json!({"title": "Review", "head": "feature", "base": "main"}),
    );
    let number = created.body["number"].as_u64().unwrap();
    let pending = admin.post(
        &format!("/repos/admin/tool/pulls/{number}/reviews"),
        json!({"body": "not yet"}),
    );
    assert_eq!(pending.status, 200, "{}", pending.body);
    assert_eq!(
        server.deliveries().len(),
        1,
        "a pending review is not delivered"
    );
    let submitted = admin.post(
        &format!("/repos/admin/tool/pulls/{number}/reviews"),
        json!({"event": "COMMENT", "body": "Looks good"}),
    );
    assert_eq!(submitted.status, 200, "{}", submitted.body);
    let deliveries = finished(&server, 2);
    assert_eq!(deliveries[1].event, "pull_request_review");
    assert_eq!(deliveries[1].payload()["action"], "submitted");
    assert_eq!(deliveries[1].payload()["review"]["state"], "commented");
}

#[test]
fn pull_request_review_comment_created_is_delivered() {
    let server = Server::start().unwrap();
    let repository = server
        .create_repository("admin", "tool", false, None, None, "main")
        .unwrap();
    let pull = repository
        .create_pull_request(1, ninot_github::fixtures::PullRequestSeed::default())
        .unwrap();
    let admin = Api::admin(&server);
    let receiver = Receiver::start(200);
    subscribe(&admin, &receiver, &["pull_request_review_comment"]);
    pull.create_review_comment(ninot_github::fixtures::ReviewCommentSeed::default())
        .unwrap();
    let deliveries = finished(&server, 2);
    assert_eq!(deliveries[1].event, "pull_request_review_comment");
    assert_eq!(deliveries[1].payload()["action"], "created");
    assert_eq!(deliveries[1].payload()["comment"]["path"], "README.md");
}

#[test]
fn issues_opened_labeled_and_issue_comment_are_delivered() {
    let server = Server::start().unwrap();
    server
        .create_repository("admin", "tool", false, None, None, "main")
        .unwrap();
    let admin = Api::admin(&server);
    let receiver = Receiver::start(200);
    subscribe(&admin, &receiver, &["issues", "issue_comment"]);
    let created = admin.post(
        "/repos/admin/tool/issues",
        json!({"title": "Bug", "labels": ["bug"]}),
    );
    assert_eq!(created.status, 201, "{}", created.body);
    let number = created.body["number"].as_u64().unwrap();
    assert_eq!(
        admin
            .post(
                &format!("/repos/admin/tool/issues/{number}/labels"),
                json!({"labels": ["extra"]}),
            )
            .status,
        200
    );
    assert_eq!(
        admin
            .post(
                &format!("/repos/admin/tool/issues/{number}/comments"),
                json!({"body": "I see it"}),
            )
            .status,
        201
    );
    let deliveries = finished(&server, 5);
    assert_eq!(deliveries[0].event, "ping");
    assert_eq!(
        owned_action(&deliveries, "issues", "opened")["issue"]["title"],
        "Bug"
    );
    let labels: Vec<String> = deliveries
        .iter()
        .filter(|delivery| delivery.event == "issues" && delivery.payload()["action"] == "labeled")
        .map(|delivery| {
            delivery.payload()["label"]["name"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(labels, ["bug", "extra"]);
    assert_eq!(
        owned_action(&deliveries, "issue_comment", "created")["comment"]["body"],
        "I see it"
    );
}

#[test]
fn check_run_and_check_suite_are_delivered_from_the_commit_fixture() {
    let server = Server::start().unwrap();
    let repository = server
        .create_repository("admin", "tool", false, None, None, "main")
        .unwrap();
    let commit = repository
        .commit_file("main", Path::new("file.txt"), b"one\n", "Add file")
        .unwrap();
    let admin = Api::admin(&server);
    let receiver = Receiver::start(200);
    subscribe(&admin, &receiver, &["check_run", "check_suite"]);
    commit
        .create_check_run(ninot_github::fixtures::CheckRunSeed::default())
        .unwrap();
    commit
        .create_check_run(ninot_github::fixtures::CheckRunSeed::default())
        .unwrap();
    let deliveries = finished(&server, 4);
    let suites = deliveries
        .iter()
        .filter(|delivery| delivery.event == "check_suite")
        .count();
    let runs = deliveries
        .iter()
        .filter(|delivery| delivery.event == "check_run")
        .count();
    assert_eq!(suites, 1);
    assert_eq!(runs, 2);
    assert_eq!(
        owned_action(&deliveries, "check_suite", "requested")["check_suite"]["head_sha"],
        commit.sha()
    );
    assert_eq!(
        owned_action(&deliveries, "check_run", "created")["check_run"]["head_sha"],
        commit.sha()
    );
}

#[test]
fn workflow_run_and_workflow_job_follow_the_run() {
    let server = Server::start().unwrap();
    let repository = server
        .create_repository("admin", "tool", false, None, None, "main")
        .unwrap();
    let admin = Api::admin(&server);
    let receiver = Receiver::start(200);
    subscribe(&admin, &receiver, &["workflow_run", "workflow_job"]);
    let run = repository
        .create_workflow_run(
            ninot_github::fixtures::WorkflowRunSeed::default(),
            vec![ninot_github::fixtures::JobSeed::default()],
        )
        .unwrap();
    let rerun = admin.post(
        &format!("/repos/admin/tool/actions/runs/{}/rerun", run.id()),
        json!({}),
    );
    assert_eq!(rerun.status, 201, "{}", rerun.body);
    let deliveries = finished(&server, 5);
    assert_eq!(
        owned_action(&deliveries, "workflow_run", "completed")["workflow_run"]["status"],
        "completed"
    );
    assert_eq!(
        owned_action(&deliveries, "workflow_job", "completed")["workflow_job"]["status"],
        "completed"
    );
    assert_eq!(
        owned_action(&deliveries, "workflow_run", "requested")["workflow_run"]["status"],
        "queued"
    );
    assert_eq!(
        owned_action(&deliveries, "workflow_job", "queued")["workflow_job"]["status"],
        "queued"
    );
}

#[test]
fn release_actions_are_delivered() {
    let server = Server::start().unwrap();
    let admin = Api::admin(&server);
    assert_eq!(
        admin
            .post("/user/repos", json!({"name": "tool", "auto_init": true}))
            .status,
        201
    );
    let receiver = Receiver::start(200);
    subscribe(&admin, &receiver, &["release"]);
    let draft = admin.post(
        "/repos/admin/tool/releases",
        json!({"tag_name": "v0", "name": "Draft", "draft": true}),
    );
    assert_eq!(draft.status, 201, "{}", draft.body);
    let id = &draft.body["id"];
    let path = format!("/repos/admin/tool/releases/{id}");
    assert_eq!(admin.patch(&path, json!({"draft": false})).status, 200);
    assert_eq!(admin.patch(&path, json!({"name": "Renamed"})).status, 200);
    assert_eq!(admin.patch(&path, json!({"draft": true})).status, 200);
    assert_eq!(admin.delete(&path).status, 204);
    let preview = admin.post(
        "/repos/admin/tool/releases",
        json!({"tag_name": "v1-rc", "name": "RC", "prerelease": true}),
    );
    assert_eq!(preview.status, 201, "{}", preview.body);
    let deliveries = finished(&server, 7);
    assert_eq!(deliveries[0].event, "ping");
    for name in [
        "created",
        "published",
        "edited",
        "unpublished",
        "deleted",
        "prereleased",
    ] {
        let payload = owned_action(&deliveries, "release", name);
        assert!(payload["release"]["tag_name"].is_string(), "{name}");
    }
    assert_eq!(
        owned_action(&deliveries, "release", "edited")["changes"]["name"]["from"],
        "Draft"
    );
}

#[test]
fn create_and_delete_are_delivered_for_branches_and_tags() {
    let server = Server::start().unwrap();
    let admin = Api::admin(&server);
    assert_eq!(
        admin
            .post("/user/repos", json!({"name": "tool", "auto_init": true}))
            .status,
        201
    );
    let receiver = Receiver::start(200);
    subscribe(&admin, &receiver, &["create", "delete"]);
    let sha = admin.get("/repos/admin/tool/git/ref/heads/main").body["object"]["sha"].clone();
    assert!(sha.is_string(), "{sha}");
    assert_eq!(
        admin
            .post(
                "/repos/admin/tool/git/refs",
                json!({"ref": "refs/heads/topic", "sha": sha})
            )
            .status,
        201
    );
    assert_eq!(
        admin
            .post(
                "/repos/admin/tool/git/refs",
                json!({"ref": "refs/tags/v1", "sha": sha})
            )
            .status,
        201
    );
    assert_eq!(
        admin
            .delete("/repos/admin/tool/git/refs/heads/topic")
            .status,
        204
    );
    assert_eq!(
        admin.delete("/repos/admin/tool/git/refs/tags/v1").status,
        204
    );
    let deliveries = finished(&server, 5);
    let refs: Vec<(String, String, String)> = deliveries
        .iter()
        .skip(1)
        .map(|delivery| {
            let payload = delivery.payload();
            (
                delivery.event.clone(),
                payload["ref"].as_str().unwrap().to_owned(),
                payload["ref_type"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(
        refs,
        [
            ("create".to_owned(), "topic".to_owned(), "branch".to_owned()),
            ("create".to_owned(), "v1".to_owned(), "tag".to_owned()),
            ("delete".to_owned(), "topic".to_owned(), "branch".to_owned()),
            ("delete".to_owned(), "v1".to_owned(), "tag".to_owned()),
        ]
    );
}

#[test]
fn status_is_delivered_from_the_commit_fixture() {
    let server = Server::start().unwrap();
    let repository = server
        .create_repository("admin", "tool", false, None, None, "main")
        .unwrap();
    let commit = repository
        .commit_file("main", Path::new("file.txt"), b"one\n", "Add file")
        .unwrap();
    let admin = Api::admin(&server);
    let receiver = Receiver::start(200);
    subscribe(&admin, &receiver, &["status"]);
    commit
        .create_status(ninot_github::fixtures::CommitStatusSeed::default())
        .unwrap();
    let deliveries = finished(&server, 2);
    assert_eq!(deliveries[1].event, "status");
    let payload = deliveries[1].payload();
    assert!(payload["action"].is_null());
    assert_eq!(payload["sha"], commit.sha());
    assert_eq!(payload["state"], "success");
    assert_eq!(payload["context"], "default");
    assert_eq!(payload["commit"]["sha"], commit.sha());
}
