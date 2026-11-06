//! Webhook management and signed deliveries.

mod common;

use std::time::Duration;

use common::{Api, Receiver, TempDir, clone_url, commit_file, git_ok};
use hmac::{Hmac, Mac};
use ninot_github::Server;
use serde_json::{Value, json};
use sha2::Sha256;

const SECRET: &str = "s3cret";

fn verify(headers: &std::collections::HashMap<String, String>, body: &[u8]) {
    let mut mac = Hmac::<Sha256>::new_from_slice(SECRET.as_bytes()).unwrap();
    mac.update(body);
    let expected: String = mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    assert_eq!(headers["x-hub-signature-256"], format!("sha256={expected}"));
}

/// The keys of a JSON object, sorted.
fn keys(value: &Value) -> Vec<&str> {
    let mut keys: Vec<&str> = value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    keys
}

fn create_hook(admin: &Api, receiver: &Receiver, extra: Value) -> Value {
    let mut body =
        json!({ "config": { "url": receiver.url, "content_type": "json", "secret": SECRET } });
    body.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    let created = admin.post("/repos/admin/tool/hooks", body);
    assert_eq!(created.status, 201, "{}", created.body);
    created.body
}

#[test]
fn creating_a_hook_pings_it_with_a_signed_payload() {
    let server = Server::start().unwrap();
    let admin = Api::admin(&server);
    admin.post("/user/repos", json!({ "name": "tool" }));
    let receiver = Receiver::start(200);
    let hook = create_hook(&admin, &receiver, json!({}));
    assert_eq!(hook["config"]["secret"], "********");
    assert_eq!(hook["events"], json!(["push"]));

    let ping = receiver.next();
    assert_eq!(ping.headers["x-github-event"], "ping");
    assert_eq!(ping.headers["x-github-hook-id"], hook["id"].to_string());
    assert_eq!(ping.headers["content-type"], "application/json");
    verify(&ping.headers, &ping.body);
    let payload: Value = serde_json::from_slice(&ping.body).unwrap();
    assert_eq!(
        keys(&payload),
        ["hook", "hook_id", "repository", "sender", "zen"]
    );
    assert_eq!(payload["hook_id"], hook["id"]);
    assert_eq!(payload["hook"]["id"], hook["id"]);
    assert_eq!(payload["hook"]["config"]["url"], receiver.url);
    assert_eq!(payload["repository"]["full_name"], "admin/tool");
    assert_eq!(payload["repository"]["owner"]["login"], "admin");
    assert_eq!(payload["sender"]["login"], "admin");

    let listed = admin.get("/repos/admin/tool/hooks?per_page=100");
    assert_eq!(listed.body.as_array().unwrap().len(), 1);
    server.wait_for_deliveries(1, Duration::from_secs(5));
    let id = hook["id"].as_u64().unwrap();
    let deliveries = admin.get(&format!("/repos/admin/tool/hooks/{id}/deliveries"));
    assert_eq!(deliveries.body[0]["event"], "ping");
    assert_eq!(deliveries.body[0]["status_code"], 200);
    assert_eq!(
        admin.get(&format!("/repos/admin/tool/hooks/{id}")).body["last_response"]["code"],
        200
    );
    let pinged = admin.post(&format!("/repos/admin/tool/hooks/{id}/pings"), json!({}));
    assert_eq!(pinged.status, 204);
    let again: Value = serde_json::from_slice(&receiver.next().body).unwrap();
    assert_eq!(
        keys(&again),
        ["hook", "hook_id", "repository", "sender", "zen"]
    );
    assert_eq!(again["hook_id"], hook["id"]);
}

