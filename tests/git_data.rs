//! Refs, commits, branches and contents after pushing with the real git client.

mod common;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use common::{Api, TempDir, clone_url, commit_file, git_ok};
use ninot_github::Server;
use reqwest::blocking::Client;
use serde_json::json;

/// Creates `admin/tool` and pushes `main` (README and `dir/a.txt`) plus `rpc/inbox`.
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
    std::fs::create_dir(dir.join("dir")).unwrap();
    commit_file(&dir, "dir/a.txt", "a\n");
    let url = clone_url(server, "admin", "tool", Some(server.admin_token()));
    git_ok(&dir, &["push", "-q", &url, "main", "main:rpc/inbox"]);
    let head = git_ok(&dir, &["rev-parse", "HEAD"]);
    (work, head)
}

#[test]
fn empty_repositories_report_conflict() {
    let server = Server::start().unwrap();
    let admin = Api::admin(&server);
    admin.post("/user/repos", json!({ "name": "empty" }));
    for path in [
        "/repos/admin/empty/git/matching-refs/heads/",
        "/repos/admin/empty/git/ref/heads/main",
        "/repos/admin/empty/commits/main",
    ] {
        let reply = admin.get(path);
        assert_eq!(reply.status, 409, "{path}");
        assert_eq!(reply.body["message"], "Git Repository is empty.");
    }
    assert_eq!(
        admin.get("/repos/admin/empty/contents/README.md").status,
        404
    );
}

