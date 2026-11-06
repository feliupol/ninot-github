//! Releases, tags and asset upload and download.

mod common;

use common::{Api, TempDir, clone_url, git_ok};
use ninot_github::Server;
use reqwest::blocking::Client;
use serde_json::json;
use sha2::{Digest, Sha256};

fn repo_with_commit(server: &Server) -> Api<'_> {
    let admin = Api::admin(server);
    assert_eq!(
        admin
            .post("/user/repos", json!({ "name": "tool", "auto_init": true }))
            .status,
        201
    );
    admin
}

fn upload(
    server: &Server,
    upload_url: &str,
    name: &str,
    bytes: &[u8],
) -> reqwest::blocking::Response {
    let url = upload_url.split('{').next().unwrap();
    Client::new()
        .post(format!("{url}?name={name}"))
        .header("Authorization", format!("token {}", server.admin_token()))
        .header("Content-Type", "application/octet-stream")
        .body(bytes.to_vec())
        .send()
        .unwrap()
}

#[test]
fn release_creates_its_tag_and_is_found_by_tag_and_latest() {
    let server = Server::start().unwrap();
    let admin = repo_with_commit(&server);
    let created = admin.post(
        "/repos/admin/tool/releases",
        json!({ "tag_name": "v1", "name": "One" }),
    );
    assert_eq!(created.status, 201);
    assert_eq!(created.body["tag_name"], "v1");
    assert_eq!(created.body["target_commitish"], "main");
    assert!(created.body["published_at"].is_string());

    let tag = admin.get("/repos/admin/tool/git/ref/tags/v1");
    assert_eq!(tag.status, 200);
    assert_eq!(
        tag.body["object"]["sha"],
        admin.get("/repos/admin/tool/commits/main").body["sha"]
    );

    admin.post(
        "/repos/admin/tool/releases",
        json!({ "tag_name": "v2-rc", "prerelease": true }),
    );
    assert_eq!(
        admin.get("/repos/admin/tool/releases/latest").body["tag_name"],
        "v1"
    );
    assert_eq!(
        admin.get("/repos/admin/tool/releases/tags/v2-rc").body["prerelease"],
        true
    );
    assert_eq!(
        admin
            .post("/repos/admin/tool/releases", json!({ "tag_name": "v1" }))
            .status,
        422
    );
    let tags: Vec<String> = admin
        .get("/repos/admin/tool/releases")
        .body
        .as_array()
        .unwrap()
        .iter()
        .map(|release| release["tag_name"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(tags, ["v2-rc", "v1"]);
}

#[test]
fn releases_need_commits_and_drafts_stay_private() {
    let server = Server::start().unwrap();
    server.add_user("bob");
    let bob = Api::new(&server, Some(&server.add_token("bob", &["repo"])));
    let admin = Api::admin(&server);
    admin.post("/user/repos", json!({ "name": "empty" }));
    assert_eq!(
        admin
            .post("/repos/admin/empty/releases", json!({ "tag_name": "v1" }))
            .status,
        422
    );

    admin.post("/user/repos", json!({ "name": "tool", "auto_init": true }));
    let draft = admin.post(
        "/repos/admin/tool/releases",
        json!({ "tag_name": "v1", "draft": true }),
    );
    assert!(draft.body["published_at"].is_null());
    assert_eq!(admin.get("/repos/admin/tool/git/ref/tags/v1").status, 404);
    assert_eq!(bob.get("/repos/admin/tool/releases").body, json!([]));
    assert_eq!(bob.get("/repos/admin/tool/releases/latest").status, 404);
    assert_eq!(
        bob.post("/repos/admin/tool/releases", json!({ "tag_name": "v9" }))
            .status,
        403
    );

    let id = draft.body["id"].as_u64().unwrap();
    let published = admin.patch(
        &format!("/repos/admin/tool/releases/{id}"),
        json!({ "draft": false }),
    );
    assert!(published.body["published_at"].is_string());
    assert_eq!(bob.get("/repos/admin/tool/releases/latest").body["id"], id);
    assert_eq!(admin.get("/repos/admin/tool/git/ref/tags/v1").status, 200);
    assert_eq!(
        admin
            .delete(&format!("/repos/admin/tool/releases/{id}"))
            .status,
        204
    );
    assert_eq!(bob.get("/repos/admin/tool/releases/latest").status, 404);
}

#[test]
fn assets_upload_download_and_digest() {
    let server = Server::start().unwrap();
    let admin = repo_with_commit(&server);
    let release = admin.post("/repos/admin/tool/releases", json!({ "tag_name": "v1" }));
    let upload_url = release.body["upload_url"].as_str().unwrap();
    assert!(
        upload_url.starts_with(&format!("{}/api/uploads/", server.url())),
        "{upload_url}"
    );

    let bytes = b"\x7fELF binary".to_vec();
    let uploaded = upload(&server, upload_url, "tool-linux", &bytes);
    assert_eq!(uploaded.status(), 201);
    let asset: serde_json::Value = uploaded.json().unwrap();
    assert_eq!(
        asset["digest"],
        format!("sha256:{:x}", Sha256::digest(&bytes))
    );
    assert_eq!(asset["size"], bytes.len());
    assert_eq!(
        upload(&server, upload_url, "tool-linux", &bytes).status(),
        422
    );

    let listed = admin.get("/repos/admin/tool/releases/tags/v1");
    assert_eq!(listed.body["assets"][0]["name"], "tool-linux");

    let id = asset["id"].as_u64().unwrap();
    let downloaded = Client::new()
        .get(format!(
            "{}/repos/admin/tool/releases/assets/{id}",
            server.url()
        ))
        .header("Accept", "application/octet-stream")
        .header("Authorization", format!("token {}", server.admin_token()))
        .send()
        .unwrap();
    assert_eq!(downloaded.bytes().unwrap().to_vec(), bytes);

    let browser_url = asset["browser_download_url"].as_str().unwrap();
    assert_eq!(
        Client::new()
            .get(browser_url)
            .send()
            .unwrap()
            .bytes()
            .unwrap()
            .to_vec(),
        bytes
    );

    assert_eq!(
        admin
            .delete(&format!("/repos/admin/tool/releases/assets/{id}"))
            .status,
        204
    );
    assert_eq!(
        admin
            .get(&format!("/repos/admin/tool/releases/assets/{id}"))
            .status,
        404
    );
}

#[test]
fn release_can_target_a_pushed_branch() {
    let server = Server::start().unwrap();
    let admin = repo_with_commit(&server);
    let work = TempDir::new();
    let url = clone_url(&server, "admin", "tool", Some(server.admin_token()));
    git_ok(&work.0, &["clone", "-q", &url, "c"]);
    let clone = work.join("c");
    git_ok(&clone, &["checkout", "-q", "-b", "next"]);
    common::commit_file(&clone, "next.txt", "n");
    git_ok(&clone, &["push", "-q", "origin", "next"]);
    let head = git_ok(&clone, &["rev-parse", "HEAD"]);
    admin.post(
        "/repos/admin/tool/releases",
        json!({ "tag_name": "v2", "target_commitish": "next" }),
    );
    assert_eq!(
        admin.get("/repos/admin/tool/git/ref/tags/v2").body["object"]["sha"],
        head
    );
}

#[test]
fn wrongly_typed_release_fields_are_invalid_requests() {
    let server = Server::start().unwrap();
    let admin = repo_with_commit(&server);
    let refused = admin.post(
        "/repos/admin/tool/releases",
        json!({ "tag_name": "v1", "draft": "yes" }),
    );
    assert_eq!(refused.status, 422);
    assert_eq!(
        refused.body,
        json!({
            "message": "Invalid request.\n\nFor 'properties/draft', \"yes\" is not a boolean.",
            "documentation_url": "https://docs.github.com/rest/releases/releases#create-a-release",
            "status": "422",
        })
    );
    assert_eq!(
        admin
            .post("/repos/admin/tool/releases", json!({ "draft": "yes" }))
            .body["message"],
        "Invalid request.\n\nFor 'properties/draft', \"yes\" is not a boolean.\n\"tag_name\" wasn't supplied."
    );
    assert_eq!(
        admin
            .post(
                "/repos/admin/tool/releases",
                json!({ "tag_name": "v1", "make_latest": "maybe" })
            )
            .body["message"],
        "Invalid request.\n\nmaybe is not a member of [\"true\", \"false\", \"legacy\"]."
    );
    assert_eq!(admin.get("/repos/admin/tool/releases").body, json!([]));
}