#[test]
fn pushes_deliver_one_event_per_changed_ref() {
    let server = Server::start().unwrap();
    let admin = Api::admin(&server);
    admin.post("/user/repos", json!({ "name": "tool" }));
    let receiver = Receiver::start(200);
    create_hook(&admin, &receiver, json!({}));
    receiver.next();

    let work = TempDir::new();
    git_ok(&work.0, &["init", "-q", "-b", "main", "w"]);
    let dir = work.join("w");
    commit_file(&dir, "a.txt", "a");
    commit_file(&dir, "b.txt", "b");
    let url = clone_url(&server, "admin", "tool", Some(server.admin_token()));
    git_ok(&dir, &["push", "-q", &url, "main", "main:rpc/inbox"]);
    let head = git_ok(&dir, &["rev-parse", "HEAD"]);

    let mut refs = Vec::new();
    for _ in 0..2 {
        let push = receiver.next();
        assert_eq!(push.headers["x-github-event"], "push");
        verify(&push.headers, &push.body);
        let payload: Value = serde_json::from_slice(&push.body).unwrap();
        // The push repository is an inline object without the full repository's counters.
        let repository = &payload["repository"];
        assert_eq!(repository["full_name"], "admin/tool");
        assert_eq!(repository["owner"]["login"], "admin");
        assert!(repository.get("network_count").is_none(), "{repository}");
        assert!(
            repository.get("subscribers_count").is_none(),
            "{repository}"
        );
        assert_eq!(payload["base_ref"], Value::Null);
        assert_eq!(payload["forced"], false);
        assert_eq!(payload["sender"]["login"], "admin");
        assert_eq!(payload["after"], head);
        assert_eq!(
            payload["before"],
            "0000000000000000000000000000000000000000"
        );
        assert_eq!(payload["created"], true);
        assert_eq!(payload["pusher"], json!({ "name": "admin", "email": null }));
        refs.push(payload["ref"].as_str().unwrap().to_owned());
        if payload["ref"] == "refs/heads/main" {
            let commits = payload["commits"].as_array().unwrap();
            assert_eq!(commits.len(), 2);
            assert_eq!(commits[1]["added"], json!(["b.txt"]));
            assert_eq!(commits[1]["removed"], json!([]));
            assert_eq!(commits[1]["modified"], json!([]));
            assert_eq!(commits[1]["distinct"], true);
            assert!(
                commits[1]["url"]
                    .as_str()
                    .unwrap()
                    .ends_with(&format!("/admin/tool/commit/{head}"))
            );
            assert_eq!(
                commits[1]["author"],
                json!({ "name": "Test", "email": "test@example.com" })
            );
            assert_eq!(payload["head_commit"], commits[1]);
            assert_eq!(payload["head_commit"]["id"], head);
        }
    }
    refs.sort();
    assert_eq!(refs, ["refs/heads/main", "refs/heads/rpc/inbox"]);

    git_ok(&dir, &["push", "-q", &url, ":rpc/inbox"]);
    let deleted: Value = serde_json::from_slice(&receiver.next().body).unwrap();
    assert_eq!(deleted["head_commit"], Value::Null);
    assert_eq!(deleted["ref"], "refs/heads/rpc/inbox");
    assert_eq!(deleted["deleted"], true);
    assert_eq!(deleted["commits"], json!([]));
}

#[test]
fn inactive_and_uninterested_hooks_receive_nothing() {
    let server = Server::start().unwrap();
    let admin = Api::admin(&server);
    admin.post("/user/repos", json!({ "name": "tool", "auto_init": true }));
    let inactive = Receiver::start(200);
    let hook = create_hook(&admin, &inactive, json!({ "active": false }));
    let issues_only = Receiver::start(200);
    create_hook(&admin, &issues_only, json!({ "events": ["issues"] }));
    issues_only.next();

    let work = TempDir::new();
    let url = clone_url(&server, "admin", "tool", Some(server.admin_token()));
    git_ok(&work.0, &["clone", "-q", &url, "c"]);
    commit_file(&work.join("c"), "x.txt", "x");
    git_ok(&work.join("c"), &["push", "-q", "origin", "main"]);
    // The ping of the second hook is the only delivery.
    server.wait_for_deliveries(1, Duration::from_secs(5));
    assert_eq!(server.deliveries().len(), 1);

    let id = hook["id"].as_u64().unwrap();
    assert_eq!(
        admin
            .patch(
                &format!("/repos/admin/tool/hooks/{id}"),
                json!({ "active": true })
            )
            .status,
        200
    );
    assert_eq!(
        admin
            .post(&format!("/repos/admin/tool/hooks/{id}/pings"), json!({}))
            .status,
        204
    );
    assert_eq!(inactive.next().headers["x-github-event"], "ping");
    assert_eq!(
        admin
            .delete(&format!("/repos/admin/tool/hooks/{id}"))
            .status,
        204
    );
    assert_eq!(
        admin.get(&format!("/repos/admin/tool/hooks/{id}")).status,
        404
    );
}

#[test]
fn form_hooks_and_failed_deliveries_are_recorded() {
    let server = Server::start().unwrap();
    let admin = Api::admin(&server);
    admin.post("/user/repos", json!({ "name": "tool" }));
    let failing = Receiver::start(500);
    admin.post(
        "/repos/admin/tool/hooks",
        json!({ "config": { "url": failing.url } }),
    );
    let ping = failing.next();
    assert_eq!(
        ping.headers["content-type"],
        "application/x-www-form-urlencoded"
    );
    assert!(!ping.headers.contains_key("x-hub-signature-256"));
    let deliveries = server.wait_for_deliveries(1, Duration::from_secs(5));
    assert_eq!(deliveries[0].status_code, Some(500));
    assert_eq!(deliveries[0].payload()["zen"], "Keep it logically awesome.");

    admin.post(
        "/repos/admin/tool/hooks",
        json!({ "config": { "url": "http://127.0.0.1:9/nothing" } }),
    );
    let deliveries = server.wait_for_deliveries(2, Duration::from_secs(15));
    assert!(deliveries[1].error.is_some());
}

