//! Git blobs, trees, commits and tags over the Git Database REST API.

mod common;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use common::{Api, TempDir, clone_url, commit_file, git_ok};
use ninot_github::Server;
use serde_json::json;

/// Creates `admin/tool` and pushes `main` with `README.md`.
fn pushed(server: &Server) -> (TempDir, String) {
    let admin = Api::admin(server);
    assert_eq!(
        admin.post("/user/repos", json!({ "name": "tool" })).status,
        201
    );
    let work = TempDir::new();
    git_ok(&work.0, &["init", "-q", "-b", "main", "w"]);
    let dir = work.join("w");
    commit_file(&dir, "README.md", "hello\n");
    let url = clone_url(server, "admin", "tool", Some(server.admin_token()));
    git_ok(&dir, &["push", "-q", &url, "main"]);
    let head = git_ok(&dir, &["rev-parse", "HEAD"]);
    (work, head)
}

fn decoded_blob(content: &str) -> String {
    let encoded: String = content.split_whitespace().collect();
    String::from_utf8(STANDARD.decode(encoded).unwrap()).unwrap()
}

#[test]
fn create_and_get_blob() {
    let server = Server::start().unwrap();
    let (_work, _) = pushed(&server);
    let admin = Api::admin(&server);
    let created = admin.post(
        "/repos/admin/tool/git/blobs",
        json!({ "content": "hello from a blob\n", "encoding": "utf-8" }),
    );
    assert_eq!(created.status, 201, "{}", created.body);
    let sha = created.body["sha"].as_str().unwrap();
    assert_eq!(
        created.body["url"].as_str().unwrap(),
        format!("{}/repos/admin/tool/git/blobs/{sha}", server.url())
    );
    assert_eq!(
        created.header("location").unwrap(),
        created.body["url"].as_str().unwrap()
    );

    let fetched = admin.get(&format!("/repos/admin/tool/git/blobs/{sha}"));
    assert_eq!(fetched.status, 200, "{}", fetched.body);
    assert_eq!(fetched.body["sha"], sha);
    assert_eq!(fetched.body["encoding"], "base64");
    assert_eq!(
        decoded_blob(fetched.body["content"].as_str().unwrap()),
        "hello from a blob\n"
    );
    assert_eq!(fetched.body["size"], 18);
}

#[test]
fn create_blob_from_base64_and_reject_bad_encoding() {
    let server = Server::start().unwrap();
    let (_work, _) = pushed(&server);
    let admin = Api::admin(&server);
    let created = admin.post(
        "/repos/admin/tool/git/blobs",
        json!({ "content": STANDARD.encode("encoded\n"), "encoding": "base64" }),
    );
    assert_eq!(created.status, 201, "{}", created.body);
    let sha = created.body["sha"].as_str().unwrap();
    let fetched = admin.get(&format!("/repos/admin/tool/git/blobs/{sha}"));
    assert_eq!(
        decoded_blob(fetched.body["content"].as_str().unwrap()),
        "encoded\n"
    );

    assert_eq!(
        admin
            .post(
                "/repos/admin/tool/git/blobs",
                json!({ "content": "x", "encoding": "latin1" }),
            )
            .status,
        422
    );
    assert_eq!(
        admin
            .post(
                "/repos/admin/tool/git/blobs",
                json!({ "content": "not base64!", "encoding": "base64" }),
            )
            .status,
        422
    );
}

