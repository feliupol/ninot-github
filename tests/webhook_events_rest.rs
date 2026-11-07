//! Webhook deliveries from REST operations added after the object-handle events.

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

const APP_JWT: &str = "eyJhbGciOiJSUzI1NiJ9.eyJpc3MiOiIxIn0.signature";

fn app_token(server: &Server, installation: u64) -> String {
    let response = reqwest::blocking::Client::new()
        .post(format!(
            "{}/app/installations/{installation}/access_tokens",
            server.url()
        ))
        .bearer_auth(APP_JWT)
        .json(&json!({}))
        .send()
        .unwrap();
    assert_eq!(response.status(), 201, "{response:?}");
    let body: Value = response.json().unwrap();
    body["token"].as_str().unwrap().to_owned()
}

#[test]
fn issues_update_fires_closed_reopened_edited_assigned_labeled_and_locked() {
    let server = Server::start().unwrap();
    server
        .create_repository("admin", "tool", false, None, None, "main")
        .unwrap();
    let admin = Api::admin(&server);
    let receiver = Receiver::start(200);
    subscribe(&admin, &receiver, &["issues"]);
    assert_eq!(
        admin
            .post("/repos/admin/tool/labels", json!({"name": "bug"}))
            .status,
        201
    );
    let milestone = admin.post("/repos/admin/tool/milestones", json!({"title": "v1"}));
    assert_eq!(milestone.status, 201, "{}", milestone.body);
    let created = admin.post("/repos/admin/tool/issues", json!({"title": "Bug"}));
    assert_eq!(created.status, 201, "{}", created.body);
    let number = created.body["number"].as_u64().unwrap();
    let path = format!("/repos/admin/tool/issues/{number}");
    assert_eq!(
        admin
            .patch(&path, json!({"title": "Fixed", "body": "notes"}))
            .status,
        200
    );
    assert_eq!(admin.patch(&path, json!({"labels": ["bug"]})).status, 200);
    assert_eq!(admin.patch(&path, json!({"labels": []})).status, 200);
    assert_eq!(
        admin.patch(&path, json!({"assignees": ["admin"]})).status,
        200
    );
    assert_eq!(admin.patch(&path, json!({"assignees": []})).status, 200);
    assert_eq!(
        admin
            .patch(&path, json!({"milestone": milestone.body["number"]}))
            .status,
        200
    );
    assert_eq!(
        admin.patch(&path, json!({"milestone": Value::Null})).status,
        200
    );
    assert_eq!(admin.patch(&path, json!({"state": "closed"})).status, 200);
    assert_eq!(admin.patch(&path, json!({"state": "open"})).status, 200);
    assert_eq!(admin.put(&format!("{path}/lock"), json!({})).status, 204);
    assert_eq!(admin.delete(&format!("{path}/lock")).status, 204);

    let deliveries = finished(&server, 13);
    assert_eq!(deliveries[0].event, "ping");
    assert_eq!(
        owned_action(&deliveries, "issues", "opened")["issue"]["title"],
        "Bug"
    );
    assert_eq!(
        owned_action(&deliveries, "issues", "edited")["changes"]["title"]["from"],
        "Bug"
    );
    assert_eq!(
        owned_action(&deliveries, "issues", "labeled")["label"]["name"],
        "bug"
    );
    assert_eq!(
        owned_action(&deliveries, "issues", "unlabeled")["label"]["name"],
        "bug"
    );
    assert_eq!(
        owned_action(&deliveries, "issues", "assigned")["assignee"]["login"],
        "admin"
    );
    assert_eq!(
        owned_action(&deliveries, "issues", "unassigned")["assignee"]["login"],
        "admin"
    );
    assert_eq!(
        owned_action(&deliveries, "issues", "milestoned")["milestone"]["title"],
        "v1"
    );
    assert_eq!(
        owned_action(&deliveries, "issues", "demilestoned")["milestone"]["title"],
        "v1"
    );
    assert_eq!(
        owned_action(&deliveries, "issues", "closed")["issue"]["state"],
        "closed"
    );
    assert_eq!(
        owned_action(&deliveries, "issues", "reopened")["issue"]["state"],
        "open"
    );
    assert_eq!(
        owned_action(&deliveries, "issues", "locked")["issue"]["locked"],
        true
    );
    assert_eq!(
        owned_action(&deliveries, "issues", "unlocked")["issue"]["locked"],
        false
    );
}

