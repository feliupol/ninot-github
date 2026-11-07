//! Issue update, labels, assignees, milestones, comments, locks and cross-repository lists.

mod common;

use std::collections::BTreeSet;

use ninot_github::Server;
use ninot_github::fixtures::{
    IssueCommentSeed, IssueSeed, LabelSeed, PullRequestSeed, ReviewCommentSeed, SeedField, UserSeed,
};
use serde_json::{Value, json};

fn titles(body: &Value) -> BTreeSet<String> {
    body.as_array()
        .unwrap()
        .iter()
        .map(|item| item["title"].as_str().unwrap().to_owned())
        .collect()
}

fn bodies(body: &Value) -> Vec<String> {
    body.as_array()
        .unwrap()
        .iter()
        .map(|item| item["body"].as_str().unwrap_or("").to_owned())
        .collect()
}

fn names(body: &Value) -> Vec<String> {
    body.as_array()
        .unwrap()
        .iter()
        .map(|item| item["name"].as_str().unwrap().to_owned())
        .collect()
}

fn logins(body: &Value) -> Vec<String> {
    body.as_array()
        .unwrap()
        .iter()
        .map(|item| item["login"].as_str().unwrap().to_owned())
        .collect()
}

fn user(login: &str) -> UserSeed {
    UserSeed {
        login: SeedField::present(login.to_owned()),
        ..Default::default()
    }
}

#[test]
fn issues_update() {
    let server = Server::start().unwrap();
    server.add_user("bob");
    server.add_user("carol");
    let bob_token = server.user("bob").unwrap().add_token(&["repo"]);
    let carol_token = server.user("carol").unwrap().add_token(&["repo"]);
    let repository = server
        .create_repository("admin", "project", false, None, None, "main")
        .unwrap();
    let admin = common::Api::admin(&server);
    let bob = common::Api::new(&server, Some(&bob_token));
    let carol = common::Api::new(&server, Some(&carol_token));
    let anon = common::Api::new(&server, None);

    let created = admin.post("/repos/admin/project/issues", json!({"title": "Ship it"}));
    assert_eq!(created.status, 201, "{:?}", created.body);
    let number = created.body["number"].as_u64().unwrap();
    let issue = format!("/repos/admin/project/issues/{number}");

    let titled = admin.patch(&issue, json!({"title": 7}));
    assert_eq!(titled.status, 200, "{:?}", titled.body);
    assert_eq!(titled.body["title"], "7");

    let blank = admin.patch(&issue, json!({"title": "  "}));
    assert_eq!(blank.status, 422, "{:?}", blank.body);
    assert_eq!(admin.get(&issue).body["title"], "7");

    let bad_reason = admin.patch(
        &issue,
        json!({"state": "closed", "state_reason": "reopened"}),
    );
    assert_eq!(bad_reason.status, 422, "{:?}", bad_reason.body);
    assert_eq!(admin.get(&issue).body["state"], "open");
    let bad_open = admin.patch(
        &issue,
        json!({"state": "open", "state_reason": "not_planned"}),
    );
    assert_eq!(bad_open.status, 422, "{:?}", bad_open.body);

    let label = admin.post(
        "/repos/admin/project/labels",
        json!({"name": "bug", "color": "abcdef"}),
    );
    assert_eq!(label.status, 201, "{:?}", label.body);
    let milestone = admin.post("/repos/admin/project/milestones", json!({"title": "v1"}));
    assert_eq!(milestone.status, 201, "{:?}", milestone.body);
    let closed = admin.patch(
        &issue,
        json!({
            "state": "closed",
            "milestone": milestone.body["number"].as_u64().unwrap(),
            "labels": ["bug"],
        }),
    );
    assert_eq!(closed.status, 200, "{:?}", closed.body);
    assert_eq!(closed.body["state"], "closed");
    assert_eq!(closed.body["state_reason"], "completed");
    assert!(closed.body["closed_at"].is_string());
    assert_eq!(closed.body["closed_by"]["login"], "admin");
    assert_eq!(closed.body["labels"][0]["name"], "bug");
    assert_eq!(closed.body["milestone"]["title"], "v1");
    assert_eq!(closed.body["milestone"]["open_issues"], 0);
    assert_eq!(closed.body["milestone"]["closed_issues"], 1);
    let closed_at = closed.body["closed_at"].clone();

    let noop = admin.patch(
        &issue,
        json!({"state": "closed", "state_reason": "not_planned"}),
    );
    assert_eq!(noop.status, 200, "{:?}", noop.body);
    assert_eq!(noop.body["state_reason"], "completed");
    assert_eq!(noop.body["closed_at"], closed_at);

    let reopened = admin.patch(&issue, json!({"state": "open"}));
    assert_eq!(reopened.status, 200, "{:?}", reopened.body);
    assert_eq!(reopened.body["state"], "open");
    assert_eq!(reopened.body["state_reason"], "reopened");
    assert!(reopened.body["closed_at"].is_null());
    assert!(reopened.body["closed_by"].is_null());

    let unknown = admin.patch(&issue, json!({"title": "changed", "milestone": 999}));
    assert_eq!(unknown.status, 422, "{:?}", unknown.body);
    assert_eq!(admin.get(&issue).body["title"], "7");

    let cleared = admin.patch(&issue, json!({"labels": []}));
    assert_eq!(cleared.status, 200, "{:?}", cleared.body);
    assert!(cleared.body["labels"].as_array().unwrap().is_empty());

    server
        .create_repository("admin", "other", false, None, None, "main")
        .unwrap()
        .create_issue(
            1,
            IssueSeed {
                title: SeedField::present("other".to_owned()),
                ..Default::default()
            },
        )
        .unwrap();
    let duplicate = admin.patch(
        &issue,
        json!({
            "state": "closed",
            "state_reason": "duplicate",
            "duplicate_issue_id": created.body["id"].as_i64().unwrap(),
        }),
    );
    assert_eq!(duplicate.status, 200, "{:?}", duplicate.body);
    assert_eq!(duplicate.body["state_reason"], "duplicate");

    let second = admin.post("/repos/admin/project/issues", json!({"title": "Second"}));
    assert_eq!(second.status, 201, "{:?}", second.body);
    let second_path = format!(
        "/repos/admin/project/issues/{}",
        second.body["number"].as_u64().unwrap()
    );
    let self_duplicate = admin.patch(
        &second_path,
        json!({
            "state": "closed",
            "state_reason": "duplicate",
            "duplicate_issue_id": second.body["id"].as_i64().unwrap(),
        }),
    );
    assert_eq!(self_duplicate.status, 422, "{:?}", self_duplicate.body);
    assert_eq!(admin.get(&second_path).body["state"], "open");

    repository
        .create_issue(
            30,
            IssueSeed {
                title: SeedField::present("authored".to_owned()),
                user: SeedField::present(user("bob")),
                ..Default::default()
            },
        )
        .unwrap();
    let authored = "/repos/admin/project/issues/30";
    let edited = bob.patch(
        authored,
        json!({"title": "kept", "milestone": 999, "labels": ["nope"]}),
    );
    assert_eq!(edited.status, 200, "{:?}", edited.body);
    assert_eq!(edited.body["title"], "kept");
    assert!(edited.body["milestone"].is_null());
    assert!(edited.body["labels"].as_array().unwrap().is_empty());
    assert_eq!(carol.patch(authored, json!({"title": "no"})).status, 403);
    assert_eq!(anon.patch(&issue, json!({"title": "no"})).status, 401);
    assert_eq!(
        bob.patch("/repos/admin/project/issues/404", json!({"title": "no"}))
            .status,
        404
    );
    assert_eq!(
        admin
            .patch("/repos/admin/missing/issues/1", json!({"title": "no"}))
            .status,
        404
    );

    repository
        .create_pull_request(
            40,
            PullRequestSeed {
                title: SeedField::present("pr".to_owned()),
                ..Default::default()
            },
        )
        .unwrap();
    let pr_patch = admin.patch("/repos/admin/project/issues/40", json!({"title": "nope"}));
    assert_eq!(pr_patch.status, 404, "{:?}", pr_patch.body);
    let pull = admin.get("/repos/admin/project/pulls/40");
    assert_eq!(pull.status, 200, "{:?}", pull.body);
    assert_eq!(pull.body["title"], "pr");
}

