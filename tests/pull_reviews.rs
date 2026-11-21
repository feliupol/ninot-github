//! Pull request review lifecycle, review comments, merge checks and branch updates.

mod common;

use std::path::Path;
use std::time::Duration;

use common::{Api, Receiver};
use ninot_github::Server;
use serde_json::{Value, json};

const PULL: &str = "/repos/admin/project/pulls/1";

/// A repository with `feature` one commit ahead of `main`, a pull request for it, and two
/// non-admin users with tokens.
fn setup() -> (Server, String, String) {
    let server = Server::start().unwrap();
    server.add_user("bob");
    server.add_user("carol");
    let bob = server.user("bob").unwrap().add_token(&["repo"]);
    let carol = server.user("carol").unwrap().add_token(&["repo"]);
    let repository = server
        .add_repository("admin", "project", false, None, None, "main")
        .unwrap();
    let file = |branch: &str, path: &str| {
        repository
            .commit_file(branch, Path::new(path), b"x\n", "Change")
            .unwrap();
    };
    file("main", "README.md");
    file("feature", "change.txt");
    let created = Api::admin(&server).post(
        "/repos/admin/project/pulls",
        json!({"title": "Change", "head": "feature", "base": "main"}),
    );
    assert_eq!(created.status, 201, "{:?}", created.body);
    (server, bob, carol)
}

/// A direct review comment request; the pinned spec requires `commit_id` there.
fn direct(server: &Server, path: &str, line: i64, body: &str) -> Value {
    let head = Api::admin(server).get(PULL).body["head"]["sha"].clone();
    let mut request = comment_body(path, line, body);
    request["commit_id"] = head;
    request
}

fn comment_body(path: &str, line: i64, body: &str) -> Value {
    json!({"body": body, "path": path, "line": line, "side": "RIGHT"})
}

#[test]
fn pending_reviews_are_private_and_submit_or_dismiss_by_state() {
    let (server, bob_token, _) = setup();
    let admin = Api::admin(&server);
    let bob = Api::new(&server, Some(&bob_token));
    let reviews = format!("{PULL}/reviews");

    let pending = bob.post(
        &reviews,
        json!({"comments": [comment_body("change.txt", 1, "Why?")]}),
    );
    assert_eq!(pending.status, 200, "{:?}", pending.body);
    assert_eq!(pending.body["state"], "PENDING");
    let review = format!("{reviews}/{}", pending.body["id"]);

    assert_eq!(bob.get(&review).body["state"], "PENDING");
    assert_eq!(admin.get(&review).status, 404);
    assert_eq!(Api::new(&server, None).get(&review).status, 404);
    assert_eq!(bob.get(&format!("{reviews}/9999")).status, 404);

    assert_eq!(bob.put(&review, json!({})).status, 422);
    assert_eq!(admin.put(&review, json!({"body": "x"})).status, 404);
    let updated = bob.put(&review, json!({"body": "Needs work"}));
    assert_eq!(updated.status, 200, "{:?}", updated.body);
    assert_eq!(updated.body["body"], "Needs work");

    let inline = bob.get(&format!("{review}/comments"));
    assert_eq!(inline.status, 200, "{:?}", inline.body);
    assert_eq!(inline.body[0]["body"], "Why?");
    assert_eq!(inline.body[0]["pull_request_review_id"], pending.body["id"]);
    assert_eq!(admin.get(&format!("{review}/comments")).status, 404);
    assert_eq!(
        admin.get("/repos/admin/project/pulls/comments").body,
        json!([])
    );
    assert_eq!(
        bob.get("/repos/admin/project/pulls/comments")
            .body
            .as_array()
            .unwrap()
            .len(),
        1
    );

    let events = format!("{review}/events");
    assert_eq!(admin.post(&events, json!({"event": "COMMENT"})).status, 404);
    assert_eq!(bob.post(&events, json!({})).status, 422);
    assert_eq!(bob.post(&events, json!({"event": "BOGUS"})).status, 422);
    let submitted = bob.post(&events, json!({"event": "REQUEST_CHANGES"}));
    assert_eq!(submitted.status, 200, "{:?}", submitted.body);
    assert_eq!(submitted.body["state"], "CHANGES_REQUESTED");
    assert_eq!(submitted.body["body"], "Needs work");
    assert!(submitted.body["submitted_at"].is_string());
    assert_eq!(bob.post(&events, json!({"event": "COMMENT"})).status, 422);
    assert_eq!(admin.get(&review).body["state"], "CHANGES_REQUESTED");
    assert_eq!(bob.delete(&review).status, 422);

    let dismissals = format!("{review}/dismissals");
    assert_eq!(bob.put(&dismissals, json!({"message": "x"})).status, 403);
    assert_eq!(admin.put(&dismissals, json!({})).status, 422);
    let dismissed = admin.put(&dismissals, json!({"message": "Stale"}));
    assert_eq!(dismissed.status, 200, "{:?}", dismissed.body);
    assert_eq!(dismissed.body["state"], "DISMISSED");
    assert_eq!(
        admin.put(&dismissals, json!({"message": "again"})).status,
        422
    );

    let commented = bob.post(&reviews, json!({"body": "Fine", "event": "COMMENT"}));
    let commented = format!("{reviews}/{}/dismissals", commented.body["id"]);
    assert_eq!(admin.put(&commented, json!({"message": "x"})).status, 422);

    let own = admin.post(&reviews, json!({}));
    let own_events = format!("{reviews}/{}/events", own.body["id"]);
    assert_eq!(
        admin.post(&own_events, json!({"event": "APPROVE"})).status,
        422
    );
}