#[test]
fn get_blob_not_found_and_empty_repository() {
    let server = Server::start().unwrap();
    let admin = Api::admin(&server);
    admin.post("/user/repos", json!({ "name": "empty" }));
    assert_eq!(
        admin
            .get("/repos/admin/empty/git/blobs/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
            .status,
        409
    );

    let (_work, _) = pushed(&server);
    assert_eq!(
        admin
            .get("/repos/admin/tool/git/blobs/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
            .status,
        404
    );
    assert_eq!(admin.get("/repos/admin/tool/git/blobs/nope").status, 422);
}

#[test]
fn create_blob_requires_push() {
    let server = Server::start().unwrap();
    let (_work, _) = pushed(&server);
    server.add_user("reader");
    let reader = Api::new(
        &server,
        Some(&server.user("reader").unwrap().add_token(&["repo"])),
    );
    assert_eq!(
        reader
            .post("/repos/admin/tool/git/blobs", json!({ "content": "x" }),)
            .status,
        403
    );
}

#[test]
fn blobs_pushed_with_git_are_readable() {
    let server = Server::start().unwrap();
    let (_work, _) = pushed(&server);
    let admin = Api::admin(&server);
    let file = admin.get("/repos/admin/tool/contents/README.md");
    let sha = file.body["sha"].as_str().unwrap();
    let blob = admin.get(&format!("/repos/admin/tool/git/blobs/{sha}"));
    assert_eq!(blob.status, 200, "{}", blob.body);
    assert_eq!(
        decoded_blob(blob.body["content"].as_str().unwrap()),
        "hello\n"
    );
}

#[test]
fn create_and_get_tree() {
    let server = Server::start().unwrap();
    let (_work, _) = pushed(&server);
    let admin = Api::admin(&server);
    let blob = admin.post(
        "/repos/admin/tool/git/blobs",
        json!({ "content": "new file\n" }),
    );
    let blob_sha = blob.body["sha"].as_str().unwrap();
    let created = admin.post(
        "/repos/admin/tool/git/trees",
        json!({
            "tree": [{
                "path": "new.txt",
                "mode": "100644",
                "type": "blob",
                "sha": blob_sha,
            }]
        }),
    );
    assert_eq!(created.status, 201, "{}", created.body);
    let tree_sha = created.body["sha"].as_str().unwrap();
    assert_eq!(created.body["truncated"], false);
    assert_eq!(created.body["tree"][0]["path"], "new.txt");
    assert_eq!(created.body["tree"][0]["sha"], blob_sha);
    assert_eq!(created.body["tree"][0]["type"], "blob");
    assert_eq!(created.body["tree"][0]["mode"], "100644");

    let fetched = admin.get(&format!("/repos/admin/tool/git/trees/{tree_sha}"));
    assert_eq!(fetched.status, 200, "{}", fetched.body);
    assert_eq!(fetched.body["sha"], tree_sha);
    assert_eq!(fetched.body["tree"].as_array().unwrap().len(), 1);
}

#[test]
fn create_tree_from_base_tree_with_content_and_nested_path() {
    let server = Server::start().unwrap();
    let (_work, head) = pushed(&server);
    let admin = Api::admin(&server);
    let commit = admin.get(&format!("/repos/admin/tool/git/commits/{head}"));
    assert_eq!(commit.status, 200, "{}", commit.body);
    let base_tree = commit.body["tree"]["sha"].as_str().unwrap();
    let created = admin.post(
        "/repos/admin/tool/git/trees",
        json!({
            "base_tree": base_tree,
            "tree": [{
                "path": "dir/nested.txt",
                "mode": "100644",
                "type": "blob",
                "content": "nested\n",
            }]
        }),
    );
    assert_eq!(created.status, 201, "{}", created.body);
    let names: Vec<&str> = created.body["tree"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["path"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"README.md"), "{names:?}");
    assert!(names.contains(&"dir"), "{names:?}");

    let tree_sha = created.body["tree"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["path"] == "dir")
        .unwrap()["sha"]
        .as_str()
        .unwrap();
    let recursive = admin.get(&format!(
        "/repos/admin/tool/git/trees/{tree_sha}?recursive=1"
    ));
    let paths: Vec<&str> = recursive.body["tree"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["path"].as_str().unwrap())
        .collect();
    assert!(paths.contains(&"nested.txt"), "{paths:?}");
}

#[test]
fn create_tree_deletes_with_null_sha_and_rejects_missing_objects() {
    let server = Server::start().unwrap();
    let (_work, head) = pushed(&server);
    let admin = Api::admin(&server);
    let base_tree = admin
        .get(&format!("/repos/admin/tool/git/commits/{head}"))
        .body["tree"]["sha"]
        .as_str()
        .unwrap()
        .to_owned();
    let deleted = admin.post(
        "/repos/admin/tool/git/trees",
        json!({
            "base_tree": base_tree,
            "tree": [{
                "path": "README.md",
                "mode": "100644",
                "type": "blob",
                "sha": null,
            }]
        }),
    );
    assert_eq!(deleted.status, 201, "{}", deleted.body);
    let names: Vec<&str> = deleted.body["tree"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["path"].as_str().unwrap())
        .collect();
    assert!(!names.contains(&"README.md"), "{names:?}");

    let missing = admin.post(
        "/repos/admin/tool/git/trees",
        json!({
            "tree": [{
                "path": "gone.txt",
                "mode": "100644",
                "type": "blob",
                "sha": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            }]
        }),
    );
    assert_eq!(missing.status, 422, "{}", missing.body);

    let both = admin.post(
        "/repos/admin/tool/git/trees",
        json!({
            "tree": [{
                "path": "both.txt",
                "mode": "100644",
                "type": "blob",
                "sha": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "content": "x",
            }]
        }),
    );
    assert_eq!(both.status, 422, "{}", both.body);
}

#[test]
fn get_tree_not_found_and_empty_repository() {
    let server = Server::start().unwrap();
    let admin = Api::admin(&server);
    admin.post("/user/repos", json!({ "name": "empty" }));
    assert_eq!(
        admin
            .get("/repos/admin/empty/git/trees/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
            .status,
        409
    );
    let (_work, _) = pushed(&server);
    assert_eq!(
        admin
            .get("/repos/admin/tool/git/trees/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
            .status,
        404
    );
}

#[test]
fn create_and_get_commit() {
    let server = Server::start().unwrap();
    let (_work, head) = pushed(&server);
    let admin = Api::admin(&server);
    let parent = admin.get(&format!("/repos/admin/tool/git/commits/{head}"));
    assert_eq!(parent.status, 200, "{}", parent.body);
    assert_eq!(parent.body["sha"], head);
    assert_eq!(parent.body["message"], "Add README.md");
    let tree = parent.body["tree"]["sha"].as_str().unwrap();

    let created = admin.post(
        "/repos/admin/tool/git/commits",
        json!({
            "message": "Empty change",
            "tree": tree,
            "parents": [head],
            "author": {
                "name": "Mona",
                "email": "mona@example.com",
                "date": "2026-01-05T13:13:22+05:00",
            },
        }),
    );
    assert_eq!(created.status, 201, "{}", created.body);
    let sha = created.body["sha"].as_str().unwrap();
    assert_eq!(created.body["message"], "Empty change");
    assert_eq!(created.body["author"]["name"], "Mona");
    assert_eq!(created.body["author"]["date"], "2026-01-05T08:13:22Z");
    assert_eq!(created.body["parents"][0]["sha"], head);
    assert_eq!(created.body["verification"]["reason"], "unsigned");

    let fetched = admin.get(&format!("/repos/admin/tool/git/commits/{sha}"));
    assert_eq!(fetched.status, 200, "{}", fetched.body);
    assert_eq!(fetched.body["sha"], sha);
    assert_eq!(fetched.body["committer"]["name"], "Mona");
}

#[test]
fn create_commit_rejects_missing_objects() {
    let server = Server::start().unwrap();
    let (_work, head) = pushed(&server);
    let admin = Api::admin(&server);
    let missing_tree = admin.post(
        "/repos/admin/tool/git/commits",
        json!({
            "message": "nope",
            "tree": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "parents": [head],
        }),
    );
    assert_eq!(missing_tree.status, 422, "{}", missing_tree.body);

    let tree = admin
        .get(&format!("/repos/admin/tool/git/commits/{head}"))
        .body["tree"]["sha"]
        .as_str()
        .unwrap()
        .to_owned();
    let missing_parent = admin.post(
        "/repos/admin/tool/git/commits",
        json!({
            "message": "nope",
            "tree": tree,
            "parents": ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"],
        }),
    );
    assert_eq!(missing_parent.status, 422, "{}", missing_parent.body);
}

#[test]
fn get_commit_not_found_and_empty_repository() {
    let server = Server::start().unwrap();
    let admin = Api::admin(&server);
    admin.post("/user/repos", json!({ "name": "empty" }));
    assert_eq!(
        admin
            .get("/repos/admin/empty/git/commits/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
            .status,
        409
    );
    let (_work, _) = pushed(&server);
    assert_eq!(
        admin
            .get("/repos/admin/tool/git/commits/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
            .status,
        404
    );
}

#[test]
fn create_and_get_tag() {
    let server = Server::start().unwrap();
    let (_work, head) = pushed(&server);
    let admin = Api::admin(&server);
    let created = admin.post(
        "/repos/admin/tool/git/tags",
        json!({
            "tag": "v0.0.1",
            "message": "initial version",
            "object": head,
            "type": "commit",
            "tagger": {
                "name": "Mona",
                "email": "mona@example.com",
                "date": "2011-06-17T14:53:35-07:00",
            },
        }),
    );
    assert_eq!(created.status, 201, "{}", created.body);
    let sha = created.body["sha"].as_str().unwrap();
    assert_eq!(created.body["tag"], "v0.0.1");
    assert_eq!(created.body["message"], "initial version");
    assert_eq!(created.body["object"]["sha"], head);
    assert_eq!(created.body["object"]["type"], "commit");
    assert_eq!(created.body["tagger"]["name"], "Mona");
    assert_eq!(created.body["verification"]["reason"], "unsigned");

    let fetched = admin.get(&format!("/repos/admin/tool/git/tags/{sha}"));
    assert_eq!(fetched.status, 200, "{}", fetched.body);
    assert_eq!(fetched.body["sha"], sha);
    assert_eq!(fetched.body["tag"], "v0.0.1");

    let reference = admin.post(
        "/repos/admin/tool/git/refs",
        json!({ "ref": "refs/tags/v0.0.1", "sha": sha }),
    );
    assert_eq!(reference.status, 201, "{}", reference.body);
    assert_eq!(reference.body["object"]["type"], "tag");
    assert_eq!(reference.body["object"]["sha"], sha);
}

#[test]
fn get_tag_not_found_and_create_tag_missing_object() {
    let server = Server::start().unwrap();
    let (_work, _) = pushed(&server);
    let admin = Api::admin(&server);
    assert_eq!(
        admin
            .get("/repos/admin/tool/git/tags/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
            .status,
        404
    );
    let missing = admin.post(
        "/repos/admin/tool/git/tags",
        json!({
            "tag": "v-missing",
            "message": "nope",
            "object": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "type": "commit",
        }),
    );
    assert_eq!(missing.status, 422, "{}", missing.body);

    admin.post("/user/repos", json!({ "name": "empty" }));
    assert_eq!(
        admin
            .get("/repos/admin/empty/git/tags/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
            .status,
        409
    );
}

#[test]
fn blob_tree_commit_ref_then_clone() {
    let server = Server::start().unwrap();
    let (work, head) = pushed(&server);
    let admin = Api::admin(&server);
    let blob = admin.post(
        "/repos/admin/tool/git/blobs",
        json!({ "content": "from the git database\n" }),
    );
    assert_eq!(blob.status, 201, "{}", blob.body);
    let blob_sha = blob.body["sha"].as_str().unwrap();
    let base_tree = admin
        .get(&format!("/repos/admin/tool/git/commits/{head}"))
        .body["tree"]["sha"]
        .as_str()
        .unwrap()
        .to_owned();
    let tree = admin.post(
        "/repos/admin/tool/git/trees",
        json!({
            "base_tree": base_tree,
            "tree": [{
                "path": "bot.txt",
                "mode": "100644",
                "type": "blob",
                "sha": blob_sha,
            }]
        }),
    );
    assert_eq!(tree.status, 201, "{}", tree.body);
    let tree_sha = tree.body["sha"].as_str().unwrap();
    let commit = admin.post(
        "/repos/admin/tool/git/commits",
        json!({
            "message": "Add bot.txt",
            "tree": tree_sha,
            "parents": [head],
        }),
    );
    assert_eq!(commit.status, 201, "{}", commit.body);
    let commit_sha = commit.body["sha"].as_str().unwrap();
    let updated = admin.patch(
        "/repos/admin/tool/git/refs/heads/main",
        json!({ "sha": commit_sha }),
    );
    assert_eq!(updated.status, 200, "{}", updated.body);
    assert_eq!(updated.body["object"]["sha"], commit_sha);

    let dir = work.join("w");
    let url = clone_url(&server, "admin", "tool", Some(server.admin_token()));
    git_ok(&dir, &["fetch", "-q", &url, "main"]);
    assert_eq!(git_ok(&dir, &["rev-parse", "FETCH_HEAD"]), commit_sha);
    assert_eq!(
        git_ok(&dir, &["cat-file", "-p", blob_sha]),
        "from the git database"
    );
    git_ok(&work.0, &["clone", "-q", &url, "clone"]);
    assert_eq!(
        std::fs::read_to_string(work.join("clone").join("bot.txt")).unwrap(),
        "from the git database\n"
    );
}

#[test]
fn create_tree_and_commit_require_push() {
    let server = Server::start().unwrap();
    let (_work, head) = pushed(&server);
    server.add_user("reader");
    let reader = Api::new(
        &server,
        Some(&server.user("reader").unwrap().add_token(&["repo"])),
    );
    assert_eq!(
        reader
            .post("/repos/admin/tool/git/trees", json!({ "tree": [] }),)
            .status,
        403
    );
    assert_eq!(
        reader
            .post(
                "/repos/admin/tool/git/commits",
                json!({ "message": "x", "tree": head }),
            )
            .status,
        403
    );
    assert_eq!(
        reader
            .post(
                "/repos/admin/tool/git/tags",
                json!({
                    "tag": "v",
                    "message": "x",
                    "object": head,
                    "type": "commit",
                }),
            )
            .status,
        403
    );
}
