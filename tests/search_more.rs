//! User, label, topic, commit and code search.

mod common;

use common::Api;
use ninot_github::Server;
use serde_json::json;

fn repository(admin: &Api<'_>, name: &str, private: bool) -> serde_json::Value {
    let created = admin.post(
        "/user/repos",
        json!({ "name": name, "auto_init": true, "private": private }),
    );
    assert_eq!(created.status, 201, "{}", created.body);
    created.body
}

fn commit_file(admin: &Api<'_>, repo: &str, path: &str, content: &str, message: &str) {
    use base64::Engine as _;
    let written = admin.put(
        &format!("/repos/admin/{repo}/contents/{path}"),
        json!({
            "message": message,
            "content": base64::engine::general_purpose::STANDARD.encode(content),
        }),
    );
    assert_eq!(written.status, 201, "{}", written.body);
}

#[test]
fn users_are_searched_by_login_name_and_type() {
    let server = Server::start().unwrap();
    server
        .add_user("octocat")
        .set_name(Some("Mona Lisa".to_owned()));
    server.add_user("hubot");
    server.add_org("octo-org", &["octocat"]);
    let api = Api::new(&server, None);

    let found = api.get("/search/users?q=octo");
    assert_eq!(found.status, 200, "{}", found.body);
    assert_eq!(found.body["total_count"], 2);
    let users = api.get("/search/users?q=octo+type:user");
    assert_eq!(users.body["total_count"], 1);
    assert_eq!(users.body["items"][0]["login"], "octocat");
    let by_name = api.get("/search/users?q=mona+in:name");
    assert_eq!(
        by_name.body["items"][0]["login"], "octocat",
        "{}",
        by_name.body
    );
    let by_login_only = api.get("/search/users?q=mona+in:login");
    assert_eq!(by_login_only.body["total_count"], 0);
    let paged = api.get("/search/users?q=o&per_page=1");
    assert_eq!(paged.body["items"].as_array().unwrap().len(), 1);
    assert!(paged.header("link").is_some());

    assert_eq!(api.get("/search/users").status, 422);
    assert_eq!(api.get("/search/users?q=x&sort=stars").status, 422);
}

#[test]
fn labels_are_searched_in_one_readable_repository() {
    let server = Server::start().unwrap();
    let admin = Api::admin(&server);
    let repo = repository(&admin, "tool", true);
    for (name, description) in [("bug", "Something is broken"), ("docs", "Needs writing")] {
        let created = admin.post(
            "/repos/admin/tool/labels",
            json!({ "name": name, "description": description }),
        );
        assert_eq!(created.status, 201, "{}", created.body);
    }
    let id = &repo["id"];
    let found = admin.get(&format!("/search/labels?repository_id={id}&q=broken"));
    assert_eq!(found.status, 200, "{}", found.body);
    assert_eq!(found.body["total_count"], 1);
    assert_eq!(found.body["items"][0]["name"], "bug");

    assert_eq!(admin.get("/search/labels?q=bug").status, 422);
    assert_eq!(
        admin
            .get("/search/labels?repository_id=999999&q=bug")
            .status,
        404
    );
    // A private repository's labels are not found by others.
    let other = server.add_user("other");
    let other = Api::new(&server, Some(&other.add_token(&["repo"])));
    assert_eq!(
        other
            .get(&format!("/search/labels?repository_id={id}&q=bug"))
            .status,
        404
    );
}

#[test]
fn topics_search_answers_without_a_topic_catalog() {
    let server = Server::start().unwrap();
    let api = Api::new(&server, None);
    let found = api.get("/search/topics?q=rust");
    assert_eq!(found.status, 200, "{}", found.body);
    assert_eq!(found.body["total_count"], 0);
    assert_eq!(api.get("/search/topics").status, 422);
}

#[test]
fn commits_are_searched_by_message_and_qualifiers() {
    let server = Server::start().unwrap();
    let admin = Api::admin(&server);
    repository(&admin, "tool", false);
    repository(&admin, "secret", true);
    commit_file(
        &admin,
        "tool",
        "src/lib.rs",
        "fn widget() {}\n",
        "Add the widget",
    );
    commit_file(
        &admin,
        "secret",
        "notes.txt",
        "widget plans\n",
        "Plan the widget",
    );

    let found = admin.get("/search/commits?q=widget");
    assert_eq!(found.status, 200, "{}", found.body);
    assert_eq!(found.body["total_count"], 2, "{}", found.body);
    let one_repo = admin.get("/search/commits?q=widget+repo:admin/tool");
    assert_eq!(one_repo.body["total_count"], 1);
    let item = &one_repo.body["items"][0];
    assert_eq!(item["commit"]["message"], "Add the widget");
    assert_eq!(item["repository"]["full_name"], "admin/tool");
    let sha = item["sha"].as_str().unwrap();
    let by_hash = admin.get(&format!("/search/commits?q=hash:{}", &sha[..7]));
    assert_eq!(by_hash.body["items"][0]["sha"], sha);

    // Private repositories are searched only for those who can read them.
    let anonymous = Api::new(&server, None);
    assert_eq!(
        anonymous.get("/search/commits?q=widget").body["total_count"],
        1
    );
    assert_eq!(admin.get("/search/commits?q=x&sort=size").status, 422);
}

#[test]
fn code_is_searched_on_the_default_branch() {
    let server = Server::start().unwrap();
    let admin = Api::admin(&server);
    repository(&admin, "tool", false);
    commit_file(&admin, "tool", "src/lib.rs", "fn widget() {}\n", "Add lib");
    commit_file(
        &admin,
        "tool",
        "docs/guide.md",
        "Use the widget.\n",
        "Add guide",
    );

    let found = admin.get("/search/code?q=widget+repo:admin/tool");
    assert_eq!(found.status, 200, "{}", found.body);
    assert_eq!(found.body["total_count"], 2, "{}", found.body);
    let rust = admin.get("/search/code?q=widget+extension:rs");
    assert_eq!(rust.body["total_count"], 1);
    let item = &rust.body["items"][0];
    assert_eq!(item["path"], "src/lib.rs");
    assert_eq!(item["name"], "lib.rs");
    assert_eq!(item["repository"]["full_name"], "admin/tool");
    let under = admin.get("/search/code?q=widget+path:docs");
    assert_eq!(
        under.body["items"][0]["path"], "docs/guide.md",
        "{}",
        under.body
    );

    assert_eq!(
        Api::new(&server, None).get("/search/code?q=widget").status,
        401
    );
    assert_eq!(admin.get("/search/code").status, 422);
}