#[test]
fn pending_reviews_delete_with_their_comments() {
    let (server, bob_token, _) = setup();
    let admin = Api::admin(&server);
    let bob = Api::new(&server, Some(&bob_token));
    let pending = bob.post(
        &format!("{PULL}/reviews"),
        json!({"comments": [comment_body("change.txt", 1, "Nit")]}),
    );
    let review = format!("{PULL}/reviews/{}", pending.body["id"]);
    assert_eq!(admin.get(PULL).body["review_comments"], 1);
    assert_eq!(admin.delete(&review).status, 404);
    let deleted = bob.delete(&review);
    assert_eq!(deleted.status, 200, "{:?}", deleted.body);
    assert_eq!(deleted.body["id"], pending.body["id"]);
    assert_eq!(bob.get(&review).status, 404);
    assert_eq!(
        bob.get("/repos/admin/project/pulls/comments").body,
        json!([])
    );
    assert_eq!(admin.get(PULL).body["review_comments"], 0);
    let bad = bob.post(
        &format!("{PULL}/reviews"),
        json!({"comments": [{"body": "x", "path": "change.txt"}]}),
    );
    assert_eq!(bad.status, 422, "{:?}", bad.body);
}

#[test]
fn review_comments_create_reply_read_list_update_and_delete() {
    let (server, bob_token, carol_token) = setup();
    let admin = Api::admin(&server);
    let bob = Api::new(&server, Some(&bob_token));
    let carol = Api::new(&server, Some(&carol_token));
    let comments = format!("{PULL}/comments");

    let mut no_body = direct(&server, "change.txt", 1, "");
    assert_eq!(bob.post(&comments, no_body.clone()).status, 422);
    no_body["body"] = json!("x");
    no_body.as_object_mut().unwrap().remove("line");
    assert_eq!(bob.post(&comments, no_body).status, 422);
    let mut bad_side = direct(&server, "change.txt", 1, "x");
    bad_side["side"] = json!("UP");
    assert_eq!(bob.post(&comments, bad_side).status, 422);
    let mut stale = direct(&server, "change.txt", 1, "x");
    stale["commit_id"] = json!("0".repeat(40));
    assert_eq!(bob.post(&comments, stale).status, 422);
    assert_eq!(
        Api::new(&server, None)
            .post(&comments, direct(&server, "a", 1, "x"))
            .status,
        401
    );
    assert_eq!(
        bob.post(
            "/repos/admin/project/pulls/77/comments",
            direct(&server, "a", 1, "x")
        )
        .status,
        404
    );

    let created = bob.post(&comments, direct(&server, "change.txt", 1, "First"));
    assert_eq!(created.status, 201, "{:?}", created.body);
    let head = admin.get(PULL).body["head"]["sha"].clone();
    assert_eq!(created.body["commit_id"], head);
    assert_eq!(created.body["user"]["login"], "bob");
    assert_eq!(created.body["path"], "change.txt");
    assert_eq!(admin.get(PULL).body["review_comments"], 1);
    let id = created.body["id"].as_i64().unwrap();
    let one = format!("/repos/admin/project/pulls/comments/{id}");
    assert_eq!(admin.get(&one).body["body"], "First");
    assert_eq!(
        admin.get("/repos/admin/project/pulls/comments/9999").status,
        404
    );

    let reply = bob.post(
        &format!("{comments}/{id}/replies"),
        json!({"body": "Second"}),
    );
    assert_eq!(reply.status, 201, "{:?}", reply.body);
    assert_eq!(reply.body["in_reply_to_id"], id);
    assert_eq!(reply.body["path"], "change.txt");
    assert_eq!(
        bob.post(&format!("{comments}/{id}/replies"), json!({}))
            .status,
        422
    );
    assert_eq!(
        bob.post(&format!("{comments}/9999/replies"), json!({"body": "x"}))
            .status,
        404
    );
    let reply_id = reply.body["id"].as_i64().unwrap();
    assert_eq!(
        bob.post(
            &format!("{comments}/{reply_id}/replies"),
            json!({"body": "x"})
        )
        .status,
        422
    );
    let inline_reply = carol.post(&comments, {
        let mut request = direct(&server, "change.txt", 1, "Third");
        request["in_reply_to"] = json!(id);
        request
    });
    assert_eq!(inline_reply.status, 201, "{:?}", inline_reply.body);
    assert_eq!(inline_reply.body["in_reply_to_id"], id);

    let all = admin.get("/repos/admin/project/pulls/comments");
    assert_eq!(
        all.body
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["body"].clone())
            .collect::<Vec<_>>(),
        [json!("First"), json!("Second"), json!("Third")]
    );
    let newest = admin.get("/repos/admin/project/pulls/comments?direction=desc&per_page=1");
    assert_eq!(newest.body[0]["body"], "Third");
    assert!(newest.header("link").unwrap().contains("rel=\"next\""));
    assert_eq!(
        admin
            .get("/repos/admin/project/pulls/comments?sort=bogus")
            .status,
        422
    );
    assert_eq!(
        admin
            .get("/repos/admin/project/pulls/comments?since=2999-01-01T00:00:00Z")
            .body,
        json!([])
    );

    assert_eq!(bob.patch(&one, json!({})).status, 422);
    assert_eq!(carol.patch(&one, json!({"body": "Hijack"})).status, 403);
    let edited = bob.patch(&one, json!({"body": "Edited"}));
    assert_eq!(edited.status, 200, "{:?}", edited.body);
    assert_eq!(edited.body["body"], "Edited");
    assert_eq!(admin.patch(&one, json!({"body": "Admin"})).status, 200);
    assert_eq!(carol.delete(&one).status, 403);
    assert_eq!(bob.delete(&one).status, 204);
    assert_eq!(admin.get(&one).status, 404);
    assert_eq!(admin.delete(&one).status, 404);
    assert_eq!(admin.get(PULL).body["review_comments"], 2);
}