#[test]
fn issues_add_assignees() {
    let server = Server::start().unwrap();
    server.add_user("bob");
    server.add_user("carol");
    let bob_token = server.user("bob").unwrap().add_token(&["repo"]);
    let repository = server
        .create_repository("admin", "project", false, None, None, "main")
        .unwrap();
    let admin = common::Api::admin(&server);
    let bob = common::Api::new(&server, Some(&bob_token));
    let created = admin.post("/repos/admin/project/issues", json!({"title": "Assign"}));
    assert_eq!(created.status, 201, "{:?}", created.body);
    let issue = format!(
        "/repos/admin/project/issues/{}",
        created.body["number"].as_u64().unwrap()
    );
    let assignees = format!("{issue}/assignees");

    let added = admin.post(&assignees, json!({"assignees": ["admin"]}));
    assert_eq!(added.status, 201, "{:?}", added.body);
    assert_eq!(added.body["assignee"]["login"], "admin");
    assert_eq!(added.body["assignees"][0]["login"], "admin");
    let rejected = admin.post(&assignees, json!({"assignees": ["bob"]}));
    assert_eq!(rejected.status, 422, "{:?}", rejected.body);
    assert_eq!(admin.get(&issue).body["assignees"][0]["login"], "admin");
    let ignored = bob.post(&assignees, json!({"assignees": ["carol"]}));
    assert_eq!(ignored.status, 201, "{:?}", ignored.body);
    assert_eq!(ignored.body["assignees"].as_array().unwrap().len(), 1);
    assert_eq!(
        admin
            .post(
                "/repos/admin/project/issues/404/assignees",
                json!({"assignees": ["admin"]})
            )
            .status,
        404
    );

    repository
        .create_pull_request(40, PullRequestSeed::default())
        .unwrap();
    let pull_assignees = admin.post(
        "/repos/admin/project/issues/40/assignees",
        json!({"assignees": ["admin"]}),
    );
    assert_eq!(pull_assignees.status, 201, "{:?}", pull_assignees.body);
    assert!(pull_assignees.body["pull_request"].is_object());
    assert_eq!(pull_assignees.body["assignee"]["login"], "admin");

    let mut members = vec!["admin".to_owned()];
    for index in 1..=11 {
        let login = format!("u{index}");
        server.add_user(&login);
        members.push(login);
    }
    let member_refs: Vec<&str> = members.iter().map(String::as_str).collect();
    server.add_org("acme", &member_refs);
    server
        .create_repository("acme", "widgets", false, None, None, "main")
        .unwrap();
    let org_issue = admin.post("/repos/acme/widgets/issues", json!({"title": "Cap"}));
    assert_eq!(org_issue.status, 201, "{:?}", org_issue.body);
    let org_assignees = format!(
        "/repos/acme/widgets/issues/{}/assignees",
        org_issue.body["number"].as_u64().unwrap()
    );
    let ordered = admin.post(&org_assignees, json!({"assignees": ["u2", "u1"]}));
    assert_eq!(ordered.status, 201, "{:?}", ordered.body);
    assert_eq!(
        logins(&ordered.body["assignees"]),
        vec!["u2".to_owned(), "u1".to_owned()]
    );
    let appended = admin.post(&org_assignees, json!({"assignees": ["u3"]}));
    assert_eq!(
        logins(&appended.body["assignees"]),
        vec!["u2".to_owned(), "u1".to_owned(), "u3".to_owned()]
    );
    let rest: Vec<String> = (4..=10).map(|index| format!("u{index}")).collect();
    let filled = admin.post(&org_assignees, json!({"assignees": rest}));
    assert_eq!(filled.status, 201, "{:?}", filled.body);
    assert_eq!(filled.body["assignees"].as_array().unwrap().len(), 10);
    let overflow = admin.post(&org_assignees, json!({"assignees": ["u11"]}));
    assert_eq!(overflow.status, 422, "{:?}", overflow.body);
    assert_eq!(
        admin.get(&org_assignees.replace("/assignees", "")).body["assignees"]
            .as_array()
            .unwrap()
            .len(),
        10
    );
    let repeat = admin.post(&org_assignees, json!({"assignees": ["u1"]}));
    assert_eq!(repeat.status, 201, "{:?}", repeat.body);
    assert_eq!(repeat.body["assignees"].as_array().unwrap().len(), 10);
    let org_login = admin.post(&org_assignees, json!({"assignees": ["acme"]}));
    assert_eq!(org_login.status, 422, "{:?}", org_login.body);
}

#[test]
fn issues_remove_assignees() {
    let server = Server::start().unwrap();
    server.add_user("bob");
    let bob_token = server.user("bob").unwrap().add_token(&["repo"]);
    let repository = server
        .create_repository("admin", "project", false, None, None, "main")
        .unwrap();
    let admin = common::Api::admin(&server);
    let bob = common::Api::new(&server, Some(&bob_token));
    let created = admin.post("/repos/admin/project/issues", json!({"title": "Assign"}));
    let issue = format!(
        "/repos/admin/project/issues/{}",
        created.body["number"].as_u64().unwrap()
    );
    let assignees = format!("{issue}/assignees");
    assert_eq!(
        admin
            .post(&assignees, json!({"assignees": ["admin"]}))
            .status,
        201
    );
    let removed = admin.delete_json(&assignees, json!({"assignees": ["nobody"]}));
    assert_eq!(removed.status, 200, "{:?}", removed.body);
    assert_eq!(removed.body["assignee"]["login"], "admin");
    let cleared = admin.delete_json(&assignees, json!({"assignees": ["admin"]}));
    assert_eq!(cleared.status, 200, "{:?}", cleared.body);
    assert!(cleared.body["assignee"].is_null());
    assert!(cleared.body["assignees"].is_null());

    admin.post(&assignees, json!({"assignees": ["admin"]}));
    let ignored = bob.delete_json(&assignees, json!({"assignees": ["admin"]}));
    assert_eq!(ignored.status, 200, "{:?}", ignored.body);
    assert_eq!(ignored.body["assignee"]["login"], "admin");
    assert_eq!(
        admin
            .delete_json(
                "/repos/admin/project/issues/404/assignees",
                json!({"assignees": ["admin"]})
            )
            .status,
        404
    );

    repository
        .create_pull_request(40, PullRequestSeed::default())
        .unwrap();
    let pull = "/repos/admin/project/issues/40/assignees";
    admin.post(pull, json!({"assignees": ["admin"]}));
    let pull_cleared = admin.delete_json(pull, json!({"assignees": ["admin"]}));
    assert_eq!(pull_cleared.status, 200, "{:?}", pull_cleared.body);
    assert!(pull_cleared.body["pull_request"].is_object());
    assert!(pull_cleared.body["assignee"].is_null());
}

#[test]
fn issues_list_assignees() {
    let server = Server::start().unwrap();
    server.add_user("bob");
    server.add_user("member");
    let bob_token = server.user("bob").unwrap().add_token(&["repo"]);
    server
        .create_repository("admin", "project", false, None, None, "main")
        .unwrap();
    server
        .create_repository("admin", "secret", true, None, None, "main")
        .unwrap();
    server.add_org("acme", &["admin", "member", "ghost"]);
    server
        .create_repository("acme", "widgets", false, None, None, "main")
        .unwrap();
    let admin = common::Api::admin(&server);
    let bob = common::Api::new(&server, Some(&bob_token));

    let listed = admin.get("/repos/admin/project/assignees");
    assert_eq!(listed.status, 200, "{:?}", listed.body);
    assert_eq!(logins(&listed.body), vec!["admin".to_owned()]);
    assert_eq!(admin.get("/repos/admin/missing/assignees").status, 404);
    assert_eq!(bob.get("/repos/admin/secret/assignees").status, 404);
    let org = admin.get("/repos/acme/widgets/assignees");
    assert_eq!(org.status, 200, "{:?}", org.body);
    assert_eq!(
        logins(&org.body),
        vec!["admin".to_owned(), "member".to_owned()]
    );
}

#[test]
fn issues_check_user_can_be_assigned() {
    let server = Server::start().unwrap();
    server.add_user("bob");
    server.add_user("member");
    server.add_user("outsider");
    server
        .create_repository("admin", "project", false, None, None, "main")
        .unwrap();
    server.add_org("acme", &["admin", "member"]);
    server
        .create_repository("acme", "widgets", false, None, None, "main")
        .unwrap();
    let admin = common::Api::admin(&server);

    assert_eq!(
        admin.get("/repos/admin/project/assignees/admin").status,
        204
    );
    assert_eq!(admin.get("/repos/admin/project/assignees/bob").status, 404);
    assert_eq!(
        admin.get("/repos/acme/widgets/assignees/member").status,
        204
    );
    assert_eq!(
        admin.get("/repos/acme/widgets/assignees/outsider").status,
        404
    );
    assert_eq!(admin.get("/repos/acme/widgets/assignees/acme").status, 404);
    assert_eq!(
        admin.get("/repos/admin/missing/assignees/admin").status,
        404
    );
}

