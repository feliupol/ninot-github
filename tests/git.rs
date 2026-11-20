//! Git hosting over smart HTTP with the real git client.

mod common;

use common::{Api, TempDir, clone_url, commit_file, git, git_ok};
use ninot_github::Server;
use serde_json::json;

#[test]
fn push_then_clone_and_fetch() {
    let server = Server::start().unwrap();
    let admin = Api::admin(&server);
    assert_eq!(
        admin.post("/user/repos", json!({ "name": "tool" })).status,
        201
    );
    let url = clone_url(&server, "admin", "tool", Some(server.admin_token()));
    let work = TempDir::new();

    git_ok(&work.0, &["init", "-q", "-b", "main", "a"]);
    let a = work.join("a");
    commit_file(&a, "one.txt", "1");
    git_ok(&a, &["push", "-q", &url, "main"]);

    git_ok(&work.0, &["clone", "-q", &url, "b"]);
    let b = work.join("b");
    assert_eq!(std::fs::read_to_string(b.join("one.txt")).unwrap(), "1");

    commit_file(&a, "two.txt", "2");
    git_ok(&a, &["push", "-q", &url, "main"]);
    git_ok(&b, &["pull", "-q", "--ff-only"]);
    assert_eq!(std::fs::read_to_string(b.join("two.txt")).unwrap(), "2");

    assert!(admin.get("/repos/admin/tool").body["pushed_at"].is_string());
}

#[test]
fn public_repositories_clone_anonymously_but_refuse_anonymous_pushes() {
    let server = Server::start().unwrap();
    let admin = Api::admin(&server);
    admin.post("/user/repos", json!({ "name": "open", "auto_init": true }));
    let work = TempDir::new();
    git_ok(
        &work.0,
        &[
            "clone",
            "-q",
            &clone_url(&server, "admin", "open", None),
            "c",
        ],
    );
    let clone = work.join("c");
    assert_eq!(
        std::fs::read_to_string(clone.join("README.md")).unwrap(),
        "# open\n"
    );
    commit_file(&clone, "x.txt", "x");
    let push = git(&clone, &["push", "-q", "origin", "main"]);
    assert!(!push.status.success());
}

#[test]
fn private_repositories_need_access() {
    let server = Server::start().unwrap();
    server.add_user("bob");
    let bob = server.add_user("bob").add_token(&["repo"]);
    let admin = Api::admin(&server);
    admin.post(
        "/user/repos",
        json!({ "name": "secret", "private": true, "auto_init": true }),
    );
    let work = TempDir::new();
    for (token, name) in [(None, "anonymous"), (Some(bob.as_str()), "bob")] {
        let output = git(
            &work.0,
            &[
                "clone",
                "-q",
                &clone_url(&server, "admin", "secret", token),
                name,
            ],
        );
        assert!(
            !output.status.success(),
            "{name} cloned a private repository"
        );
    }
    git_ok(
        &work.0,
        &[
            "clone",
            "-q",
            &clone_url(&server, "admin", "secret", Some(server.admin_token())),
            "ok",
        ],
    );
}

#[test]
fn readers_cannot_push() {
    let server = Server::start().unwrap();
    server.add_user("bob");
    let bob = server.add_user("bob").add_token(&["repo"]);
    Api::admin(&server).post("/user/repos", json!({ "name": "open", "auto_init": true }));
    let work = TempDir::new();
    git_ok(
        &work.0,
        &[
            "clone",
            "-q",
            &clone_url(&server, "admin", "open", Some(&bob)),
            "c",
        ],
    );
    let clone = work.join("c");
    commit_file(&clone, "x.txt", "x");
    let push = git(&clone, &["push", "-q", "origin", "main"]);
    assert!(!push.status.success());
    assert!(
        String::from_utf8_lossy(&push.stderr).contains("403"),
        "{}",
        String::from_utf8_lossy(&push.stderr)
    );
}

#[test]
fn renamed_repositories_keep_serving_git_at_the_old_url() {
    let server = Server::start().unwrap();
    let admin = Api::admin(&server);
    admin.post("/user/repos", json!({ "name": "old", "auto_init": true }));
    admin.patch("/repos/admin/old", json!({ "name": "new" }));
    let work = TempDir::new();
    git_ok(
        &work.0,
        &[
            "clone",
            "-q",
            &clone_url(&server, "admin", "old", Some(server.admin_token())),
            "c",
        ],
    );
    assert!(work.join("c/README.md").exists());
}

#[test]
fn deleting_a_repository_removes_it_from_git() {
    let server = Server::start().unwrap();
    let admin = Api::admin(&server);
    admin.post("/user/repos", json!({ "name": "gone", "auto_init": true }));
    assert_eq!(admin.delete("/repos/admin/gone").status, 204);
    let work = TempDir::new();
    let output = git(
        &work.0,
        &[
            "clone",
            "-q",
            &clone_url(&server, "admin", "gone", Some(server.admin_token())),
            "c",
        ],
    );
    assert!(!output.status.success());
}

