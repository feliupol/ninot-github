//! Reactions on issues, pull requests, comments and releases.

mod common;

use common::Api;
use ninot_github::Server;
use ninot_github::fixtures::{PullRequestSeed, ReviewCommentSeed};
use serde_json::{Value, json};

/// A public repository `admin/tool` with one commit.
fn repository(server: &Server) -> Api<'_> {
    let admin = Api::admin(server);
    let created = admin.post("/user/repos", json!({ "name": "tool", "auto_init": true }));
    assert_eq!(created.status, 201, "{}", created.body);
    admin
}

/// The reaction collections of one issue, one comment, one review comment and one release.
fn subjects(server: &Server, admin: &Api<'_>) -> Vec<String> {
    let issue = admin.post("/repos/admin/tool/issues", json!({ "title": "Bug" }));
    assert_eq!(issue.status, 201, "{}", issue.body);
    let comment = admin.post(
        "/repos/admin/tool/issues/1/comments",
        json!({ "body": "Seen" }),
    );
    assert_eq!(comment.status, 201, "{}", comment.body);
    let pull = server
        .repository("admin", "tool")
        .unwrap()
        .add_pull_request(2, PullRequestSeed::default())
        .unwrap();
    pull.add_review_comment(ReviewCommentSeed::default())
        .unwrap();
    let review_comments = admin.get("/repos/admin/tool/pulls/comments");
    let release = admin.post("/repos/admin/tool/releases", json!({ "tag_name": "v1" }));
    assert_eq!(release.status, 201, "{}", release.body);
    vec![
        "/repos/admin/tool/issues/1/reactions".to_owned(),
        "/repos/admin/tool/issues/2/reactions".to_owned(),
        format!(
            "/repos/admin/tool/issues/comments/{}/reactions",
            comment.body["id"]
        ),
        format!(
            "/repos/admin/tool/pulls/comments/{}/reactions",
            review_comments.body[0]["id"]
        ),
        format!(
            "/repos/admin/tool/releases/{}/reactions",
            release.body["id"]
        ),
    ]
}

#[test]
fn reactions_are_created_listed_and_deleted_on_every_subject() {
    let server = Server::start().unwrap();
    let admin = repository(&server);
    for path in subjects(&server, &admin) {
        let created = admin.post(&path, json!({ "content": "heart" }));
        assert_eq!(created.status, 201, "{path}: {}", created.body);
        assert_eq!(created.body["content"], "heart");
        assert_eq!(created.body["user"]["login"], "admin");
        let again = admin.post(&path, json!({ "content": "heart" }));
        assert_eq!(again.status, 200, "{path}: {}", again.body);
        assert_eq!(again.body["id"], created.body["id"]);
        let rocket = admin.post(&path, json!({ "content": "rocket" }));
        assert_eq!(rocket.status, 201, "{path}: {}", rocket.body);

        let listed = admin.get(&path);
        assert_eq!(listed.status, 200, "{path}: {}", listed.body);
        let contents: Vec<&Value> = listed
            .body
            .as_array()
            .unwrap()
            .iter()
            .map(|r| &r["content"])
            .collect();
        assert_eq!(contents, [&json!("heart"), &json!("rocket")], "{path}");
        let filtered = admin.get(&format!("{path}?content=rocket"));
        assert_eq!(filtered.body.as_array().unwrap().len(), 1, "{path}");

        let deleted = admin.delete(&format!("{path}/{}", created.body["id"]));
        assert_eq!(deleted.status, 204, "{path}: {}", deleted.body);
        let gone = admin.delete(&format!("{path}/{}", created.body["id"]));
        assert_eq!(gone.status, 404, "{path}");
        assert_eq!(admin.get(&path).body.as_array().unwrap().len(), 1, "{path}");
    }
}

#[test]
fn reactions_roll_up_on_the_reacted_object() {
    let server = Server::start().unwrap();
    let admin = repository(&server);
    let paths = subjects(&server, &admin);
    for path in &paths {
        assert_eq!(admin.post(path, json!({ "content": "+1" })).status, 201);
    }
    let reader = server.add_user("reader");
    let reader = Api::new(&server, Some(&reader.add_token(&["public_repo"])));
    assert_eq!(
        reader.post(&paths[0], json!({ "content": "+1" })).status,
        201
    );

    let issue = admin.get("/repos/admin/tool/issues/1");
    let rollup = &issue.body["reactions"];
    assert_eq!(rollup["+1"], 2, "{}", issue.body);
    assert_eq!(rollup["total_count"], 2);
    assert_eq!(rollup["heart"], 0);
    let comment = admin.get(paths[2].trim_end_matches("/reactions"));
    assert_eq!(comment.body["reactions"]["+1"], 1, "{}", comment.body);
    let review_comment = admin.get(paths[3].trim_end_matches("/reactions"));
    assert_eq!(
        review_comment.body["reactions"]["total_count"], 1,
        "{}",
        review_comment.body
    );
    let release = admin.get(paths[4].trim_end_matches("/reactions"));
    assert_eq!(release.body["reactions"]["+1"], 1, "{}", release.body);
}

#[test]
fn reactions_follow_github_validation_and_permissions() {
    let server = Server::start().unwrap();
    let admin = repository(&server);
    let paths = subjects(&server, &admin);

    let invalid = admin.post(&paths[0], json!({ "content": "thumbsup" }));
    assert_eq!(invalid.status, 422, "{}", invalid.body);
    // Releases accept fewer reactions than issues.
    let release_minus = admin.post(&paths[4], json!({ "content": "-1" }));
    assert_eq!(release_minus.status, 422, "{}", release_minus.body);
    let missing = admin.get("/repos/admin/tool/issues/99/reactions");
    assert_eq!(missing.status, 404);
    let missing_comment = admin.post(
        "/repos/admin/tool/issues/comments/999999/reactions",
        json!({ "content": "eyes" }),
    );
    assert_eq!(missing_comment.status, 404);

    let anonymous = Api::new(&server, None);
    assert_eq!(anonymous.get(&paths[0]).status, 200);
    assert_eq!(
        anonymous
            .post(&paths[0], json!({ "content": "eyes" }))
            .status,
        401
    );

    let created = admin.post(&paths[0], json!({ "content": "eyes" }));
    let other = server.add_user("other");
    let other = Api::new(&server, Some(&other.add_token(&["repo"])));
    let foreign = other.delete(&format!("{}/{}", paths[0], created.body["id"]));
    assert_eq!(foreign.status, 403, "{}", foreign.body);
    // A reaction is deleted through the subject it is on.
    let elsewhere = admin.delete(&format!("{}/{}", paths[1], created.body["id"]));
    assert_eq!(elsewhere.status, 404);

    let private = admin.post("/user/repos", json!({ "name": "secret", "private": true }));
    assert_eq!(private.status, 201);
    assert_eq!(
        admin
            .post("/repos/admin/secret/issues", json!({ "title": "x" }))
            .status,
        201
    );
    assert_eq!(
        other.get("/repos/admin/secret/issues/1/reactions").status,
        404
    );
}