#[test]
fn refs_and_matching_refs() {
    let server = Server::start().unwrap();
    let (_work, head) = pushed(&server);
    let admin = Api::admin(&server);
    let found = admin.get("/repos/admin/tool/git/ref/heads/rpc/inbox");
    assert_eq!(found.status, 200);
    assert_eq!(found.body["ref"], "refs/heads/rpc/inbox");
    assert_eq!(found.body["object"]["sha"], head);
    assert_eq!(found.body["object"]["type"], "commit");
    assert_eq!(
        admin.get("/repos/admin/tool/git/ref/heads/nope").status,
        404
    );

    let matching = admin.get("/repos/admin/tool/git/matching-refs/heads/rpc/");
    let names: Vec<&str> = matching
        .body
        .as_array()
        .unwrap()
        .iter()
        .map(|found| found["ref"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["refs/heads/rpc/inbox"]);
    assert_eq!(
        admin.get("/repos/admin/tool/git/matching-refs/tags/").body,
        json!([])
    );
}

#[test]
fn commits_by_branch_and_sha() {
    let server = Server::start().unwrap();
    let (_work, head) = pushed(&server);
    let admin = Api::admin(&server);
    let by_branch = admin.get("/repos/admin/tool/commits/rpc/inbox");
    assert_eq!(by_branch.status, 200);
    assert_eq!(by_branch.body["sha"], head);
    assert_eq!(by_branch.body["commit"]["message"], "Add dir/a.txt");
    assert_eq!(
        by_branch.body["commit"]["author"]["email"],
        "test@example.com"
    );
    assert_eq!(by_branch.body["parents"].as_array().unwrap().len(), 1);
    assert_eq!(
        admin.get(&format!("/repos/admin/tool/commits/{head}")).body["sha"],
        head
    );
    assert_eq!(admin.get("/repos/admin/tool/commits/nope").status, 422);
}

#[test]
fn branches() {
    let server = Server::start().unwrap();
    let (_work, head) = pushed(&server);
    let admin = Api::admin(&server);
    let list = admin.get("/repos/admin/tool/branches");
    let names: Vec<&str> = list
        .body
        .as_array()
        .unwrap()
        .iter()
        .map(|branch| branch["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["main", "rpc/inbox"]);
    let branch = admin.get("/repos/admin/tool/branches/rpc/inbox");
    assert_eq!(branch.body["name"], "rpc/inbox");
    assert_eq!(branch.body["commit"]["sha"], head);
    assert_eq!(admin.get("/repos/admin/tool/branches/nope").status, 404);
}

#[test]
fn contents_as_json_raw_and_directory() {
    let server = Server::start().unwrap();
    let (_work, _) = pushed(&server);
    let admin = Api::admin(&server);
    let file = admin.get("/repos/admin/tool/contents/dir/a.txt");
    assert_eq!(file.status, 200);
    assert_eq!(file.body["type"], "file");
    assert_eq!(file.body["encoding"], "base64");
    let content = file.body["content"].as_str().unwrap().replace('\n', "");
    assert_eq!(STANDARD.decode(content).unwrap(), b"a\n");

    let raw = Client::new()
        .get(format!(
            "{}/repos/admin/tool/contents/README.md?ref=rpc/inbox",
            server.url()
        ))
        .header("Accept", "application/vnd.github.raw")
        .header("Authorization", format!("token {}", server.admin_token()))
        .send()
        .unwrap();
    assert_eq!(raw.status(), 200);
    assert_eq!(raw.text().unwrap(), "hello\n");

    let directory = admin.get("/repos/admin/tool/contents/dir");
    let entries = directory.body.as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["path"], "dir/a.txt");
    assert_eq!(admin.get("/repos/admin/tool/contents/missing").status, 404);
    assert_eq!(
        admin
            .get("/repos/admin/tool/contents/README.md?ref=nope")
            .status,
        404
    );
}

#[test]
fn old_locations_redirect_git_data() {
    let server = Server::start().unwrap();
    let (_work, _) = pushed(&server);
    let admin = Api::admin(&server);
    admin.patch("/repos/admin/tool", json!({ "name": "tool2" }));
    let moved = admin.get("/repos/admin/tool/contents/README.md?ref=main");
    assert_eq!(moved.status, 301);
    assert_eq!(
        moved.header("location").unwrap(),
        format!("{}/repos/admin/tool2/contents/README.md", server.url())
    );
}

#[cfg(unix)]
#[test]
fn symlinks_report_their_target() {
    let server = Server::start().unwrap();
    let (work, _) = pushed(&server);
    let dir = work.join("w");
    std::os::unix::fs::symlink("dir/a.txt", dir.join("link")).unwrap();
    git_ok(&dir, &["add", "link"]);
    git_ok(&dir, &["commit", "-q", "-m", "Add link"]);
    let url = clone_url(&server, "admin", "tool", Some(server.admin_token()));
    git_ok(&dir, &["push", "-q", &url, "main"]);
    let link = Api::admin(&server).get("/repos/admin/tool/contents/link");
    assert_eq!(link.status, 200, "{}", link.body);
    assert_eq!(link.body["type"], "symlink");
    assert_eq!(link.body["target"], "dir/a.txt");
}

#[test]
fn gitlinks_are_submodules_but_list_as_files() {
    let server = Server::start().unwrap();
    let (work, _) = pushed(&server);
    let dir = work.join("w");
    let pinned = git_ok(&dir, &["rev-parse", "HEAD"]);
    let hosted = format!("{}/admin/lib.git", server.url());
    for (name, path, url) in [
        ("lib", "vendor/lib", hosted.as_str()),
        (
            "other",
            "vendor/other",
            "https://example.com/team/other.git",
        ),
    ] {
        let key = format!("submodule.{name}");
        git_ok(
            &dir,
            &[
                "config",
                "--file",
                ".gitmodules",
                &format!("{key}.path"),
                path,
            ],
        );
        git_ok(
            &dir,
            &[
                "config",
                "--file",
                ".gitmodules",
                &format!("{key}.url"),
                url,
            ],
        );
        let cacheinfo = format!("160000,{pinned},{path}");
        git_ok(&dir, &["update-index", "--add", "--cacheinfo", &cacheinfo]);
    }
    git_ok(&dir, &["add", ".gitmodules"]);
    git_ok(&dir, &["commit", "-q", "-m", "Add submodules"]);
    let url = clone_url(&server, "admin", "tool", Some(server.admin_token()));
    git_ok(&dir, &["push", "-q", &url, "main"]);
    let admin = Api::admin(&server);
    let base = server.url();

    let submodule = admin.get("/repos/admin/tool/contents/vendor/lib");
    assert_eq!(submodule.status, 200, "{}", submodule.body);
    let git_url = format!("{base}/repos/admin/lib/git/trees/{pinned}");
    let html_url = format!("{base}/admin/lib/tree/{pinned}");
    let self_url = format!("{base}/repos/admin/tool/contents/vendor/lib?ref=main");
    assert_eq!(
        submodule.body,
        json!({
            "name": "lib",
            "path": "vendor/lib",
            "sha": pinned,
            "size": 0,
            "url": self_url,
            "html_url": html_url,
            "git_url": git_url,
            "download_url": null,
            "type": "submodule",
            "submodule_git_url": hosted,
            "_links": { "self": self_url, "git": git_url, "html": html_url },
        })
    );
    let external = admin.get("/repos/admin/tool/contents/vendor/other");
    assert_eq!(external.body["type"], "submodule");
    assert_eq!(
        external.body["submodule_git_url"],
        "https://example.com/team/other.git"
    );
    assert_eq!(external.body["html_url"], json!(null));

    let listing = admin.get("/repos/admin/tool/contents/vendor");
    assert_eq!(listing.status, 200, "{}", listing.body);
    assert_eq!(
        listing.body[0],
        json!({
            "name": "lib",
            "path": "vendor/lib",
            "sha": pinned,
            "size": 0,
            "url": self_url,
            "html_url": html_url,
            "git_url": git_url,
            "download_url": null,
            "type": "file",
            "_links": { "self": self_url, "git": git_url, "html": html_url },
        })
    );
    assert_eq!(listing.body[1]["type"], "file");
    assert_eq!(listing.body[1]["git_url"], json!(null));
}

fn decoded_content(reply: &common::Reply) -> String {
    let encoded: String = reply.body["content"]
        .as_str()
        .unwrap()
        .split_whitespace()
        .collect();
    String::from_utf8(STANDARD.decode(encoded).unwrap()).unwrap()
}

#[test]
fn putting_contents_creates_a_file_on_a_new_branch() {
    let server = Server::start().unwrap();
    let (work, head) = pushed(&server);
    let admin = Api::admin(&server);
    let base = admin.get("/repos/admin/tool/git/ref/heads/main");
    let sha = base.body["object"]["sha"].as_str().unwrap();
    let branch = admin.post(
        "/repos/admin/tool/git/refs",
        json!({ "ref": "refs/heads/setup", "sha": sha }),
    );
    assert_eq!(branch.status, 201, "{}", branch.body);

    let created = admin.put(
        "/repos/admin/tool/contents/.github/config.yml",
        json!({
            "message": "Add config",
            "content": STANDARD.encode("label: automerge\n"),
            "branch": "setup",
        }),
    );
    assert_eq!(created.status, 201, "{}", created.body);
    let content = &created.body["content"];
    assert_eq!(content["name"], "config.yml");
    assert_eq!(content["path"], ".github/config.yml");
    assert_eq!(content["type"], "file");
    assert_eq!(content["size"], 17);
    let commit = &created.body["commit"];
    assert_eq!(commit["message"], "Add config");
    assert_eq!(commit["parents"][0]["sha"], head);
    assert_eq!(commit["author"]["name"], "admin");
    assert_eq!(commit["committer"]["name"], "admin");

    let tip = admin.get("/repos/admin/tool/git/ref/heads/setup");
    assert_eq!(tip.body["object"]["sha"], commit["sha"]);
    let file = admin.get("/repos/admin/tool/contents/.github/config.yml?ref=setup");
    assert_eq!(file.status, 200, "{}", file.body);
    assert_eq!(file.body["sha"], content["sha"]);
    assert_eq!(decoded_content(&file), "label: automerge\n");
    assert_eq!(
        admin
            .get("/repos/admin/tool/contents/.github/config.yml")
            .status,
        404
    );

    let dir = work.join("w");
    let url = clone_url(&server, "admin", "tool", Some(server.admin_token()));
    git_ok(&dir, &["fetch", "-q", &url, "setup"]);
    assert_eq!(
        git_ok(&dir, &["show", "FETCH_HEAD:.github/config.yml"]),
        "label: automerge"
    );
}

#[test]
fn putting_contents_updates_a_file_only_with_its_current_sha() {
    let server = Server::start().unwrap();
    let (_work, head) = pushed(&server);
    let admin = Api::admin(&server);
    let current = admin.get("/repos/admin/tool/contents/README.md");
    let blob = current.body["sha"].as_str().unwrap().to_owned();
    let update = |sha: Option<&str>| {
        let mut body = json!({
            "message": "Update README",
            "content": STANDARD.encode("updated\n"),
            "committer": { "name": "Mona", "email": "mona@example.com" },
            "author": {
                "name": "Hubot",
                "email": "hubot@example.com",
                "date": "2026-01-05T13:13:22+05:00",
            },
        });
        if let Some(sha) = sha {
            body["sha"] = json!(sha);
        }
        admin.put("/repos/admin/tool/contents/README.md", body)
    };

    let missing = update(None);
    assert_eq!(missing.status, 422, "{}", missing.body);
    assert_eq!(
        missing.body["message"],
        "Invalid request.\n\n\"sha\" wasn't supplied."
    );
    let stale = update(Some("0000000000000000000000000000000000000000"));
    assert_eq!(stale.status, 409, "{}", stale.body);
    assert_eq!(
        stale.body["message"],
        "README.md does not match 0000000000000000000000000000000000000000"
    );
    assert_eq!(
        admin.get("/repos/admin/tool/git/ref/heads/main").body["object"]["sha"],
        head
    );

    let updated = update(Some(&blob));
    assert_eq!(updated.status, 200, "{}", updated.body);
    let commit = &updated.body["commit"];
    assert_eq!(commit["author"]["name"], "Hubot");
    assert_eq!(commit["author"]["date"], "2026-01-05T08:13:22Z");
    assert_eq!(commit["committer"]["email"], "mona@example.com");
    let by_sha = admin.get(&format!(
        "/repos/admin/tool/commits/{}",
        commit["sha"].as_str().unwrap()
    ));
    assert_eq!(by_sha.body["commit"]["message"], "Update README");
    assert_eq!(
        by_sha.body["commit"]["author"]["email"],
        "hubot@example.com"
    );
    let file = admin.get("/repos/admin/tool/contents/README.md");
    assert_eq!(file.body["sha"], updated.body["content"]["sha"]);
    assert_eq!(decoded_content(&file), "updated\n");
}

#[test]
fn putting_contents_rejects_unknown_branches_and_readers() {
    let server = Server::start().unwrap();
    let (_work, _) = pushed(&server);
    let admin = Api::admin(&server);
    let body = json!({ "message": "Add", "content": STANDARD.encode("x") });
    let mut on_missing_branch = body.clone();
    on_missing_branch["branch"] = json!("nope");
    let missing = admin.put("/repos/admin/tool/contents/new.txt", on_missing_branch);
    assert_eq!(missing.status, 404, "{}", missing.body);
    assert_eq!(missing.body["message"], "Branch nope not found");

    let mut not_base64 = body.clone();
    not_base64["content"] = json!("not base64!");
    assert_eq!(
        admin
            .put("/repos/admin/tool/contents/new.txt", not_base64)
            .status,
        422
    );

    server.add_user("reader");
    let reader = Api::new(&server, Some(&server.add_token("reader", &["repo"])));
    assert_eq!(
        reader
            .put("/repos/admin/tool/contents/new.txt", body)
            .status,
        404
    );
}

#[test]
fn putting_contents_into_an_empty_repository_creates_its_branch() {
    let server = Server::start().unwrap();
    let admin = Api::admin(&server);
    admin.post("/user/repos", json!({ "name": "empty" }));
    let created = admin.put(
        "/repos/admin/empty/contents/README.md",
        json!({ "message": "Initial commit", "content": STANDARD.encode("hi\n") }),
    );
    assert_eq!(created.status, 201, "{}", created.body);
    assert!(created.body["commit"]["parents"][0].is_null());
    let file = admin.get("/repos/admin/empty/contents/README.md");
    assert_eq!(decoded_content(&file), "hi\n");
}