#[test]
fn issues_check_user_can_be_assigned_to_issue() {
    let server = Server::start().unwrap();
    server.add_user("bob");
    let repository = server
        .create_repository("admin", "project", false, None, None, "main")
        .unwrap();
    let admin = common::Api::admin(&server);
    let created = admin.post("/repos/admin/project/issues", json!({"title": "Assign"}));
    let number = created.body["number"].as_u64().unwrap();
    assert_eq!(
        admin
            .get(&format!(
                "/repos/admin/project/issues/{number}/assignees/admin"
            ))
            .status,
        204
    );
    assert_eq!(
        admin
            .get(&format!(
                "/repos/admin/project/issues/{number}/assignees/bob"
            ))
            .status,
        404
    );
    assert_eq!(
        admin
            .get("/repos/admin/project/issues/404/assignees/admin")
            .status,
        404
    );
    repository
        .create_pull_request(40, PullRequestSeed::default())
        .unwrap();
    assert_eq!(
        admin
            .get("/repos/admin/project/issues/40/assignees/admin")
            .status,
        204
    );
    assert_eq!(
        admin
            .get("/repos/admin/missing/issues/1/assignees/admin")
            .status,
        404
    );
}

#[test]
fn issues_set_labels() {
    let server = Server::start().unwrap();
    server.add_user("bob");
    let bob_token = server.user("bob").unwrap().add_token(&["repo"]);
    let repository = server
        .create_repository("admin", "project", false, None, None, "main")
        .unwrap();
    let admin = common::Api::admin(&server);
    let bob = common::Api::new(&server, Some(&bob_token));
    let created = admin.post("/repos/admin/project/issues", json!({"title": "Labels"}));
    let number = created.body["number"].as_u64().unwrap();
    let labels = format!("/repos/admin/project/issues/{number}/labels");
    assert_eq!(
        admin
            .post(
                "/repos/admin/project/labels",
                json!({"name": "bug", "color": "aaaaaa"})
            )
            .status,
        201
    );
    let set = admin.put(&labels, json!({"labels": ["bug"]}));
    assert_eq!(set.status, 200, "{:?}", set.body);
    assert_eq!(names(&set.body), vec!["bug".to_owned()]);
    assert_eq!(
        admin
            .get(&format!("/repos/admin/project/issues/{number}"))
            .body["labels"][0]["name"],
        "bug"
    );
    let unknown = admin.put(&labels, json!({"labels": ["missing"]}));
    assert_eq!(unknown.status, 422, "{:?}", unknown.body);
    assert_eq!(bob.put(&labels, json!({"labels": ["bug"]})).status, 403);
    assert_eq!(
        admin
            .put(
                "/repos/admin/project/issues/404/labels",
                json!({"labels": ["bug"]})
            )
            .status,
        404
    );
    repository
        .create_pull_request(40, PullRequestSeed::default())
        .unwrap();
    let pull = admin.put(
        "/repos/admin/project/issues/40/labels",
        json!({"labels": ["bug"]}),
    );
    assert_eq!(pull.status, 200, "{:?}", pull.body);
    assert_eq!(pull.body[0]["name"], "bug");
}

#[test]
fn issues_remove_all_labels() {
    let server = Server::start().unwrap();
    server.add_user("bob");
    let bob_token = server.user("bob").unwrap().add_token(&["repo"]);
    server
        .create_repository("admin", "project", false, None, None, "main")
        .unwrap();
    let admin = common::Api::admin(&server);
    let bob = common::Api::new(&server, Some(&bob_token));
    let created = admin.post("/repos/admin/project/issues", json!({"title": "Labels"}));
    let number = created.body["number"].as_u64().unwrap();
    let labels = format!("/repos/admin/project/issues/{number}/labels");
    admin.post(
        "/repos/admin/project/labels",
        json!({"name": "bug", "color": "aaaaaa"}),
    );
    admin.put(&labels, json!({"labels": ["bug"]}));
    assert_eq!(bob.delete(&labels).status, 403);
    let cleared = admin.delete(&labels);
    assert_eq!(cleared.status, 204, "{:?}", cleared.body);
    let remaining = admin.get(&labels);
    assert_eq!(remaining.status, 200, "{:?}", remaining.body);
    assert!(remaining.body.as_array().unwrap().is_empty());
    assert_eq!(
        admin
            .delete("/repos/admin/project/issues/404/labels")
            .status,
        404
    );
}

#[test]
fn issues_remove_label() {
    let server = Server::start().unwrap();
    server.add_user("bob");
    let bob_token = server.user("bob").unwrap().add_token(&["repo"]);
    let repository = server
        .create_repository("admin", "project", false, None, None, "main")
        .unwrap();
    let admin = common::Api::admin(&server);
    let bob = common::Api::new(&server, Some(&bob_token));
    let created = admin.post("/repos/admin/project/issues", json!({"title": "Labels"}));
    let number = created.body["number"].as_u64().unwrap();
    let labels = format!("/repos/admin/project/issues/{number}/labels");
    admin.post("/repos/admin/project/labels", json!({"name": "Bug"}));
    admin.post("/repos/admin/project/labels", json!({"name": "docs"}));
    admin.put(&labels, json!({"labels": ["Bug", "docs"]}));
    assert_eq!(bob.delete(&format!("{labels}/bug")).status, 403);
    assert_eq!(bob.delete(&format!("{labels}/absent")).status, 404);
    let removed = admin.delete(&format!("{labels}/bug"));
    assert_eq!(removed.status, 200, "{:?}", removed.body);
    assert_eq!(names(&removed.body), vec!["docs".to_owned()]);
    assert_eq!(admin.delete(&format!("{labels}/docs")).status, 200);
    assert_eq!(admin.delete(&format!("{labels}/docs")).status, 404);

    repository
        .create_pull_request(40, PullRequestSeed::default())
        .unwrap();
    admin.put(
        "/repos/admin/project/issues/40/labels",
        json!({"labels": ["docs"]}),
    );
    let pull = admin.delete("/repos/admin/project/issues/40/labels/docs");
    assert_eq!(pull.status, 200, "{:?}", pull.body);
    assert!(pull.body.as_array().unwrap().is_empty());
    assert_eq!(
        admin
            .delete("/repos/admin/project/issues/404/labels/docs")
            .status,
        404
    );
}

#[test]
fn issues_create_label() {
    let server = Server::start().unwrap();
    server.add_user("bob");
    let bob_token = server.user("bob").unwrap().add_token(&["repo"]);
    server
        .create_repository("admin", "project", false, None, None, "main")
        .unwrap();
    let admin = common::Api::admin(&server);
    let bob = common::Api::new(&server, Some(&bob_token));
    let created = admin.post("/repos/admin/project/issues", json!({"title": "Labels"}));
    let number = created.body["number"].as_u64().unwrap();

    let label = admin.post("/repos/admin/project/labels", json!({"name": "Bug"}));
    assert_eq!(label.status, 201, "{:?}", label.body);
    assert_eq!(label.body["name"], "Bug");
    assert_eq!(label.body["color"], "ededed");
    assert!(label.body["description"].is_null());
    assert_eq!(label.body["default"], false);
    assert_eq!(
        label.header("location"),
        Some(label.body["url"].as_str().unwrap())
    );
    let colored = admin.post(
        "/repos/admin/project/labels",
        json!({"name": "docs", "color": "ABCDEF", "description": "Notes"}),
    );
    assert_eq!(colored.status, 201, "{:?}", colored.body);
    assert_eq!(colored.body["color"], "abcdef");
    assert_eq!(colored.body["description"], "Notes");
    let listed = admin.get("/repos/admin/project/labels");
    assert_eq!(listed.status, 200, "{:?}", listed.body);
    assert!(names(&listed.body).contains(&"Bug".to_owned()));

    assert_eq!(
        admin
            .post("/repos/admin/project/labels", json!({"name": "bug"}))
            .status,
        422
    );
    assert_eq!(
        admin
            .post(
                "/repos/admin/project/labels",
                json!({"name": "bad", "color": "#ffffff"})
            )
            .status,
        422
    );
    assert_eq!(
        admin
            .post(
                "/repos/admin/project/labels",
                json!({"name": "bad", "color": "fff"})
            )
            .status,
        422
    );
    assert_eq!(
        admin
            .post(
                "/repos/admin/project/labels",
                json!({"name": "bad", "color": "zzzzzz"})
            )
            .status,
        422
    );
    assert_eq!(
        admin
            .post(
                "/repos/admin/project/labels",
                json!({"name": "bad", "description": "a".repeat(101)})
            )
            .status,
        422
    );
    assert_eq!(
        admin
            .post("/repos/admin/project/labels", json!({"name": "  "}))
            .status,
        422
    );
    assert_eq!(
        bob.post("/repos/admin/project/labels", json!({"name": "nope"}))
            .status,
        403
    );
    assert_eq!(
        admin
            .post("/repos/admin/missing/labels", json!({"name": "nope"}))
            .status,
        404
    );

    let derived = admin.post(
        &format!("/repos/admin/project/issues/{number}/labels"),
        json!({"labels": ["automation"]}),
    );
    assert_eq!(derived.status, 200, "{:?}", derived.body);
    assert_eq!(
        admin
            .post("/repos/admin/project/labels", json!({"name": "automation"}))
            .status,
        422
    );
}

