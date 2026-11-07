//! Users, organizations and the repository lifecycle.

mod common;

use common::Api;
use ninot_github::Server;
use serde_json::json;

fn names(reply: &common::Reply) -> Vec<String> {
    reply
        .body
        .as_array()
        .unwrap()
        .iter()
        .map(|repo| repo["full_name"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn authenticated_user_and_profiles() {
    let server = Server::start().unwrap();
    server.add_user("alice");
    server.add_org("acme", &["alice"]);
    let alice = Api::new(
        &server,
        Some(&server.add_user("alice").add_token(&["repo"])),
    );
    let me = alice.get("/user");
    assert_eq!(me.status, 200);
    assert_eq!(me.body["login"], "alice");
    assert_eq!(me.body["type"], "User");
    assert_eq!(Api::new(&server, None).get("/user").status, 401);
    assert_eq!(alice.get("/users/ALICE").body["login"], "alice");
    assert_eq!(alice.get("/orgs/acme").body["type"], "Organization");
    assert_eq!(alice.get("/orgs/alice").status, 404);
    assert_eq!(alice.get("/users/nobody").status, 404);
}

#[test]
fn create_get_and_list_repositories() {
    let server = Server::start().unwrap();
    server.add_user("alice");
    server.add_user("bob");
    server.add_org("acme", &["alice"]);
    let alice = Api::new(
        &server,
        Some(&server.add_user("alice").add_token(&["repo"])),
    );
    let bob = Api::new(&server, Some(&server.add_user("bob").add_token(&["repo"])));

    let created = alice.post("/user/repos", json!({ "name": "tool", "description": "d" }));
    assert_eq!(created.status, 201);
    assert_eq!(created.body["full_name"], "alice/tool");
    assert_eq!(created.body["private"], false);
    assert_eq!(created.body["owner"]["login"], "alice");
    assert!(
        created.body["clone_url"]
            .as_str()
            .unwrap()
            .ends_with("/alice/tool.git")
    );
    let duplicate = alice.post("/user/repos", json!({ "name": "TOOL" }));
    assert_eq!(duplicate.status, 422);
    assert_eq!(
        duplicate.body,
        json!({
            "message": "Validation Failed",
            "errors": [{ "resource": "Repository", "field": "name", "code": "custom" }],
            "documentation_url": "https://docs.github.com/rest",
            "status": "422",
        })
    );
    assert_eq!(alice.post("/user/repos", json!({})).status, 422);
    assert_eq!(
        alice.post("/user/repos", json!({ "name": "a b" })).status,
        422
    );
    assert_eq!(
        alice
            .post("/user/repos", json!({ "name": ".secret", "private": true }))
            .status,
        201
    );
    assert_eq!(
        alice
            .post(
                "/orgs/acme/repos",
                json!({ "name": "shared", "visibility": "private" })
            )
            .status,
        201
    );
    assert_eq!(
        bob.post("/orgs/acme/repos", json!({ "name": "x" })).status,
        403
    );
    assert_eq!(
        bob.post("/orgs/nope/repos", json!({ "name": "x" })).status,
        404
    );

    assert_eq!(
        names(&alice.get("/user/repos")),
        ["acme/shared", "alice/.secret", "alice/tool"]
    );
    assert_eq!(
        names(&alice.get("/user/repos?visibility=private")),
        ["acme/shared", "alice/.secret"]
    );
    assert_eq!(names(&bob.get("/user/repos")), Vec::<String>::new());
    assert_eq!(names(&bob.get("/users/alice/repos")), ["alice/tool"]);
    assert_eq!(names(&bob.get("/orgs/acme/repos")), Vec::<String>::new());
    assert_eq!(names(&alice.get("/orgs/acme/repos")), ["acme/shared"]);

    let fetched = bob.get("/repos/Alice/Tool");
    assert_eq!(fetched.status, 200);
    assert_eq!(fetched.body["permissions"]["admin"], false);
    assert_eq!(fetched.body["permissions"]["pull"], true);
    assert_eq!(bob.get("/repos/alice/.secret").status, 404);
    assert_eq!(
        alice.get("/repos/alice/.secret").body["visibility"],
        "private"
    );
}

#[test]
fn list_is_paginated() {
    let server = Server::start().unwrap();
    let admin = Api::admin(&server);
    for index in 0..5 {
        assert_eq!(
            admin
                .post("/user/repos", json!({ "name": format!("r{index}") }))
                .status,
            201
        );
    }
    let page = admin.get("/user/repos?per_page=2&page=2");
    assert_eq!(names(&page), ["admin/r2", "admin/r3"]);
    let link = page.header("link").unwrap();
    assert!(
        link.contains("rel=\"next\"") && link.contains("rel=\"prev\""),
        "{link}"
    );
}

#[test]
fn scopes_limit_what_a_token_may_do() {
    let server = Server::start().unwrap();
    server.add_user("alice");
    let public_only = Api::new(
        &server,
        Some(&server.add_user("alice").add_token(&["public_repo"])),
    );
    assert_eq!(
        public_only
            .post("/user/repos", json!({ "name": "open" }))
            .status,
        201
    );
    assert_eq!(
        public_only
            .post("/user/repos", json!({ "name": "closed", "private": true }))
            .status,
        403
    );
    let repo_only = Api::new(
        &server,
        Some(&server.add_user("alice").add_token(&["repo"])),
    );
    let refused = repo_only.delete("/repos/alice/open");
    assert_eq!(refused.status, 403);
    assert_eq!(
        refused.body["message"],
        "Must have admin rights to Repository."
    );
    let deleter = Api::new(
        &server,
        Some(&server.add_user("alice").add_token(&["repo", "delete_repo"])),
    );
    assert_eq!(deleter.delete("/repos/alice/open").status, 204);
    assert_eq!(deleter.get("/repos/alice/open").status, 404);
}

#[test]
fn rename_and_transfer_redirect_old_locations() {
    let server = Server::start().unwrap();
    server.add_user("alice");
    server.add_user("bob");
    server.add_org("acme", &["alice"]);
    let alice = Api::new(
        &server,
        Some(&server.add_user("alice").add_token(&["repo"])),
    );
    let bob = Api::new(&server, Some(&server.add_user("bob").add_token(&["repo"])));
    alice.post("/user/repos", json!({ "name": "tool" }));
    assert_eq!(
        bob.patch("/repos/alice/tool", json!({ "private": true }))
            .status,
        403
    );

    let renamed = alice.patch(
        "/repos/alice/tool",
        json!({ "name": "tool2", "private": true }),
    );
    assert_eq!(renamed.status, 200);
    assert_eq!(renamed.body["full_name"], "alice/tool2");
    assert_eq!(renamed.body["private"], true);

    let transferred = alice.post(
        "/repos/alice/tool2/transfer",
        json!({ "new_owner": "acme" }),
    );
    assert_eq!(transferred.status, 202);
    assert_eq!(transferred.body["full_name"], "acme/tool2");
    assert_eq!(
        alice
            .post("/repos/acme/tool2/transfer", json!({ "new_owner": "bob" }))
            .status,
        422
    );

    let moved = alice.get("/api/v3/repos/alice/tool");
    assert_eq!(moved.status, 301);
    let location = format!("{}/api/v3/repos/acme/tool2", server.url());
    assert_eq!(moved.header("location").unwrap(), location);
    assert_eq!(
        moved.body,
        json!({
            "message": "Moved Permanently",
            "url": location,
            "documentation_url": "https://docs.github.com/rest",
        })
    );
    assert_eq!(
        alice.get("/repos/alice/tool").header("location").unwrap(),
        format!("{}/repos/acme/tool2", server.url())
    );
    // The repository is private, so its old location reveals nothing to outsiders.
    assert_eq!(bob.get("/repos/alice/tool").status, 404);
}

#[test]
fn update_stores_every_setting() {
    let server = Server::start().unwrap();
    let admin = Api::admin(&server);
    admin.post("/user/repos", json!({ "name": "tool" }));
    let changes = json!({
        "homepage": "https://example.com",
        "has_issues": false,
        "has_projects": false,
        "has_wiki": false,
        "has_pull_requests": false,
        "pull_request_creation_policy": "collaborators_only",
        "is_template": true,
        "allow_squash_merge": false,
        "allow_merge_commit": false,
        "allow_rebase_merge": false,
        "allow_auto_merge": true,
        "delete_branch_on_merge": true,
        "allow_update_branch": true,
        "use_squash_pr_title_as_default": true,
        "squash_merge_commit_title": "PR_TITLE",
        "squash_merge_commit_message": "PR_BODY",
        "merge_commit_title": "PR_TITLE",
        "merge_commit_message": "PR_BODY",
        "archived": true,
        "allow_forking": false,
        "web_commit_signoff_required": true,
    });
    let before = admin.get("/repos/admin/tool").body;
    for (field, value) in changes.as_object().unwrap() {
        assert_ne!(&before[field], value, "{field} already has the new value");
    }
    let updated = admin.patch("/repos/admin/tool", changes.clone());
    assert_eq!(updated.status, 200, "{}", updated.body);
    let got = admin.get("/repos/admin/tool").body;
    for (field, value) in changes.as_object().unwrap() {
        assert_eq!(
            &updated.body[field], value,
            "{field} in the update response"
        );
        assert_eq!(&got[field], value, "{field} after the update");
    }

    let graphql = admin.post(
        "/api/graphql",
        json!({ "query": "{repository(owner: \"admin\", name: \"tool\"){hasIssuesEnabled hasWikiEnabled mergeCommitAllowed squashMergeAllowed rebaseMergeAllowed isArchived isTemplate}}" }),
    );
    assert_eq!(
        graphql.body["data"]["repository"],
        json!({
            "hasIssuesEnabled": false,
            "hasWikiEnabled": false,
            "mergeCommitAllowed": false,
            "squashMergeAllowed": false,
            "rebaseMergeAllowed": false,
            "isArchived": true,
            "isTemplate": true,
        })
    );

    let partial = admin.patch("/repos/admin/tool", json!({ "has_wiki": true }));
    assert_eq!(partial.body["has_wiki"], true);
    assert_eq!(partial.body["has_issues"], false);
    assert_eq!(
        admin
            .patch("/repos/admin/tool", json!({ "homepage": null }))
            .body["message"],
        "Invalid request.\n\nFor 'properties/homepage', nil is not a string."
    );
    assert_eq!(
        admin
            .patch("/repos/admin/tool", json!({ "has_wiki": "no" }))
            .status,
        422
    );
    assert_eq!(
        admin
            .patch(
                "/repos/admin/tool",
                json!({ "merge_commit_title": "BLANK" })
            )
            .status,
        422
    );
    assert_eq!(admin.get("/repos/admin/tool").body["has_wiki"], true);
}

/// GitHub's answer to a body that violates the operation's request schema.
fn invalid_request(reply: &common::Reply, message: &str) {
    assert_eq!(reply.status, 422, "{}", reply.body);
    assert_eq!(
        reply.body["message"],
        format!("Invalid request.\n\n{message}")
    );
    assert_eq!(reply.body["status"], "422");
}

#[test]
fn wrongly_typed_repository_fields_are_invalid_requests() {
    let server = Server::start().unwrap();
    let admin = Api::admin(&server);
    let created = admin.post("/user/repos", json!({ "name": "tool", "private": "yes" }));
    invalid_request(
        &created,
        "For 'properties/private', \"yes\" is not a boolean.",
    );
    assert_eq!(
        created.body["documentation_url"],
        "https://docs.github.com/rest/repos/repos#create-a-repository-for-the-authenticated-user"
    );
    assert_eq!(admin.get("/repos/admin/tool").status, 404);
    invalid_request(
        &admin.post("/user/repos", json!({})),
        "\"name\" wasn't supplied.",
    );

    admin.post("/user/repos", json!({ "name": "tool" }));
    invalid_request(
        &admin.patch("/repos/admin/tool", json!({ "has_wiki": "no" })),
        "For 'properties/has_wiki', \"no\" is not a boolean.",
    );
    invalid_request(
        &admin.patch("/repos/admin/tool", json!({ "visibility": "internal" })),
        "internal is not a member of [\"public\", \"private\"].",
    );
    assert_eq!(admin.get("/repos/admin/tool").body["private"], false);
}

#[test]
fn failed_update_changes_nothing() {
    let server = Server::start().unwrap();
    let admin = Api::admin(&server);
    admin.post("/user/repos", json!({ "name": "tool", "auto_init": true }));
    let before = admin.get("/repos/admin/tool").body;

    let refused = admin.patch(
        "/repos/admin/tool",
        json!({ "name": "renamed", "has_wiki": false, "default_branch": "nope" }),
    );
    assert_eq!(refused.status, 422, "{}", refused.body);
    assert_eq!(
        refused.body["errors"],
        json!([{ "resource": "Repository", "field": "default_branch", "code": "invalid" }])
    );
    assert_eq!(admin.get("/repos/admin/renamed").status, 404);
    let after = admin.get("/repos/admin/tool");
    assert_eq!(after.status, 200);
    assert_eq!(after.body["name"], "tool");
    assert_eq!(after.body["has_wiki"], true);
    assert_eq!(after.body["default_branch"], "main");
    assert_eq!(after.body["updated_at"], before["updated_at"]);
}

#[test]
fn repository_search_resolves_user_me_to_the_caller() {
    let server = Server::start().unwrap();
    server.add_user("octocat");
    server.add_user("other");
    for owner in ["octocat", "other"] {
        server
            .create_repository(owner, "match", false, None, None, "main")
            .unwrap();
    }
    let octocat = Api::new(
        &server,
        Some(&server.add_user("octocat").add_token(&["repo"])),
    );
    let found = octocat.get("/search/repositories?q=match+in%3Aname%2Cdescription+user%3A%40me");
    assert_eq!(found.status, 200, "{}", found.body);
    let names: Vec<&str> = found.body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|repo| repo["full_name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["octocat/match"]);
}