#[test]
fn the_first_pushed_branch_becomes_the_default() {
    let server = Server::start().unwrap();
    let admin = Api::admin(&server);
    admin.post("/user/repos", json!({ "name": "tool" }));
    let url = clone_url(&server, "admin", "tool", Some(server.admin_token()));
    let work = TempDir::new();
    git_ok(&work.0, &["init", "-q", "-b", "master", "a"]);
    let a = work.join("a");
    commit_file(&a, "one.txt", "1");
    git_ok(&a, &["push", "-q", &url, "master"]);

    assert_eq!(
        admin.get("/repos/admin/tool").body["default_branch"],
        "master"
    );
    let contents = admin.get("/repos/admin/tool/contents/one.txt");
    assert_eq!(contents.status, 200);
    assert_eq!(contents.body["name"], "one.txt");
    let graphql = admin.post(
        "/graphql",
        json!({
            "query": "{ repository(owner: \"admin\", name: \"tool\") { defaultBranchRef { name } } }"
        }),
    );
    assert_eq!(
        graphql.body["data"]["repository"]["defaultBranchRef"]["name"],
        "master"
    );

    git_ok(&work.0, &["clone", "-q", &url, "b"]);
    assert_eq!(
        git_ok(&work.join("b"), &["branch", "--show-current"]).trim(),
        "master"
    );
}

#[test]
fn a_first_push_of_several_branches_prefers_master_over_others() {
    let server = Server::start().unwrap();
    let admin = Api::admin(&server);
    admin.post("/user/repos", json!({ "name": "tool" }));
    let url = clone_url(&server, "admin", "tool", Some(server.admin_token()));
    let work = TempDir::new();
    git_ok(&work.0, &["init", "-q", "-b", "alpha", "a"]);
    let a = work.join("a");
    commit_file(&a, "one.txt", "1");
    git_ok(&a, &["branch", "master"]);
    git_ok(&a, &["branch", "zeta"]);
    git_ok(&a, &["push", "-q", &url, "alpha", "master", "zeta"]);
    assert_eq!(
        admin.get("/repos/admin/tool").body["default_branch"],
        "master"
    );

    git_ok(&a, &["checkout", "-q", "-b", "later"]);
    git_ok(&a, &["push", "-q", &url, "later"]);
    assert_eq!(
        admin.get("/repos/admin/tool").body["default_branch"],
        "master"
    );
}

#[test]
fn commit_and_compare_honor_diff_and_patch_media_types() {
    let server = Server::start().unwrap();
    let admin = Api::admin(&server);
    assert_eq!(
        admin
            .post("/user/repos", json!({ "name": "tool", "auto_init": true }))
            .status,
        201
    );
    let url = clone_url(&server, "admin", "tool", Some(server.admin_token()));
    let work = TempDir::new();
    git_ok(&work.0, &["clone", "-q", &url, "c"]);
    let clone = work.join("c");
    git_ok(&clone, &["checkout", "-q", "-b", "feature"]);
    commit_file(&clone, "feature.txt", "feature\n");
    git_ok(&clone, &["push", "-q", "origin", "feature"]);

    let sha = admin.get("/repos/admin/tool/commits/feature").body["sha"]
        .as_str()
        .unwrap()
        .to_owned();
    let diff = admin.get_accept(
        &format!("/repos/admin/tool/commits/{sha}"),
        "application/vnd.github.diff",
    );
    assert_eq!(diff.status, 200, "{:?}", diff.body);
    let text = diff.body.as_str().unwrap();
    assert!(
        text.contains("diff --git a/feature.txt b/feature.txt"),
        "{text}"
    );
    assert!(text.contains("+feature"), "{text}");
    let patch = admin.get_accept(
        &format!("/repos/admin/tool/commits/{sha}"),
        "application/vnd.github.v3.patch",
    );
    assert!(
        patch
            .body
            .as_str()
            .unwrap()
            .contains("Subject: [PATCH] Add feature.txt"),
        "{:?}",
        patch.body
    );
    let compare = admin.get_accept(
        "/repos/admin/tool/compare/main...feature",
        "application/vnd.github.v3.diff",
    );
    assert_eq!(compare.status, 200, "{:?}", compare.body);
    assert!(
        compare.body.as_str().unwrap().contains("feature.txt"),
        "{:?}",
        compare.body
    );
    let compare_patch = admin.get_accept(
        "/repos/admin/tool/compare/main...feature",
        "application/vnd.github.patch",
    );
    assert!(
        compare_patch
            .body
            .as_str()
            .unwrap()
            .contains("Subject: [PATCH] Add feature.txt"),
        "{:?}",
        compare_patch.body
    );
    assert_eq!(
        admin
            .get_accept(
                "/repos/admin/tool/commits/deadbeef",
                "application/vnd.github.diff"
            )
            .status,
        422
    );
    assert_eq!(
        admin
            .get_accept(
                "/repos/admin/tool/compare/main...no-such-branch",
                "application/vnd.github.diff"
            )
            .status,
        404
    );
    assert!(admin.get("/repos/admin/tool/commits/feature").body["sha"].is_string());
}