#[test]
fn issues_label_and_assignee_operations_fire() {
    let server = Server::start().unwrap();
    server
        .create_repository("admin", "tool", false, None, None, "main")
        .unwrap();
    let admin = Api::admin(&server);
    let receiver = Receiver::start(200);
    subscribe(&admin, &receiver, &["issues"]);
    assert_eq!(
        admin
            .post("/repos/admin/tool/labels", json!({"name": "bug"}))
            .status,
        201
    );
    assert_eq!(
        admin
            .post("/repos/admin/tool/labels", json!({"name": "extra"}))
            .status,
        201
    );
    let created = admin.post("/repos/admin/tool/issues", json!({"title": "Work"}));
    let number = created.body["number"].as_u64().unwrap();
    let labels = format!("/repos/admin/tool/issues/{number}/labels");
    assert_eq!(admin.put(&labels, json!({"labels": ["bug"]})).status, 200);
    assert_eq!(
        admin.post(&labels, json!({"labels": ["extra"]})).status,
        200
    );
    assert_eq!(admin.delete(&format!("{labels}/bug")).status, 200);
    assert_eq!(admin.delete(&labels).status, 204);
    assert_eq!(
        admin
            .post(
                &format!("/repos/admin/tool/issues/{number}/assignees"),
                json!({"assignees": ["admin"]}),
            )
            .status,
        201
    );
    assert_eq!(
        admin
            .delete_json(
                &format!("/repos/admin/tool/issues/{number}/assignees"),
                json!({"assignees": ["admin"]}),
            )
            .status,
        200
    );
    let deliveries = finished(&server, 8);
    assert_eq!(
        owned_action(&deliveries, "issues", "labeled")["label"]["name"],
        "bug"
    );
    let names: Vec<String> = deliveries
        .iter()
        .filter(|delivery| delivery.event == "issues" && delivery.payload()["action"] == "labeled")
        .map(|delivery| {
            delivery.payload()["label"]["name"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(names, ["bug", "extra"]);
    assert_eq!(
        owned_action(&deliveries, "issues", "assigned")["assignee"]["login"],
        "admin"
    );
    assert_eq!(
        owned_action(&deliveries, "issues", "unassigned")["assignee"]["login"],
        "admin"
    );
}

#[test]
fn issue_comment_edited_and_deleted_are_delivered() {
    let server = Server::start().unwrap();
    server
        .create_repository("admin", "tool", false, None, None, "main")
        .unwrap();
    let admin = Api::admin(&server);
    let receiver = Receiver::start(200);
    subscribe(&admin, &receiver, &["issue_comment"]);
    let created = admin.post("/repos/admin/tool/issues", json!({"title": "Bug"}));
    let number = created.body["number"].as_u64().unwrap();
    let comment = admin.post(
        &format!("/repos/admin/tool/issues/{number}/comments"),
        json!({"body": "first"}),
    );
    assert_eq!(comment.status, 201, "{}", comment.body);
    let id = comment.body["id"].as_i64().unwrap();
    let path = format!("/repos/admin/tool/issues/comments/{id}");
    assert_eq!(admin.patch(&path, json!({"body": "second"})).status, 200);
    assert_eq!(admin.delete(&path).status, 204);
    let deliveries = finished(&server, 4);
    assert_eq!(
        owned_action(&deliveries, "issue_comment", "created")["comment"]["body"],
        "first"
    );
    let edited = owned_action(&deliveries, "issue_comment", "edited");
    assert_eq!(edited["comment"]["body"], "second");
    assert_eq!(edited["changes"]["body"]["from"], "first");
    assert_eq!(
        owned_action(&deliveries, "issue_comment", "deleted")["comment"]["body"],
        "second"
    );
}

#[test]
fn checks_create_update_suite_and_rerequest_fire() {
    let server = Server::start().unwrap();
    let repository = server
        .create_repository("admin", "tool", false, None, None, "main")
        .unwrap();
    repository.create_installation(7).unwrap();
    let token = app_token(&server, 7);
    let commit = repository
        .commit_file("main", Path::new("file.txt"), b"one\n", "Add file")
        .unwrap();
    let admin = Api::admin(&server);
    let app = Api::new(&server, Some(&token));
    let receiver = Receiver::start(200);
    subscribe(&admin, &receiver, &["check_run", "check_suite"]);
    let suite = app.post(
        "/repos/admin/tool/check-suites",
        json!({"head_sha": commit.sha()}),
    );
    assert_eq!(suite.status, 201, "{}", suite.body);
    let suite_id = suite.body["id"].as_i64().unwrap();
    let created = app.post(
        "/repos/admin/tool/check-runs",
        json!({
            "name": "tests",
            "head_sha": commit.sha(),
            "status": "in_progress",
        }),
    );
    assert_eq!(created.status, 201, "{}", created.body);
    let id = created.body["id"].as_i64().unwrap();
    let updated = app.patch(
        &format!("/repos/admin/tool/check-runs/{id}"),
        json!({"conclusion": "success"}),
    );
    assert_eq!(updated.status, 200, "{}", updated.body);
    assert_eq!(
        app.post(
            &format!("/repos/admin/tool/check-runs/{id}/rerequest"),
            json!({}),
        )
        .status,
        201
    );
    assert_eq!(
        app.post(
            &format!("/repos/admin/tool/check-suites/{suite_id}/rerequest"),
            json!({}),
        )
        .status,
        201
    );
    let deliveries = finished(&server, 6);
    assert_eq!(
        owned_action(&deliveries, "check_suite", "requested")["check_suite"]["head_sha"],
        commit.sha()
    );
    assert_eq!(
        owned_action(&deliveries, "check_run", "created")["check_run"]["name"],
        "tests"
    );
    assert_eq!(
        owned_action(&deliveries, "check_run", "completed")["check_run"]["conclusion"],
        "success"
    );
    assert_eq!(
        owned_action(&deliveries, "check_run", "rerequested")["action"],
        "rerequested"
    );
    assert_eq!(
        owned_action(&deliveries, "check_suite", "rerequested")["action"],
        "rerequested"
    );
}

#[test]
fn git_create_tag_and_update_ref_fire_create_and_push() {
    let server = Server::start().unwrap();
    let repository = server
        .create_repository("admin", "tool", false, None, None, "main")
        .unwrap();
    let first = repository
        .commit_file("main", Path::new("file.txt"), b"one\n", "one")
        .unwrap();
    let second = repository
        .commit_file("main", Path::new("file.txt"), b"two\n", "two")
        .unwrap();
    let admin = Api::admin(&server);
    let receiver = Receiver::start(200);
    subscribe(&admin, &receiver, &["create", "push"]);
    let tagged = admin.post(
        "/repos/admin/tool/git/tags",
        json!({
            "tag": "v1",
            "message": "release",
            "object": second.sha(),
            "type": "commit",
        }),
    );
    assert_eq!(tagged.status, 201, "{}", tagged.body);
    let moved = admin.patch(
        "/repos/admin/tool/git/refs/heads/main",
        json!({"sha": first.sha(), "force": true}),
    );
    assert_eq!(moved.status, 200, "{}", moved.body);
    let deliveries = finished(&server, 3);
    assert_eq!(deliveries[0].event, "ping");
    let created = deliveries
        .iter()
        .find(|delivery| delivery.event == "create")
        .unwrap()
        .payload();
    assert_eq!(created["ref"], "v1");
    assert_eq!(created["ref_type"], "tag");
    let pushed = deliveries
        .iter()
        .find(|delivery| delivery.event == "push")
        .unwrap()
        .payload();
    assert_eq!(pushed["ref"], "refs/heads/main");
    assert_eq!(pushed["before"], second.sha());
    assert_eq!(pushed["after"], first.sha());
}