#[test]
fn issues_get_label() {
    let server = Server::start().unwrap();
    server
        .create_repository("admin", "project", false, None, None, "main")
        .unwrap();
    let admin = common::Api::admin(&server);
    admin.post(
        "/repos/admin/project/labels",
        json!({"name": "Good Bug", "color": "123abc"}),
    );
    let exact = admin.get("/repos/admin/project/labels/Good%20Bug");
    assert_eq!(exact.status, 200, "{:?}", exact.body);
    assert_eq!(exact.body["name"], "Good Bug");
    assert_eq!(exact.body["color"], "123abc");
    let folded = admin.get("/repos/admin/project/labels/good%20bug");
    assert_eq!(folded.status, 200, "{:?}", folded.body);
    assert_eq!(folded.body["name"], "Good Bug");
    assert_eq!(admin.get("/repos/admin/project/labels/missing").status, 404);
    assert_eq!(admin.get("/repos/admin/missing/labels/bug").status, 404);
}

#[test]
fn issues_update_label() {
    let server = Server::start().unwrap();
    server.add_user("bob");
    let bob_token = server.user("bob").unwrap().add_token(&["repo"]);
    server
        .create_repository("admin", "project", false, None, None, "main")
        .unwrap();
    let admin = common::Api::admin(&server);
    let bob = common::Api::new(&server, Some(&bob_token));
    let first = admin.post("/repos/admin/project/issues", json!({"title": "One"}));
    let second = admin.post("/repos/admin/project/issues", json!({"title": "Two"}));
    let first_number = first.body["number"].as_u64().unwrap();
    let second_number = second.body["number"].as_u64().unwrap();
    admin.post(
        "/repos/admin/project/labels",
        json!({"name": "Bug", "color": "bbbbbb"}),
    );
    admin.post("/repos/admin/project/labels", json!({"name": "Other"}));
    admin.put(
        &format!("/repos/admin/project/issues/{first_number}/labels"),
        json!({"labels": ["Bug"]}),
    );

    let renamed = admin.patch(
        "/repos/admin/project/labels/bug",
        json!({"new_name": "Feature", "color": "ABCDEF", "description": "nice"}),
    );
    assert_eq!(renamed.status, 200, "{:?}", renamed.body);
    assert_eq!(renamed.body["name"], "Feature");
    assert_eq!(renamed.body["color"], "abcdef");
    assert_eq!(renamed.body["description"], "nice");
    let issue = admin.get(&format!("/repos/admin/project/issues/{first_number}"));
    assert_eq!(issue.body["labels"][0]["name"], "Feature");
    assert_eq!(issue.body["labels"][0]["color"], "abcdef");

    assert_eq!(
        admin
            .patch(
                "/repos/admin/project/labels/Feature",
                json!({"new_name": "Other"})
            )
            .status,
        422
    );
    let recased = admin.patch(
        "/repos/admin/project/labels/Feature",
        json!({"new_name": "feature"}),
    );
    assert_eq!(recased.status, 200, "{:?}", recased.body);
    assert_eq!(recased.body["name"], "feature");

    let archived = admin.patch(
        "/repos/admin/project/labels/feature",
        json!({"archived": true}),
    );
    assert_eq!(archived.status, 200, "{:?}", archived.body);
    assert!(archived.body["archived_at"].is_string());
    assert_eq!(archived.body["archived_by"]["login"], "admin");
    assert_eq!(
        admin
            .put(
                &format!("/repos/admin/project/issues/{second_number}/labels"),
                json!({"labels": ["feature"]})
            )
            .status,
        422
    );
    assert_eq!(
        admin
            .get(&format!("/repos/admin/project/issues/{first_number}"))
            .body["labels"][0]["name"],
        "feature"
    );
    assert_eq!(
        admin
            .put(
                &format!("/repos/admin/project/issues/{first_number}/labels"),
                json!({"labels": ["feature"]})
            )
            .status,
        200
    );
    assert_eq!(
        admin
            .patch(
                "/repos/admin/project/labels/feature",
                json!({"archived": false})
            )
            .status,
        200
    );
    let added = admin.put(
        &format!("/repos/admin/project/issues/{second_number}/labels"),
        json!({"labels": ["feature"]}),
    );
    assert_eq!(added.status, 200, "{:?}", added.body);
    assert_eq!(added.body[0]["name"], "feature");

    assert_eq!(
        bob.patch(
            "/repos/admin/project/labels/feature",
            json!({"color": "cccccc"})
        )
        .status,
        403
    );
    assert_eq!(
        bob.patch(
            "/repos/admin/project/labels/missing",
            json!({"color": "cccccc"})
        )
        .status,
        404
    );
    assert_eq!(
        admin
            .patch(
                "/repos/admin/project/labels/missing",
                json!({"color": "cccccc"})
            )
            .status,
        404
    );
}