#[test]
fn review_submission_and_comment_creation_fire_webhooks() {
    let (server, bob_token, _) = setup();
    let admin = Api::admin(&server);
    let bob = Api::new(&server, Some(&bob_token));
    let receiver = Receiver::start(200);
    let hook = admin.post(
        "/repos/admin/project/hooks",
        json!({
            "config": {"url": receiver.url, "content_type": "json"},
            "events": ["pull_request_review", "pull_request_review_comment"],
        }),
    );
    assert_eq!(hook.status, 201, "{:?}", hook.body);

    let comment = bob.post(
        &format!("{PULL}/comments"),
        direct(&server, "change.txt", 1, "Look"),
    );
    assert_eq!(comment.status, 201, "{:?}", comment.body);
    let one = format!("/repos/admin/project/pulls/comments/{}", comment.body["id"]);
    let reply = bob.post(
        &format!("{PULL}/comments/{}/replies", comment.body["id"]),
        json!({"body": "Again"}),
    );
    assert_eq!(reply.status, 201, "{:?}", reply.body);
    bob.patch(&one, json!({"body": "Look again"}));
    let pending = bob.post(
        &format!("{PULL}/reviews"),
        json!({"body": "Overall", "comments": [comment_body("change.txt", 1, "Inline")]}),
    );
    let review = format!("{PULL}/reviews/{}", pending.body["id"]);
    assert_eq!(
        bob.post(
            &format!("{review}/events"),
            json!({"event": "REQUEST_CHANGES"})
        )
        .status,
        200
    );

    // Two direct comments, one submitted review and its inline comment; the pending review
    // creation itself, edits and the dismissal queue nothing.
    let deliveries = server.wait_for_deliveries(5, Duration::from_secs(5));
    let seen = deliveries
        .iter()
        .map(|delivery| {
            assert!(delivery.error.is_none(), "{:?}", delivery.error);
            format!(
                "{}:{}",
                delivery.event,
                delivery.payload()["action"].as_str().unwrap_or("none")
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        seen.iter()
            .filter(|name| *name == "pull_request_review_comment:created")
            .count(),
        3,
        "{seen:?}"
    );
    assert!(
        seen.iter()
            .any(|name| name == "pull_request_review:submitted"),
        "{seen:?}"
    );
    let submitted = deliveries
        .iter()
        .find(|delivery| delivery.event == "pull_request_review")
        .unwrap()
        .payload();
    assert_eq!(submitted["review"]["body"], "Overall");
    assert_eq!(submitted["sender"]["login"], "bob");
}

#[test]
fn merge_state_reads_back_through_check_if_merged() {
    let (server, bob_token, _) = setup();
    let admin = Api::admin(&server);
    let bob = Api::new(&server, Some(&bob_token));
    let merge = format!("{PULL}/merge");
    assert_eq!(bob.get(&merge).status, 404);
    assert_eq!(bob.get("/repos/admin/project/pulls/77/merge").status, 404);
    assert_eq!(admin.put(&merge, json!({})).status, 200);
    assert_eq!(bob.get(&merge).status, 204);
}

#[test]
fn update_branch_merges_the_base_into_the_head() {
    let (server, bob_token, _) = setup();
    let repository = server.repository("admin", "project").unwrap();
    let admin = Api::admin(&server);
    let bob = Api::new(&server, Some(&bob_token));
    let update = format!("{PULL}/update-branch");
    assert_eq!(
        admin.put(&update, json!({})).status,
        422,
        "nothing new on the base"
    );

    repository
        .commit_file("main", Path::new("base.txt"), b"base\n", "Advance base")
        .unwrap();
    let before = admin.get(PULL).body["head"]["sha"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(bob.put(&update, json!({})).status, 403);
    assert_eq!(
        admin
            .put(&update, json!({"expected_head_sha": "0".repeat(40)}))
            .status,
        422
    );
    let accepted = admin.put(&update, json!({"expected_head_sha": before}));
    assert_eq!(accepted.status, 202, "{:?}", accepted.body);
    assert!(accepted.body["message"].is_string());
    let after = admin.get(PULL).body["head"]["sha"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(before, after);
    let commit = admin.get(&format!("/repos/admin/project/commits/{after}"));
    assert_eq!(commit.body["parents"].as_array().unwrap().len(), 2);
    assert_eq!(admin.put(&update, json!({})).status, 422);
    assert_eq!(
        admin
            .put("/repos/admin/project/pulls/77/update-branch", json!({}))
            .status,
        404
    );
}

#[test]
fn update_branch_refuses_conflicts_and_closed_pull_requests() {
    let (server, _, _) = setup();
    let repository = server.repository("admin", "project").unwrap();
    let admin = Api::admin(&server);
    let update = format!("{PULL}/update-branch");
    repository
        .commit_file("main", Path::new("change.txt"), b"base side\n", "Conflict")
        .unwrap();
    let conflict = admin.put(&update, json!({}));
    assert_eq!(conflict.status, 422, "{:?}", conflict.body);
    assert!(
        conflict.body["message"]
            .as_str()
            .unwrap()
            .contains("conflict")
    );
    assert_eq!(admin.patch(PULL, json!({"state": "closed"})).status, 200);
    assert_eq!(admin.put(&update, json!({})).status, 422);
}
