//! Issue events: list for an issue, list for a repository, and get by id.

mod common;

use ninot_github::Server;
use ninot_github::fixtures::{PullRequestSeed, SeedField, TimelineEventSeed, UserSeed};
use serde_json::{Value, json};

fn kinds(body: &Value) -> Vec<String> {
    body.as_array()
        .unwrap()
        .iter()
        .map(|item| item["event"].as_str().unwrap().to_owned())
        .collect()
}

fn user(login: &str) -> UserSeed {
    UserSeed {
        login: SeedField::present(login.to_owned()),
        ..Default::default()
    }
}

#[test]
fn mutations_appear_in_the_issue_event_stream() {
    let server = Server::start().unwrap();
    server
        .create_repository("admin", "project", false, None, None, "main")
        .unwrap();
    let admin = common::Api::admin(&server);

    let created = admin.post(
        "/repos/admin/project/issues",
        json!({"title": "Ship it", "labels": ["fresh"]}),
    );
    assert_eq!(created.status, 201, "{:?}", created.body);
    let number = created.body["number"].as_u64().unwrap();
    let issue = format!("/repos/admin/project/issues/{number}");
    let events_path = format!("{issue}/events");

    let opened = admin.get(&events_path);
    assert_eq!(opened.status, 200, "{:?}", opened.body);
    assert_eq!(kinds(&opened.body), ["labeled"]);
    assert_eq!(opened.body[0]["label"]["name"], "fresh");
    assert!(opened.body[0].get("issue").is_none());
    assert_eq!(opened.body[0]["actor"]["login"], "admin");
    assert!(opened.body[0]["commit_id"].is_null());
    assert!(opened.body[0]["commit_url"].is_null());
    assert!(!opened.body[0]["node_id"].as_str().unwrap().is_empty());

    assert_eq!(
        admin
            .post(
                "/repos/admin/project/labels",
                json!({"name": "bug", "color": "abcdef"})
            )
            .status,
        201
    );
    let milestone = admin.post("/repos/admin/project/milestones", json!({"title": "v1"}));
    assert_eq!(milestone.status, 201, "{:?}", milestone.body);
    let milestone_number = milestone.body["number"].as_u64().unwrap();

    let closed = admin.patch(
        &issue,
        json!({
            "state": "closed",
            "labels": ["bug"],
            "assignees": ["admin"],
            "milestone": milestone_number,
        }),
    );
    assert_eq!(closed.status, 200, "{:?}", closed.body);

    let listed = admin.get(&events_path);
    assert_eq!(listed.status, 200, "{:?}", listed.body);
    assert_eq!(
        kinds(&listed.body),
        [
            "labeled",
            "unlabeled",
            "labeled",
            "assigned",
            "milestoned",
            "closed",
        ]
    );
    assert_eq!(listed.body[1]["label"]["name"], "fresh");
    assert_eq!(listed.body[2]["label"]["name"], "bug");
    assert_eq!(listed.body[2]["label"]["color"], "abcdef");
    assert_eq!(listed.body[3]["assignee"]["login"], "admin");
    assert_eq!(listed.body[3]["assigner"]["login"], "admin");
    assert_eq!(listed.body[4]["milestone"]["title"], "v1");
    assert_eq!(listed.body[5]["event"], "closed");

    let reopened = admin.patch(&issue, json!({"state": "open", "milestone": null}));
    assert_eq!(reopened.status, 200, "{:?}", reopened.body);
    let unassigned = admin.delete_json(
        &format!("{issue}/assignees"),
        json!({"assignees": ["admin"]}),
    );
    assert_eq!(unassigned.status, 200, "{:?}", unassigned.body);
    assert_eq!(admin.put(&format!("{issue}/lock"), json!({})).status, 204);
    assert_eq!(
        admin
            .put(
                &format!("{issue}/lock"),
                json!({"lock_reason": "off-topic"})
            )
            .status,
        422
    );
    assert_eq!(admin.delete(&format!("{issue}/lock")).status, 204);
    assert_eq!(
        admin
            .put(
                &format!("{issue}/lock"),
                json!({"lock_reason": "too heated"})
            )
            .status,
        204
    );

    let listed = admin.get(&events_path);
    assert_eq!(listed.status, 200, "{:?}", listed.body);
    let events = kinds(&listed.body);
    assert_eq!(
        &events[6..],
        [
            "demilestoned",
            "reopened",
            "unassigned",
            "locked",
            "unlocked",
            "locked",
        ]
    );
    assert_eq!(listed.body[6]["milestone"]["title"], "v1");
    assert_eq!(listed.body[8]["assignee"]["login"], "admin");
    assert!(listed.body[9]["lock_reason"].is_null());
    assert_eq!(listed.body[11]["lock_reason"], "too heated");

    assert_eq!(
        admin
            .post(&format!("{issue}/labels"), json!(["fresh"]))
            .status,
        200
    );
    assert_eq!(admin.delete(&format!("{issue}/labels/bug")).status, 200);
    assert_eq!(admin.delete(&format!("{issue}/labels")).status, 204);
    let listed = admin.get(&events_path);
    let events = kinds(&listed.body);
    assert_eq!(
        &events[events.len() - 3..],
        ["labeled", "unlabeled", "unlabeled"]
    );
    assert_eq!(listed.body[events.len() - 3]["label"]["name"], "fresh");
}