#[test]
fn issues_delete_label() {
    let server = Server::start().unwrap();
    server.add_user("bob");
    let bob_token = server.user("bob").unwrap().add_token(&["repo"]);
    server
        .create_repository("admin", "project", false, None, None, "main")
        .unwrap();
    let admin = common::Api::admin(&server);
    let bob = common::Api::new(&server, Some(&bob_token));
    let created = admin.post("/repos/admin/project/issues", json!({"title": "Labels"}));
    let number = created.body["number"].as_u64().unwrap();
    admin.post("/repos/admin/project/labels", json!({"name": "bug"}));
    admin.post("/repos/admin/project/labels", json!({"name": "kept"}));
    admin.put(
        &format!("/repos/admin/project/issues/{number}/labels"),
        json!({"labels": ["bug"]}),
    );
    assert_eq!(bob.delete("/repos/admin/project/labels/kept").status, 403);
    assert_eq!(
        bob.delete("/repos/admin/project/labels/missing").status,
        404
    );
    assert_eq!(admin.delete("/repos/admin/project/labels/bug").status, 204);
    assert_eq!(admin.get("/repos/admin/project/labels/bug").status, 404);
    assert!(
        admin
            .get(&format!("/repos/admin/project/issues/{number}"))
            .body["labels"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(admin.delete("/repos/admin/project/labels/bug").status, 404);

    admin.post(
        &format!("/repos/admin/project/issues/{number}/labels"),
        json!({"labels": ["automation"]}),
    );
    assert_eq!(
        admin
            .delete("/repos/admin/project/labels/automation")
            .status,
        204
    );
    assert_eq!(
        admin.get("/repos/admin/project/labels/automation").status,
        404
    );
    assert!(
        admin
            .get(&format!("/repos/admin/project/issues/{number}/labels"))
            .body
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn issues_list_milestones() {
    let server = Server::start().unwrap();
    server.add_user("bob");
    let bob_token = server.user("bob").unwrap().add_token(&["repo"]);
    server
        .create_repository("admin", "project", false, None, None, "main")
        .unwrap();
    server
        .create_repository("admin", "secret", true, None, None, "main")
        .unwrap();
    let admin = common::Api::admin(&server);
    let bob = common::Api::new(&server, Some(&bob_token));
    let milestones = "/repos/admin/project/milestones";
    for (title, extra) in [
        ("m1", json!({"due_on": "2012-10-09T23:39:01Z"})),
        ("m2", json!({"due_on": "2020-01-01T00:00:00Z"})),
        ("m3", json!({})),
        ("m4", json!({"state": "closed"})),
    ] {
        let mut body = extra;
        body["title"] = json!(title);
        let created = admin.post(milestones, body);
        assert_eq!(created.status, 201, "{:?}", created.body);
    }

    let open = admin.get(milestones);
    assert_eq!(open.status, 200, "{:?}", open.body);
    assert_eq!(
        titles(&open.body),
        titles(&json!([{"title": "m1"}, {"title": "m2"}, {"title": "m3"}]))
    );
    let numbers = open
        .body
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["number"].as_i64().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(numbers, vec![1, 2, 3]);

    let closed = admin.get(&format!("{milestones}?state=closed"));
    assert_eq!(titles(&closed.body), titles(&json!([{"title": "m4"}])));
    let descending = admin.get(&format!("{milestones}?state=all&direction=desc"));
    assert_eq!(
        descending
            .body
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["number"].as_i64().unwrap())
            .collect::<Vec<_>>(),
        vec![2, 1, 4, 3]
    );
    let completeness = admin.get(&format!(
        "{milestones}?state=all&sort=completeness&direction=desc"
    ));
    assert_eq!(
        completeness
            .body
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["number"].as_i64().unwrap())
            .collect::<Vec<_>>(),
        vec![4, 3, 2, 1]
    );
    assert_eq!(admin.get(&format!("{milestones}?state=nope")).status, 422);
    assert_eq!(admin.get("/repos/admin/missing/milestones").status, 404);
    assert_eq!(bob.get("/repos/admin/secret/milestones").status, 404);
}

#[test]
fn issues_create_milestone() {
    let server = Server::start().unwrap();
    server.add_user("bob");
    let bob_token = server.user("bob").unwrap().add_token(&["repo"]);
    server
        .create_repository("admin", "project", false, None, None, "main")
        .unwrap();
    let admin = common::Api::admin(&server);
    let bob = common::Api::new(&server, Some(&bob_token));
    let created = admin.post(
        "/repos/admin/project/milestones",
        json!({"title": "  v1  ", "description": "notes"}),
    );
    assert_eq!(created.status, 201, "{:?}", created.body);
    assert_eq!(created.body["number"], 1);
    assert_eq!(created.body["title"], "v1");
    assert_eq!(created.body["state"], "open");
    assert_eq!(created.body["description"], "notes");
    assert_eq!(created.body["open_issues"], 0);
    assert_eq!(created.body["closed_issues"], 0);
    assert!(created.body["closed_at"].is_null());
    assert_eq!(created.body["creator"]["login"], "admin");
    let closed = admin.post(
        "/repos/admin/project/milestones",
        json!({"title": "done", "state": "closed"}),
    );
    assert_eq!(closed.status, 201, "{:?}", closed.body);
    assert!(closed.body["closed_at"].is_string());
    assert_eq!(
        admin
            .post("/repos/admin/project/milestones", json!({"title": " "}))
            .status,
        422
    );
    assert_eq!(
        admin
            .post("/repos/admin/project/milestones", json!({"title": "V1"}))
            .status,
        422
    );
    assert_eq!(
        bob.post("/repos/admin/project/milestones", json!({"title": "nope"}))
            .status,
        403
    );
    assert_eq!(
        admin
            .post("/repos/admin/missing/milestones", json!({"title": "nope"}))
            .status,
        404
    );
}

#[test]
fn issues_get_milestone() {
    let server = Server::start().unwrap();
    server
        .create_repository("admin", "project", false, None, None, "main")
        .unwrap();
    let admin = common::Api::admin(&server);
    let created = admin.post("/repos/admin/project/milestones", json!({"title": "v1"}));
    let number = created.body["number"].as_u64().unwrap();
    let got = admin.get(&format!("/repos/admin/project/milestones/{number}"));
    assert_eq!(got.status, 200, "{:?}", got.body);
    assert_eq!(got.body["title"], "v1");
    assert_eq!(admin.get("/repos/admin/project/milestones/99").status, 404);
    assert_eq!(admin.get("/repos/admin/missing/milestones/1").status, 404);
}

#[test]
fn issues_update_milestone() {
    let server = Server::start().unwrap();
    server.add_user("bob");
    let bob_token = server.user("bob").unwrap().add_token(&["repo"]);
    server
        .create_repository("admin", "project", false, None, None, "main")
        .unwrap();
    let admin = common::Api::admin(&server);
    let bob = common::Api::new(&server, Some(&bob_token));
    let first = admin.post("/repos/admin/project/milestones", json!({"title": "v1"}));
    let second = admin.post("/repos/admin/project/milestones", json!({"title": "v2"}));
    assert_eq!(first.status, 201, "{:?}", first.body);
    let number = first.body["number"].as_u64().unwrap();
    let milestone = format!("/repos/admin/project/milestones/{number}");
    let issue = admin.post("/repos/admin/project/issues", json!({"title": "Tracked"}));
    let issue_number = issue.body["number"].as_u64().unwrap();
    let issue_path = format!("/repos/admin/project/issues/{issue_number}");
    let assigned = admin.patch(&issue_path, json!({"milestone": number}));
    assert_eq!(assigned.status, 200, "{:?}", assigned.body);

    let closed = admin.patch(&milestone, json!({"title": "v1.1", "state": "closed"}));
    assert_eq!(closed.status, 200, "{:?}", closed.body);
    assert_eq!(closed.body["title"], "v1.1");
    assert_eq!(closed.body["state"], "closed");
    assert!(closed.body["closed_at"].is_string());
    assert_eq!(closed.body["open_issues"], 1);
    assert_eq!(closed.body["closed_issues"], 0);
    let embedded = admin.get(&issue_path);
    assert_eq!(embedded.body["milestone"]["title"], "v1.1");
    assert_eq!(embedded.body["milestone"]["state"], "closed");
    assert_eq!(embedded.body["milestone"]["open_issues"], 1);

    let reopened = admin.patch(&milestone, json!({"state": "open"}));
    assert_eq!(reopened.status, 200, "{:?}", reopened.body);
    assert!(reopened.body["closed_at"].is_null());
    assert_eq!(reopened.body["title"], "v1.1");
    let described = admin.patch(&milestone, json!({"description": "notes"}));
    assert_eq!(described.status, 200, "{:?}", described.body);
    assert_eq!(described.body["description"], "notes");
    assert_eq!(described.body["title"], "v1.1");
    assert_eq!(
        admin
            .patch(
                &milestone,
                json!({"title": second.body["title"].as_str().unwrap()})
            )
            .status,
        422
    );
    assert_eq!(admin.get(&milestone).body["title"], "v1.1");
    assert_eq!(
        bob.patch("/repos/admin/project/milestones/99", json!({"title": "x"}))
            .status,
        404
    );
    assert_eq!(bob.patch(&milestone, json!({"title": "x"})).status, 403);

    admin.patch(&issue_path, json!({"state": "closed"}));
    let counted = admin.get(&milestone);
    assert_eq!(counted.body["open_issues"], 0);
    assert_eq!(counted.body["closed_issues"], 1);
}

#[test]
fn issues_delete_milestone() {
    let server = Server::start().unwrap();
    server.add_user("bob");
    let bob_token = server.user("bob").unwrap().add_token(&["repo"]);
    server
        .create_repository("admin", "project", false, None, None, "main")
        .unwrap();
    let admin = common::Api::admin(&server);
    let bob = common::Api::new(&server, Some(&bob_token));
    let created = admin.post("/repos/admin/project/milestones", json!({"title": "v1"}));
    let number = created.body["number"].as_u64().unwrap();
    let milestone = format!("/repos/admin/project/milestones/{number}");
    let issue = admin.post("/repos/admin/project/issues", json!({"title": "Tracked"}));
    let issue_path = format!(
        "/repos/admin/project/issues/{}",
        issue.body["number"].as_u64().unwrap()
    );
    admin.patch(&issue_path, json!({"milestone": number}));
    assert_eq!(bob.delete(&milestone).status, 403);
    assert_eq!(bob.delete("/repos/admin/project/milestones/99").status, 404);
    assert_eq!(admin.delete(&milestone).status, 204);
    assert!(admin.get(&issue_path).body["milestone"].is_null());
    assert_eq!(admin.get(&milestone).status, 404);
    assert_eq!(admin.delete(&milestone).status, 404);
}

#[test]
fn issues_list_labels_for_milestone() {
    let server = Server::start().unwrap();
    server
        .create_repository("admin", "project", false, None, None, "main")
        .unwrap();
    let admin = common::Api::admin(&server);
    let milestone = admin.post("/repos/admin/project/milestones", json!({"title": "v1"}));
    let number = milestone.body["number"].as_u64().unwrap();
    admin.post("/repos/admin/project/labels", json!({"name": "zeta"}));
    admin.post("/repos/admin/project/labels", json!({"name": "Alpha"}));
    admin.post("/repos/admin/project/labels", json!({"name": "other"}));
    let inside = admin.post("/repos/admin/project/issues", json!({"title": "Inside"}));
    let outside = admin.post("/repos/admin/project/issues", json!({"title": "Outside"}));
    let inside_number = inside.body["number"].as_u64().unwrap();
    let outside_number = outside.body["number"].as_u64().unwrap();
    admin.patch(
        &format!("/repos/admin/project/issues/{inside_number}"),
        json!({"milestone": number, "labels": ["zeta", "Alpha"]}),
    );
    admin.patch(
        &format!("/repos/admin/project/issues/{outside_number}"),
        json!({"labels": ["other"]}),
    );
    let listed = admin.get(&format!("/repos/admin/project/milestones/{number}/labels"));
    assert_eq!(listed.status, 200, "{:?}", listed.body);
    assert_eq!(
        names(&listed.body),
        vec!["Alpha".to_owned(), "zeta".to_owned()]
    );
    assert_eq!(
        admin
            .get("/repos/admin/project/milestones/99/labels")
            .status,
        404
    );
}

#[test]
fn issues_get_comment() {
    let server = Server::start().unwrap();
    server.add_user("bob");
    let bob_token = server.user("bob").unwrap().add_token(&["repo"]);
    let repository = server
        .create_repository("admin", "project", false, None, None, "main")
        .unwrap();
    let secret = server
        .create_repository("admin", "secret", true, None, None, "main")
        .unwrap();
    server
        .create_repository("admin", "other", false, None, None, "main")
        .unwrap();
    let admin = common::Api::admin(&server);
    let bob = common::Api::new(&server, Some(&bob_token));
    let created = admin.post("/repos/admin/project/issues", json!({"title": "Talk"}));
    let number = created.body["number"].as_u64().unwrap();
    let comment = admin.post(
        &format!("/repos/admin/project/issues/{number}/comments"),
        json!({"body": "hello"}),
    );
    assert_eq!(comment.status, 201, "{:?}", comment.body);
    let id = comment.body["id"].as_i64().unwrap();
    let got = admin.get(&format!("/repos/admin/project/issues/comments/{id}"));
    assert_eq!(got.status, 200, "{:?}", got.body);
    assert_eq!(got.body["body"], "hello");
    assert_eq!(
        admin
            .get("/repos/admin/project/issues/comments/999999")
            .status,
        404
    );
    assert_eq!(
        admin
            .get(&format!("/repos/admin/other/issues/comments/{id}"))
            .status,
        404
    );

    secret
        .create_issue(1, IssueSeed::default())
        .unwrap()
        .create_comment(IssueCommentSeed {
            body: SeedField::present("hidden".to_owned()),
            ..Default::default()
        })
        .unwrap();
    let hidden = admin.get("/repos/admin/secret/issues/1/comments");
    assert_eq!(hidden.status, 200, "{:?}", hidden.body);
    let hidden_id = hidden.body[0]["id"].as_i64().unwrap();
    assert_eq!(
        bob.get(&format!("/repos/admin/secret/issues/comments/{hidden_id}"))
            .status,
        404
    );
    let _ = repository;
}

#[test]
fn issues_update_comment() {
    let server = Server::start().unwrap();
    server.add_user("bob");
    server.add_user("carol");
    let bob_token = server.user("bob").unwrap().add_token(&["repo"]);
    let carol_token = server.user("carol").unwrap().add_token(&["repo"]);
    let repository = server
        .create_repository("admin", "project", false, None, None, "main")
        .unwrap();
    let admin = common::Api::admin(&server);
    let bob = common::Api::new(&server, Some(&bob_token));
    let carol = common::Api::new(&server, Some(&carol_token));
    let issue = repository
        .create_issue(
            1,
            IssueSeed {
                title: SeedField::present("Talk".to_owned()),
                ..Default::default()
            },
        )
        .unwrap();
    issue
        .create_comment(IssueCommentSeed {
            body: SeedField::present("original".to_owned()),
            user: SeedField::present(Some(user("bob"))),
            updated_at: SeedField::present("2000-01-01T00:00:00Z".to_owned()),
            ..Default::default()
        })
        .unwrap();
    let listed = admin.get("/repos/admin/project/issues/1/comments");
    let id = listed.body[0]["id"].as_i64().unwrap();
    let path = format!("/repos/admin/project/issues/comments/{id}");
    assert_ne!(
        admin.get("/repos/admin/project/issues/1").body["updated_at"],
        listed.body[0]["updated_at"]
    );
    assert_eq!(carol.patch(&path, json!({"body": "no"})).status, 403);
    assert_eq!(admin.patch(&path, json!({})).status, 422);
    let edited = bob.patch(&path, json!({"body": "edited"}));
    assert_eq!(edited.status, 200, "{:?}", edited.body);
    assert_eq!(edited.body["body"], "edited");
    assert_ne!(edited.body["updated_at"], "2000-01-01T00:00:00Z");
    assert_eq!(
        admin.get("/repos/admin/project/issues/1").body["updated_at"],
        edited.body["updated_at"]
    );
    let admin_edit = admin.patch(&path, json!({"body": "admin edit"}));
    assert_eq!(admin_edit.status, 200, "{:?}", admin_edit.body);
    assert_eq!(admin_edit.body["body"], "admin edit");
    assert_eq!(
        admin
            .patch(
                "/repos/admin/project/issues/comments/999999",
                json!({"body": "no"})
            )
            .status,
        404
    );

    repository
        .create_pull_request(8, PullRequestSeed::default())
        .unwrap()
        .create_issue_comment(IssueCommentSeed {
            body: SeedField::present("on the pull".to_owned()),
            user: SeedField::present(Some(user("bob"))),
            ..Default::default()
        })
        .unwrap();
    let pull_comments = admin.get("/repos/admin/project/issues/8/comments");
    let pull_id = pull_comments.body[0]["id"].as_i64().unwrap();
    let pull_edit = bob.patch(
        &format!("/repos/admin/project/issues/comments/{pull_id}"),
        json!({"body": "pull edited"}),
    );
    assert_eq!(pull_edit.status, 200, "{:?}", pull_edit.body);
    assert_eq!(pull_edit.body["body"], "pull edited");
}

#[test]
fn issues_delete_comment() {
    let server = Server::start().unwrap();
    server.add_user("bob");
    server.add_user("carol");
    let bob_token = server.user("bob").unwrap().add_token(&["repo"]);
    let carol_token = server.user("carol").unwrap().add_token(&["repo"]);
    let repository = server
        .create_repository("admin", "project", false, None, None, "main")
        .unwrap();
    let admin = common::Api::admin(&server);
    let bob = common::Api::new(&server, Some(&bob_token));
    let carol = common::Api::new(&server, Some(&carol_token));
    let issue = repository.create_issue(1, IssueSeed::default()).unwrap();
    issue
        .create_comment(IssueCommentSeed {
            body: SeedField::present("bob's".to_owned()),
            user: SeedField::present(Some(user("bob"))),
            ..Default::default()
        })
        .unwrap();
    issue
        .create_comment(IssueCommentSeed {
            body: SeedField::present("carol's".to_owned()),
            user: SeedField::present(Some(user("carol"))),
            ..Default::default()
        })
        .unwrap();
    let listed = admin.get("/repos/admin/project/issues/1/comments");
    let bob_id = listed.body[0]["id"].as_i64().unwrap();
    let carol_id = listed.body[1]["id"].as_i64().unwrap();
    assert_eq!(
        carol
            .delete(&format!("/repos/admin/project/issues/comments/{bob_id}"))
            .status,
        403
    );
    assert_eq!(
        bob.delete(&format!("/repos/admin/project/issues/comments/{bob_id}"))
            .status,
        204
    );
    assert_eq!(
        admin
            .get(&format!("/repos/admin/project/issues/comments/{bob_id}"))
            .status,
        404
    );
    assert_eq!(
        admin.get("/repos/admin/project/issues/1").body["comments"],
        1
    );
    assert_eq!(
        admin
            .delete(&format!("/repos/admin/project/issues/comments/{carol_id}"))
            .status,
        204
    );
    assert_eq!(
        admin.get("/repos/admin/project/issues/1").body["comments"],
        0
    );
    assert_eq!(
        admin
            .delete("/repos/admin/project/issues/comments/999999")
            .status,
        404
    );

    let pull = repository
        .create_pull_request(8, PullRequestSeed::default())
        .unwrap();
    pull.create_issue_comment(IssueCommentSeed {
        body: SeedField::present("note".to_owned()),
        ..Default::default()
    })
    .unwrap();
    pull.create_review_comment(ReviewCommentSeed {
        path: SeedField::present("src/main.rs".to_owned()),
        body: SeedField::present("line".to_owned()),
        ..Default::default()
    })
    .unwrap();
    let pull_comment = admin.get("/repos/admin/project/issues/8/comments");
    let pull_id = pull_comment.body[0]["id"].as_i64().unwrap();
    let review = admin.get("/repos/admin/project/pulls/8/comments");
    let review_id = review.body[0]["id"].as_i64().unwrap();
    assert_eq!(
        admin
            .delete(&format!("/repos/admin/project/issues/comments/{review_id}"))
            .status,
        404
    );
    assert_eq!(
        admin.get("/repos/admin/project/pulls/8/comments").body[0]["id"],
        review_id
    );
    assert_eq!(
        admin
            .delete(&format!("/repos/admin/project/issues/comments/{pull_id}"))
            .status,
        204
    );
    assert_eq!(
        admin.get("/repos/admin/project/pulls/8").body["comments"],
        0
    );
}

#[test]
fn issues_list_comments_for_repo() {
    let server = Server::start().unwrap();
    server.add_user("bob");
    let bob_token = server.user("bob").unwrap().add_token(&["repo"]);
    let repository = server
        .create_repository("admin", "project", false, None, None, "main")
        .unwrap();
    server
        .create_repository("admin", "secret", true, None, None, "main")
        .unwrap();
    let admin = common::Api::admin(&server);
    let bob = common::Api::new(&server, Some(&bob_token));
    let issue = repository.create_issue(1, IssueSeed::default()).unwrap();
    let posted = admin.post(
        "/repos/admin/project/issues/1/comments",
        json!({"body": "new"}),
    );
    assert_eq!(posted.status, 201, "{:?}", posted.body);
    issue
        .create_comment(IssueCommentSeed {
            body: SeedField::present("old".to_owned()),
            created_at: SeedField::present("2000-01-01T00:00:00Z".to_owned()),
            updated_at: SeedField::present("2000-01-01T00:00:00Z".to_owned()),
            ..Default::default()
        })
        .unwrap();
    let pull = repository
        .create_pull_request(8, PullRequestSeed::default())
        .unwrap();
    pull.create_issue_comment(IssueCommentSeed {
        body: SeedField::present("pull-note".to_owned()),
        ..Default::default()
    })
    .unwrap();
    pull.create_review_comment(ReviewCommentSeed {
        path: SeedField::present("src/main.rs".to_owned()),
        body: SeedField::present("review-note".to_owned()),
        ..Default::default()
    })
    .unwrap();

    let comments = "/repos/admin/project/issues/comments";
    let listed = admin.get(comments);
    assert_eq!(listed.status, 200, "{:?}", listed.body);
    assert_eq!(
        bodies(&listed.body),
        vec!["new".to_owned(), "old".to_owned(), "pull-note".to_owned()]
    );
    let reversed = admin.get(&format!("{comments}?direction=desc"));
    assert_eq!(bodies(&reversed.body), bodies(&listed.body));
    let by_created = admin.get(&format!("{comments}?sort=created"));
    assert_eq!(
        bodies(&by_created.body),
        vec!["old".to_owned(), "new".to_owned(), "pull-note".to_owned()]
    );
    let by_updated = admin.get(&format!("{comments}?sort=updated&direction=desc"));
    assert_eq!(
        bodies(&by_updated.body),
        vec!["pull-note".to_owned(), "new".to_owned(), "old".to_owned()]
    );
    let since = admin.get(&format!("{comments}?since=2001-01-01T00:00:00Z"));
    assert_eq!(since.body.as_array().unwrap().len(), 2);
    assert!(!bodies(&since.body).contains(&"old".to_owned()));
    let future = admin.get(&format!("{comments}?since=2999-01-01T00:00:00Z"));
    assert!(future.body.as_array().unwrap().is_empty());
    assert_eq!(admin.get(&format!("{comments}?sort=nope")).status, 422);
    assert_eq!(
        admin.get(&format!("{comments}?since=yesterday")).status,
        422
    );
    assert_eq!(
        admin
            .get(&format!("{comments}?sort=created&direction=sideways"))
            .status,
        422
    );
    assert_eq!(
        admin.get("/repos/admin/missing/issues/comments").status,
        404
    );
    assert_eq!(bob.get("/repos/admin/secret/issues/comments").status, 404);
}

#[test]
fn issues_lock() {
    let server = Server::start().unwrap();
    server.add_user("bob");
    let bob_token = server.user("bob").unwrap().add_token(&["repo"]);
    let repository = server
        .create_repository("admin", "project", false, None, None, "main")
        .unwrap();
    let admin = common::Api::admin(&server);
    let bob = common::Api::new(&server, Some(&bob_token));
    let created = admin.post("/repos/admin/project/issues", json!({"title": "Talk"}));
    let number = created.body["number"].as_u64().unwrap();
    let issue = format!("/repos/admin/project/issues/{number}");
    let lock = format!("{issue}/lock");
    assert_eq!(admin.put(&lock, json!({})).status, 204);
    let locked = admin.get(&issue);
    assert_eq!(locked.body["locked"], true);
    assert!(locked.body["active_lock_reason"].is_null());
    assert_eq!(admin.put(&lock, json!({})).status, 422);
    assert_eq!(admin.get(&issue).body["locked"], true);
    assert_eq!(admin.delete(&lock).status, 204);
    let reason = admin.put(&lock, json!({"lock_reason": "off-topic"}));
    assert_eq!(reason.status, 204, "{:?}", reason.body);
    assert_eq!(admin.get(&issue).body["active_lock_reason"], "off-topic");
    assert_eq!(admin.put(&lock, json!({"lock_reason": "nope"})).status, 422);
    assert_eq!(admin.get(&issue).body["active_lock_reason"], "off-topic");
    assert_eq!(
        admin
            .put("/repos/admin/project/issues/404/lock", json!({}))
            .status,
        404
    );
    assert_eq!(bob.put(&lock, json!({})).status, 403);

    repository
        .create_pull_request(8, PullRequestSeed::default())
        .unwrap();
    assert_eq!(
        admin
            .put(
                "/repos/admin/project/issues/8/lock",
                json!({"lock_reason": "resolved"})
            )
            .status,
        204
    );
    let pull = admin.get("/repos/admin/project/pulls/8");
    assert_eq!(pull.body["locked"], true);
    assert_eq!(pull.body["active_lock_reason"], "resolved");
}

#[test]
fn issues_unlock() {
    let server = Server::start().unwrap();
    server.add_user("bob");
    let bob_token = server.user("bob").unwrap().add_token(&["repo"]);
    let repository = server
        .create_repository("admin", "project", false, None, None, "main")
        .unwrap();
    let admin = common::Api::admin(&server);
    let bob = common::Api::new(&server, Some(&bob_token));
    let created = admin.post("/repos/admin/project/issues", json!({"title": "Talk"}));
    let number = created.body["number"].as_u64().unwrap();
    let issue = format!("/repos/admin/project/issues/{number}");
    let lock = format!("{issue}/lock");
    assert_eq!(admin.delete(&lock).status, 204);
    assert_eq!(admin.get(&issue).body["locked"], false);
    admin.put(&lock, json!({"lock_reason": "spam"}));
    assert_eq!(bob.delete(&lock).status, 403);
    assert_eq!(admin.get(&issue).body["locked"], true);
    assert_eq!(admin.delete(&lock).status, 204);
    let unlocked = admin.get(&issue);
    assert_eq!(unlocked.body["locked"], false);
    assert!(unlocked.body["active_lock_reason"].is_null());
    assert_eq!(admin.delete(&lock).status, 204);
    assert_eq!(
        admin.delete("/repos/admin/project/issues/404/lock").status,
        404
    );

    repository
        .create_pull_request(8, PullRequestSeed::default())
        .unwrap();
    admin.put("/repos/admin/project/issues/8/lock", json!({}));
    assert_eq!(
        admin.delete("/repos/admin/project/issues/8/lock").status,
        204
    );
    assert_eq!(
        admin.get("/repos/admin/project/pulls/8").body["locked"],
        false
    );
}

#[test]
fn issues_list() {
    let server = Server::start().unwrap();
    server.add_user("bob");
    server.add_user("carol");
    let bob_token = server.user("bob").unwrap().add_token(&["repo"]);
    let carol_token = server.user("carol").unwrap().add_token(&["repo"]);
    let project = server
        .create_repository("admin", "project", false, None, None, "main")
        .unwrap();
    server
        .create_repository("bob", "tools", false, None, None, "main")
        .unwrap();
    server.add_org("acme", &["bob"]);
    server
        .create_repository("acme", "widgets", false, None, None, "main")
        .unwrap();
    server
        .create_repository("carol", "public", false, None, None, "main")
        .unwrap();
    let secret = server
        .create_repository("carol", "secret", true, None, None, "main")
        .unwrap();
    let bob = common::Api::new(&server, Some(&bob_token));
    let carol = common::Api::new(&server, Some(&carol_token));
    let anon = common::Api::new(&server, None);

    project
        .create_issue(
            1,
            IssueSeed {
                title: SeedField::present("assigned".to_owned()),
                assignees: SeedField::present(vec![user("bob")]),
                ..Default::default()
            },
        )
        .unwrap();
    project
        .create_issue(
            2,
            IssueSeed {
                title: SeedField::present("by-bob".to_owned()),
                user: SeedField::present(user("bob")),
                ..Default::default()
            },
        )
        .unwrap();
    project
        .create_issue(
            3,
            IssueSeed {
                title: SeedField::present("mention".to_owned()),
                body: SeedField::present(Some("ping @bob".to_owned())),
                ..Default::default()
            },
        )
        .unwrap();
    project
        .create_issue(
            4,
            IssueSeed {
                title: SeedField::present("almost".to_owned()),
                body: SeedField::present(Some("ping @bobby".to_owned())),
                ..Default::default()
            },
        )
        .unwrap();
    project
        .create_issue(
            5,
            IssueSeed {
                title: SeedField::present("shut".to_owned()),
                state: SeedField::present("closed".to_owned()),
                ..Default::default()
            },
        )
        .unwrap();
    project
        .create_issue(
            6,
            IssueSeed {
                title: SeedField::present("labeled".to_owned()),
                labels: SeedField::present(vec![LabelSeed {
                    name: SeedField::present("bug".to_owned()),
                    ..Default::default()
                }]),
                ..Default::default()
            },
        )
        .unwrap();
    project
        .create_pull_request(
            9,
            PullRequestSeed {
                title: SeedField::present("pr".to_owned()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(
        bob.post("/repos/bob/tools/issues", json!({"title": "tool"}))
            .status,
        201
    );
    assert_eq!(
        bob.post("/repos/acme/widgets/issues", json!({"title": "widget"}))
            .status,
        201
    );
    assert_eq!(
        carol
            .post(
                "/repos/carol/public/issues",
                json!({"title": "carol-public"})
            )
            .status,
        201
    );
    secret
        .create_issue(
            1,
            IssueSeed {
                title: SeedField::present("carol-secret".to_owned()),
                ..Default::default()
            },
        )
        .unwrap();

    assert_eq!(anon.get("/issues").status, 401);
    assert_eq!(
        titles(&bob.get("/issues").body),
        BTreeSet::from(["assigned".to_owned()])
    );
    assert_eq!(
        titles(&bob.get("/issues?filter=created").body),
        BTreeSet::from(["by-bob".to_owned(), "tool".to_owned(), "widget".to_owned()])
    );
    assert_eq!(
        titles(&bob.get("/issues?filter=mentioned").body),
        BTreeSet::from(["mention".to_owned()])
    );
    assert!(
        bob.get("/issues?filter=subscribed")
            .body
            .as_array()
            .unwrap()
            .is_empty()
    );
    let open = BTreeSet::from([
        "assigned".to_owned(),
        "by-bob".to_owned(),
        "mention".to_owned(),
        "almost".to_owned(),
        "labeled".to_owned(),
        "pr".to_owned(),
        "tool".to_owned(),
        "widget".to_owned(),
        "carol-public".to_owned(),
    ]);
    assert_eq!(titles(&bob.get("/issues?filter=all").body), open);
    let mut everything = open.clone();
    everything.insert("shut".to_owned());
    assert_eq!(
        titles(&bob.get("/issues?filter=all&state=all").body),
        everything
    );
    let mut without_pulls = everything.clone();
    without_pulls.remove("pr");
    assert_eq!(
        titles(&bob.get("/issues?filter=all&state=all&pulls=false").body),
        without_pulls
    );
    assert_eq!(
        titles(&bob.get("/issues?filter=all&state=closed").body),
        BTreeSet::from(["shut".to_owned()])
    );
    assert!(
        bob.get("/issues?filter=all&state=all&since=2999-01-01T00:00:00Z")
            .body
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        titles(
            &bob.get("/issues?filter=all&state=all&since=2000-01-01T00:00:00Z")
                .body
        ),
        everything
    );
    assert_eq!(bob.get("/issues?filter=nope").status, 422);
    assert_eq!(bob.get("/issues?filter=all&pulls=maybe").status, 422);
    assert_eq!(
        bob.get("/issues?filter=all&state=all&per_page=1")
            .body
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        titles(&bob.get("/issues?filter=all&labels=bug").body),
        BTreeSet::from(["labeled".to_owned()])
    );
    assert!(
        bob.get("/issues?filter=all&labels=bug,missing")
            .body
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        titles(&bob.get("/issues?filter=all&state=all&owned=true").body),
        BTreeSet::from(["tool".to_owned()])
    );
    assert_eq!(
        titles(&bob.get("/issues?filter=all&state=all&orgs=true").body),
        BTreeSet::from(["widget".to_owned()])
    );
    let mut collab = everything.clone();
    collab.remove("tool");
    collab.remove("widget");
    assert_eq!(
        titles(&bob.get("/issues?filter=all&state=all&collab=true").body),
        collab
    );
    assert!(
        bob.get("/issues?filter=all&state=all&owned=false")
            .body
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn issues_list_for_org() {
    let server = Server::start().unwrap();
    server.add_user("bob");
    let bob_token = server.user("bob").unwrap().add_token(&["repo"]);
    server.add_org("acme", &["bob"]);
    server
        .create_repository("acme", "widgets", false, None, None, "main")
        .unwrap();
    let bob = common::Api::new(&server, Some(&bob_token));
    let anon = common::Api::new(&server, None);
    assert_eq!(
        bob.post("/repos/acme/widgets/issues", json!({"title": "widget"}))
            .status,
        201
    );
    assert_eq!(anon.get("/orgs/missing/issues").status, 401);
    assert_eq!(
        titles(&bob.get("/orgs/acme/issues?filter=all").body),
        BTreeSet::from(["widget".to_owned()])
    );
    assert!(
        bob.get("/orgs/acme/issues?filter=all&type=bug")
            .body
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(bob.get("/orgs/admin/issues").status, 404);
    assert_eq!(bob.get("/orgs/missing/issues").status, 404);
    assert_eq!(bob.get("/orgs/acme/issues?filter=nope").status, 422);
}

#[test]
fn issues_list_for_authenticated_user() {
    let server = Server::start().unwrap();
    server.add_user("bob");
    let bob_token = server.user("bob").unwrap().add_token(&["repo"]);
    let project = server
        .create_repository("admin", "project", false, None, None, "main")
        .unwrap();
    server
        .create_repository("bob", "tools", false, None, None, "main")
        .unwrap();
    let bob = common::Api::new(&server, Some(&bob_token));
    let anon = common::Api::new(&server, None);
    project
        .create_issue(
            1,
            IssueSeed {
                title: SeedField::present("assigned".to_owned()),
                assignees: SeedField::present(vec![user("bob")]),
                ..Default::default()
            },
        )
        .unwrap();
    let created = bob.post("/repos/bob/tools/issues", json!({"title": "tool"}));
    assert_eq!(created.status, 201, "{:?}", created.body);
    let number = created.body["number"].as_u64().unwrap();
    assert_eq!(
        bob.post(
            &format!("/repos/bob/tools/issues/{number}/assignees"),
            json!({"assignees": ["bob"]})
        )
        .status,
        201
    );
    assert_eq!(anon.get("/user/issues").status, 401);
    assert_eq!(
        titles(&bob.get("/user/issues").body),
        BTreeSet::from(["tool".to_owned()])
    );
}