#[test]
fn hooks_need_admin() {
    let server = Server::start().unwrap();
    server.add_user("bob");
    let bob = Api::new(&server, Some(&server.add_token("bob", &["repo"])));
    Api::admin(&server).post("/user/repos", json!({ "name": "tool" }));
    assert_eq!(bob.get("/repos/admin/tool/hooks").status, 404);
    assert_eq!(
        bob.post(
            "/repos/admin/tool/hooks",
            json!({ "config": { "url": "http://x" } })
        )
        .status,
        404
    );
    assert_eq!(
        Api::admin(&server)
            .post("/repos/admin/tool/hooks", json!({ "config": {} }))
            .status,
        422
    );
}

#[test]
fn pushing_more_than_three_tags_sends_no_tag_events() {
    let server = Server::start().unwrap();
    let admin = Api::admin(&server);
    admin.post("/user/repos", json!({ "name": "tool", "auto_init": true }));
    let receiver = Receiver::start(200);
    create_hook(&admin, &receiver, json!({}));
    receiver.next();

    let work = TempDir::new();
    let url = clone_url(&server, "admin", "tool", Some(server.admin_token()));
    git_ok(&work.0, &["clone", "-q", &url, "c"]);
    let clone = work.join("c");
    commit_file(&clone, "x.txt", "x");
    for tag in ["t1", "t2", "t3", "t4"] {
        git_ok(&clone, &["tag", tag]);
    }
    git_ok(&clone, &["push", "-q", "origin", "main", "--tags"]);
    let pushed = server.wait_for_deliveries(2, Duration::from_secs(5));
    let refs: Vec<Value> = pushed[1..]
        .iter()
        .map(|delivery| delivery.payload()["ref"].clone())
        .collect();
    assert_eq!(refs, [json!("refs/heads/main")]);

    for tag in ["t5", "t6", "t7"] {
        git_ok(&clone, &["tag", tag]);
    }
    git_ok(&clone, &["push", "-q", "origin", "--tags"]);
    let pushed = server.wait_for_deliveries(5, Duration::from_secs(5));
    let mut refs: Vec<String> = pushed[2..]
        .iter()
        .map(|delivery| delivery.payload()["ref"].as_str().unwrap().to_owned())
        .collect();
    refs.sort();
    assert_eq!(refs, ["refs/tags/t5", "refs/tags/t6", "refs/tags/t7"]);
}

#[test]
fn add_events_and_remove_events_edit_the_event_list() {
    let server = Server::start().unwrap();
    let admin = Api::admin(&server);
    admin.post("/user/repos", json!({ "name": "tool" }));
    let receiver = Receiver::start(200);
    let hook = create_hook(&admin, &receiver, json!({ "events": ["push", "issues"] }));
    let path = format!("/repos/admin/tool/hooks/{}", hook["id"]);

    let added = admin.patch(&path, json!({ "add_events": ["release", "push"] }));
    assert_eq!(added.status, 200, "{}", added.body);
    assert_eq!(added.body["events"], json!(["push", "issues", "release"]));
    let removed = admin.patch(&path, json!({ "remove_events": ["issues", "create"] }));
    assert_eq!(removed.body["events"], json!(["push", "release"]));
    assert_eq!(admin.get(&path).body["events"], json!(["push", "release"]));
    let replaced = admin.patch(
        &path,
        json!({ "events": ["issues"], "add_events": ["create"], "remove_events": ["issues"] }),
    );
    assert_eq!(replaced.body["events"], json!(["create"]));
}

#[test]
fn wrongly_typed_hook_fields_are_invalid_requests() {
    let server = Server::start().unwrap();
    let admin = Api::admin(&server);
    admin.post("/user/repos", json!({ "name": "tool" }));
    let refused = admin.post(
        "/repos/admin/tool/hooks",
        json!({ "config": { "url": "http://x" }, "active": "yes" }),
    );
    assert_eq!(refused.status, 422);
    assert_eq!(
        refused.body["message"],
        "Invalid request.\n\nFor 'properties/active', \"yes\" is not a boolean."
    );
    assert_eq!(
        admin
            .post(
                "/repos/admin/tool/hooks",
                json!({ "config": { "url": "http://x" }, "url": "http://x" })
            )
            .body["message"],
        "Invalid request.\n\n\"url\" is not a permitted key."
    );
    assert_eq!(admin.get("/repos/admin/tool/hooks").body, json!([]));
}

#[test]
fn failed_hook_update_changes_nothing() {
    let server = Server::start().unwrap();
    let admin = Api::admin(&server);
    admin.post("/user/repos", json!({ "name": "tool" }));
    let receiver = Receiver::start(200);
    let hook = create_hook(&admin, &receiver, json!({}));
    let path = format!("/repos/admin/tool/hooks/{}", hook["id"]);
    server.wait_for_deliveries(1, Duration::from_secs(10));
    let before = admin.get(&path).body;

    let refused = admin.patch(
        &path,
        json!({
            "config": { "url": "https://changed.example", "content_type": "xml" },
            "active": false,
        }),
    );
    assert_eq!(refused.status, 422, "{}", refused.body);
    assert_eq!(admin.get(&path).body, before);
}