#[test]
fn list_for_repo_embeds_the_issue_and_get_event_reads_one() {
    let server = Server::start().unwrap();
    let admin = common::Api::admin(&server);
    server
        .create_repository("admin", "project", false, None, None, "main")
        .unwrap();

    let first = admin.post("/repos/admin/project/issues", json!({"title": "One"}));
    let second = admin.post("/repos/admin/project/issues", json!({"title": "Two"}));
    let first_number = first.body["number"].as_u64().unwrap();
    let second_number = second.body["number"].as_u64().unwrap();
    assert_eq!(
        admin
            .patch(
                &format!("/repos/admin/project/issues/{first_number}"),
                json!({"state": "closed"})
            )
            .status,
        200
    );
    assert_eq!(
        admin
            .patch(
                &format!("/repos/admin/project/issues/{second_number}"),
                json!({"state": "closed"})
            )
            .status,
        200
    );

    let repo = admin.get("/repos/admin/project/issues/events");
    assert_eq!(repo.status, 200, "{:?}", repo.body);
    assert_eq!(kinds(&repo.body), ["closed", "closed"]);
    assert_eq!(repo.body[0]["issue"]["title"], "One");
    assert_eq!(repo.body[0]["issue"]["number"], first_number);
    assert_eq!(repo.body[1]["issue"]["title"], "Two");
    assert!(
        repo.body[0]["url"]
            .as_str()
            .unwrap()
            .contains("/issues/events/")
    );

    let id = repo.body[0]["id"].as_u64().unwrap();
    let got = admin.get(&format!("/repos/admin/project/issues/events/{id}"));
    assert_eq!(got.status, 200, "{:?}", got.body);
    assert_eq!(got.body["id"], id);
    assert_eq!(got.body["event"], "closed");
    assert_eq!(got.body["issue"]["title"], "One");
    assert_eq!(got.body["actor"]["login"], "admin");

    let page = admin.get("/repos/admin/project/issues/events?per_page=1");
    assert_eq!(page.status, 200, "{:?}", page.body);
    assert_eq!(page.body.as_array().unwrap().len(), 1);
    assert_eq!(page.body[0]["issue"]["title"], "One");
    let link = page.header("link").unwrap();
    assert!(link.contains("rel=\"next\""), "{link}");
}

#[test]
fn pull_request_timeline_events_appear_as_issue_events() {
    let server = Server::start().unwrap();
    let repository = server
        .create_repository("admin", "project", false, None, None, "main")
        .unwrap();
    let admin = common::Api::admin(&server);
    let pull = repository
        .create_pull_request(
            1,
            PullRequestSeed {
                title: SeedField::present("Change".to_owned()),
                user: SeedField::present(user("alice")),
                created_at: SeedField::present("2026-01-02T10:00:00Z".to_owned()),
                ..Default::default()
            },
        )
        .unwrap();
    pull.create_timeline_event(TimelineEventSeed {
        event: "closed".to_owned(),
        created_at: "2026-01-02T11:00:00Z".to_owned(),
        actor: SeedField::present(user("admin")),
        ..Default::default()
    })
    .unwrap();

    let listed = admin.get("/repos/admin/project/issues/1/events");
    assert_eq!(listed.status, 200, "{:?}", listed.body);
    assert_eq!(kinds(&listed.body), ["closed"]);
    assert_eq!(listed.body[0]["actor"]["login"], "admin");
    assert!(listed.body[0].get("issue").is_none());

    assert_eq!(
        admin
            .put(
                "/repos/admin/project/issues/1/lock",
                json!({"lock_reason": "resolved"})
            )
            .status,
        204
    );
    let listed = admin.get("/repos/admin/project/issues/1/events");
    assert_eq!(kinds(&listed.body), ["closed", "locked"]);
    assert_eq!(listed.body[1]["lock_reason"], "resolved");

    let repo = admin.get("/repos/admin/project/issues/events");
    assert_eq!(repo.status, 200, "{:?}", repo.body);
    assert_eq!(kinds(&repo.body), ["closed", "locked"]);
    assert_eq!(repo.body[0]["issue"]["title"], "Change");
    assert!(repo.body[0]["issue"]["pull_request"].is_object());
}

#[test]
fn missing_issue_or_event_is_not_found() {
    let server = Server::start().unwrap();
    let admin = common::Api::admin(&server);
    server
        .create_repository("admin", "project", false, None, None, "main")
        .unwrap();
    assert_eq!(
        admin.get("/repos/admin/project/issues/404/events").status,
        404
    );
    assert_eq!(
        admin.get("/repos/admin/project/issues/events/404").status,
        404
    );
    assert_eq!(admin.get("/repos/admin/missing/issues/events").status, 404);
    let created = admin.post("/repos/admin/project/issues", json!({"title": "Alone"}));
    assert_eq!(created.status, 201, "{:?}", created.body);
    let empty = admin.get(&format!(
        "/repos/admin/project/issues/{}/events",
        created.body["number"].as_u64().unwrap()
    ));
    assert_eq!(empty.status, 200, "{:?}", empty.body);
    assert_eq!(empty.body.as_array().unwrap().len(), 0);
}
