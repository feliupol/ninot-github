//! Check runs and check suites.

mod common;

use std::path::Path;

use common::Api;
use ninot_github::fixtures::{CheckRunSeed, GitIdentitySeed, IntegrationSeed, SeedField};
use ninot_github::{Repository, Server};
use serde_json::{Value, json};

const APP_JWT: &str = "eyJhbGciOiJSUzI1NiJ9.eyJpc3MiOiIxIn0.signature";

fn app_token(server: &Server, installation: u64) -> String {
    let response = reqwest::blocking::Client::new()
        .post(format!(
            "{}/app/installations/{installation}/access_tokens",
            server.url()
        ))
        .bearer_auth(APP_JWT)
        .json(&json!({}))
        .send()
        .unwrap();
    assert_eq!(response.status(), 201, "{response:?}");
    let body: Value = response.json().unwrap();
    body["token"].as_str().unwrap().to_owned()
}

fn setup() -> (Server, Repository, String) {
    let server = Server::start().unwrap();
    let repo = server
        .create_repository("admin", "checks", false, None, None, "main")
        .unwrap();
    repo.create_installation(7).unwrap();
    let token = app_token(&server, 7);
    (server, repo, token)
}

fn outsider(server: &Server) -> Api<'_> {
    server.add_user("bob");
    Api::new(
        server,
        Some(&server.user("bob").unwrap().add_token(&["repo"])),
    )
}

fn create_run(api: &Api<'_>, name: &str, sha: &str, extra: Value) -> common::Reply {
    let mut body = json!({ "name": name, "head_sha": sha });
    for (key, value) in extra.as_object().unwrap() {
        body[key] = value.clone();
    }
    api.post("/repos/admin/checks/check-runs", body)
}

#[test]
fn checks_create_records_a_run_and_its_suite() {
    let (server, repo, token) = setup();
    let api = Api::new(&server, Some(&token));
    let admin = Api::admin(&server);
    let identity = GitIdentitySeed {
        name: "Test".to_owned(),
        email: "test@example.com".to_owned(),
        date: "2020-01-02T03:04:05Z".to_owned(),
    };
    let commit = repo
        .commit_file_by(
            "main",
            Path::new("README.md"),
            b"hello\n",
            "Add README",
            Some(&identity),
            None,
        )
        .unwrap();
    let created = create_run(
        &api,
        "tests",
        commit.sha(),
        json!({
            "status": "in_progress",
            "external_id": "42",
            "output": { "title": "Tests", "summary": "running" },
            "actions": [{
                "label": "Fix",
                "identifier": "fix_errors",
                "description": "Fix them"
            }]
        }),
    );
    assert_eq!(created.status, 201, "{:?}", created.body);
    assert_eq!(created.body["status"], "in_progress");
    assert!(created.body["conclusion"].is_null());
    assert!(created.body["completed_at"].is_null());
    assert!(created.body["started_at"].is_string());
    assert_eq!(created.body["head_sha"], commit.sha());
    assert_eq!(created.body["name"], "tests");
    assert_eq!(created.body["app"]["id"], 1);
    assert_eq!(created.body["output"]["title"], "Tests");
    let suite_id = created.body["check_suite"]["id"].as_i64().unwrap();
    let listed = api.get("/repos/admin/checks/commits/main/check-runs");
    assert_eq!(listed.status, 200, "{:?}", listed.body);
    assert_eq!(listed.body["check_runs"][0]["id"], created.body["id"]);
    let suites = api.get("/repos/admin/checks/commits/main/check-suites");
    assert_eq!(suites.body["check_suites"][0]["head_sha"], commit.sha());
    assert_eq!(suites.body["check_suites"][0]["head_branch"], "main");
    assert_eq!(
        suites.body["check_suites"][0]["before"],
        "0000000000000000000000000000000000000000"
    );
    assert_eq!(
        suites.body["check_suites"][0]["head_commit"]["message"],
        "Add README"
    );
    assert_eq!(
        suites.body["check_suites"][0]["head_commit"]["author"]["email"],
        "test@example.com"
    );
    assert_eq!(suites.body["check_suites"][0]["id"], suite_id);

    let denied = admin.post(
        "/repos/admin/checks/check-runs",
        json!({ "name": "tests", "head_sha": "abc" }),
    );
    assert_eq!(denied.status, 403, "{:?}", denied.body);
    assert_eq!(
        denied.body["message"],
        "Resource not accessible by personal access token"
    );
    assert_eq!(
        api.post(
            "/repos/admin/missing/check-runs",
            json!({ "name": "tests", "head_sha": "abc" })
        )
        .status,
        404
    );
    assert_eq!(
        Api::new(&server, None)
            .post(
                "/repos/admin/checks/check-runs",
                json!({ "name": "tests", "head_sha": "abc" })
            )
            .status,
        401
    );
    assert_eq!(
        api.post(
            "/repos/admin/checks/check-runs",
            json!({ "name": "tests", "head_sha": "abc", "conclusion": "stale" })
        )
        .status,
        422
    );
    assert_eq!(
        api.post(
            "/repos/admin/checks/check-runs",
            json!({
                "name": "tests",
                "head_sha": "abc",
                "status": "queued",
                "completed_at": "2020-01-02T03:04:05Z"
            })
        )
        .status,
        422
    );
}

#[test]
fn checks_update_appends_annotations_and_completes() {
    let (server, repo, token) = setup();
    repo.create_app_installation(8, 2).unwrap();
    let other = app_token(&server, 8);
    let api = Api::new(&server, Some(&token));
    let created = create_run(&api, "lint", "abc", json!({}));
    assert_eq!(created.status, 201, "{:?}", created.body);
    let id = created.body["id"].as_i64().unwrap();
    let path = format!("/repos/admin/checks/check-runs/{id}");
    let columns = api.patch(
        &path,
        json!({
            "output": {
                "title": "Lint",
                "summary": "one",
                "annotations": [{
                    "path": "README.md",
                    "annotation_level": "warning",
                    "message": "wrap this",
                    "start_line": 1,
                    "end_line": 3,
                    "start_column": 1
                }]
            }
        }),
    );
    assert_eq!(columns.status, 422, "{:?}", columns.body);
    let updated = api.patch(
        &path,
        json!({
            "conclusion": "failure",
            "output": {
                "title": "Lint",
                "summary": "failed",
                "annotations": [{
                    "path": "README.md",
                    "annotation_level": "failure",
                    "message": "syntax",
                    "title": "Parse",
                    "start_line": 2,
                    "end_line": 2,
                    "start_column": 1,
                    "end_column": 4
                }]
            }
        }),
    );
    assert_eq!(updated.status, 200, "{:?}", updated.body);
    assert_eq!(updated.body["status"], "completed");
    assert_eq!(updated.body["conclusion"], "failure");
    assert!(updated.body["completed_at"].is_string());
    assert_eq!(updated.body["output"]["annotations_count"], 1);
    assert_eq!(updated.body["output"]["title"], "Lint");
    let again = api.patch(
        &path,
        json!({
            "output": {
                "summary": "still failed",
                "annotations": [{
                    "path": "README.md",
                    "annotation_level": "notice",
                    "message": "style",
                    "start_line": 4,
                    "end_line": 4
                }]
            }
        }),
    );
    assert_eq!(again.status, 200, "{:?}", again.body);
    assert_eq!(again.body["conclusion"], "failure");
    assert_eq!(again.body["output"]["annotations_count"], 2);
    assert_eq!(again.body["output"]["title"], "Lint");
    let forbidden = Api::new(&server, Some(&other)).patch(&path, json!({ "name": "nope" }));
    assert_eq!(forbidden.status, 403, "{:?}", forbidden.body);
    assert_eq!(
        forbidden.body["message"],
        "Resource not accessible by integration"
    );
    assert_eq!(
        Api::admin(&server)
            .patch(&path, json!({ "name": "nope" }))
            .status,
        403
    );
    assert_eq!(
        api.patch("/repos/admin/checks/check-runs/999", json!({}))
            .status,
        404
    );
    assert_eq!(
        api.patch("/repos/nobody/checks/check-runs/1", json!({}))
            .status,
        404
    );
}

#[test]
fn checks_list_annotations_paginates() {
    let (server, _repo, token) = setup();
    let api = Api::new(&server, Some(&token));
    let created = create_run(
        &api,
        "lint",
        "abc",
        json!({
            "output": {
                "title": "Lint",
                "summary": "notes",
                "annotations": [
                    {
                        "path": "a.rs",
                        "annotation_level": "notice",
                        "message": "first",
                        "start_line": 1,
                        "end_line": 1
                    },
                    {
                        "path": "b.rs",
                        "annotation_level": "warning",
                        "message": "second",
                        "start_line": 2,
                        "end_line": 2
                    }
                ]
            }
        }),
    );
    assert_eq!(created.status, 201, "{:?}", created.body);
    let id = created.body["id"].as_i64().unwrap();
    let page = api.get(&format!(
        "/repos/admin/checks/check-runs/{id}/annotations?per_page=1"
    ));
    assert_eq!(page.status, 200, "{:?}", page.body);
    assert_eq!(page.body.as_array().unwrap().len(), 1);
    assert_eq!(page.body[0]["message"], "first");
    assert!(page.header("link").unwrap().contains("rel=\"next\""));
    let next = api.get(&format!(
        "/repos/admin/checks/check-runs/{id}/annotations?per_page=1&page=2"
    ));
    assert_eq!(next.body[0]["message"], "second");
    assert_eq!(
        api.get("/repos/admin/checks/check-runs/999/annotations")
            .status,
        404
    );
    let hidden = server
        .create_repository("admin", "hidden", true, None, None, "main")
        .unwrap();
    hidden.create_installation(9).unwrap();
    assert_eq!(
        outsider(&server)
            .get("/repos/admin/hidden/check-runs/1/annotations")
            .status,
        404
    );
}

#[test]
fn checks_rerequest_run_resets_the_suite_only() {
    let (server, repo, token) = setup();
    repo.create_app_installation(8, 2).unwrap();
    let other = app_token(&server, 8);
    let api = Api::new(&server, Some(&token));
    let created = create_run(
        &api,
        "tests",
        "abc",
        json!({ "conclusion": "success", "completed_at": "2020-01-02T03:04:05Z" }),
    );
    assert_eq!(created.status, 201, "{:?}", created.body);
    let id = created.body["id"].as_i64().unwrap();
    let suite_id = created.body["check_suite"]["id"].as_i64().unwrap();
    let rerequested = api.post(
        &format!("/repos/admin/checks/check-runs/{id}/rerequest"),
        json!({}),
    );
    assert_eq!(rerequested.status, 201, "{:?}", rerequested.body);
    assert_eq!(rerequested.body, json!({}));
    let run = api.get(&format!("/repos/admin/checks/check-runs/{id}"));
    assert_eq!(run.body["status"], "completed");
    assert_eq!(run.body["conclusion"], "success");
    let suite = api.get(&format!("/repos/admin/checks/check-suites/{suite_id}"));
    assert_eq!(suite.body["status"], "queued");
    assert!(suite.body["conclusion"].is_null());
    let renamed = api.patch(
        &format!("/repos/admin/checks/check-runs/{id}"),
        json!({ "name": "tests-2" }),
    );
    assert_eq!(renamed.status, 200, "{:?}", renamed.body);
    assert_eq!(renamed.body["conclusion"], "success");
    let restored = api.get(&format!("/repos/admin/checks/check-suites/{suite_id}"));
    assert_eq!(restored.body["status"], "completed");
    assert_eq!(restored.body["conclusion"], "success");

    assert_eq!(
        Api::new(&server, Some(&other))
            .post(
                &format!("/repos/admin/checks/check-runs/{id}/rerequest"),
                json!({})
            )
            .status,
        403
    );
    assert_eq!(
        Api::admin(&server)
            .post(
                &format!("/repos/admin/checks/check-runs/{id}/rerequest"),
                json!({})
            )
            .status,
        403
    );
    assert_eq!(
        api.post("/repos/admin/checks/check-runs/999/rerequest", json!({}))
            .status,
        404
    );

    let seeded = repo.commit("seeded").unwrap();
    seeded
        .create_check_run(CheckRunSeed {
            name: SeedField::present("orphan".to_owned()),
            app: SeedField::present(Some(IntegrationSeed {
                id: SeedField::present(1),
                external_url: SeedField::present("https://example.com".to_owned()),
                html_url: SeedField::present("https://example.com/apps/ninot-test".to_owned()),
                ..Default::default()
            })),
            ..Default::default()
        })
        .unwrap();
    let listed = api.get("/repos/admin/checks/commits/seeded/check-runs?filter=all");
    assert_eq!(listed.status, 200, "{:?}", listed.body);
    let orphan = listed.body["check_runs"][0]["id"].as_i64().unwrap();
    let blocked = api.post(
        &format!("/repos/admin/checks/check-runs/{orphan}/rerequest"),
        json!({}),
    );
    assert_eq!(blocked.status, 422, "{:?}", blocked.body);
}

#[test]
fn checks_create_suite_is_idempotent_per_app() {
    let (server, repo, token) = setup();
    repo.create_app_installation(8, 2).unwrap();
    let other = app_token(&server, 8);
    let api = Api::new(&server, Some(&token));
    assert_eq!(
        Api::admin(&server)
            .patch(
                "/repos/admin/checks/check-suites/preferences",
                json!({ "auto_trigger_checks": [{ "app_id": 1, "setting": false }] }),
            )
            .status,
        200
    );
    let created = api.post(
        "/repos/admin/checks/check-suites",
        json!({ "head_sha": "abc" }),
    );
    assert_eq!(created.status, 201, "{:?}", created.body);
    assert_eq!(created.body["head_sha"], "abc");
    assert_eq!(created.body["status"], "queued");
    assert!(created.body["conclusion"].is_null());
    assert_eq!(created.body["latest_check_runs_count"], 0);
    let again = api.post(
        "/repos/admin/checks/check-suites",
        json!({ "head_sha": "abc" }),
    );
    assert_eq!(again.status, 200, "{:?}", again.body);
    assert_eq!(again.body["id"], created.body["id"]);
    let second = Api::new(&server, Some(&other)).post(
        "/repos/admin/checks/check-suites",
        json!({ "head_sha": "abc" }),
    );
    assert_eq!(second.status, 201, "{:?}", second.body);
    assert_ne!(second.body["id"], created.body["id"]);
    assert_eq!(
        Api::admin(&server)
            .post(
                "/repos/admin/checks/check-suites",
                json!({ "head_sha": "abc" })
            )
            .status,
        403
    );
    assert_eq!(
        api.post(
            "/repos/admin/missing/check-suites",
            json!({ "head_sha": "abc" })
        )
        .status,
        404
    );
    assert_eq!(
        Api::new(&server, None)
            .post(
                "/repos/admin/checks/check-suites",
                json!({ "head_sha": "abc" })
            )
            .status,
        401
    );
}

#[test]
fn checks_set_suites_preferences_requires_admin() {
    let (server, _repo, _token) = setup();
    let admin = Api::admin(&server);
    let saved = admin.patch(
        "/repos/admin/checks/check-suites/preferences",
        json!({
            "auto_trigger_checks": [
                { "app_id": 2, "setting": true },
                { "app_id": 1, "setting": false },
                { "app_id": 1, "setting": true }
            ]
        }),
    );
    assert_eq!(saved.status, 200, "{:?}", saved.body);
    assert_eq!(
        saved.body["preferences"]["auto_trigger_checks"][0]["app_id"],
        1
    );
    assert!(
        saved.body["preferences"]["auto_trigger_checks"][0]["setting"]
            .as_bool()
            .unwrap()
    );
    assert_eq!(
        saved.body["preferences"]["auto_trigger_checks"][1]["app_id"],
        2
    );
    assert_eq!(saved.body["repository"]["name"], "checks");
    let narrowed = admin.patch(
        "/repos/admin/checks/check-suites/preferences",
        json!({ "auto_trigger_checks": [{ "app_id": 2, "setting": false }] }),
    );
    assert_eq!(narrowed.status, 200, "{:?}", narrowed.body);
    assert!(
        narrowed.body["preferences"]["auto_trigger_checks"][0]["setting"]
            .as_bool()
            .unwrap()
    );
    assert!(
        !narrowed.body["preferences"]["auto_trigger_checks"][1]["setting"]
            .as_bool()
            .unwrap()
    );
    let echoed = admin.patch("/repos/admin/checks/check-suites/preferences", json!({}));
    assert_eq!(
        echoed.body["preferences"]["auto_trigger_checks"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        outsider(&server)
            .patch(
                "/repos/admin/checks/check-suites/preferences",
                json!({ "auto_trigger_checks": [{ "app_id": 1, "setting": false }] })
            )
            .status,
        403
    );
    assert_eq!(
        Api::new(&server, None)
            .patch("/repos/admin/checks/check-suites/preferences", json!({}))
            .status,
        401
    );
    assert_eq!(
        admin
            .patch("/repos/admin/missing/check-suites/preferences", json!({}))
            .status,
        404
    );
}

#[test]
fn checks_get_suite_reads_rollup() {
    let (server, _repo, token) = setup();
    let api = Api::new(&server, Some(&token));
    let created = api.post(
        "/repos/admin/checks/check-suites",
        json!({ "head_sha": "abc" }),
    );
    assert_eq!(created.status, 201, "{:?}", created.body);
    let id = created.body["id"].as_i64().unwrap();
    create_run(&api, "lint", "abc", json!({ "conclusion": "success" }));
    create_run(&api, "build", "abc", json!({ "conclusion": "failure" }));
    let suite = api.get(&format!("/repos/admin/checks/check-suites/{id}"));
    assert_eq!(suite.status, 200, "{:?}", suite.body);
    assert_eq!(suite.body["status"], "completed");
    assert_eq!(suite.body["conclusion"], "failure");
    assert_eq!(suite.body["latest_check_runs_count"], 2);
    assert_eq!(suite.body["app"]["slug"], "ninot-test");
    assert_eq!(api.get("/repos/admin/checks/check-suites/999").status, 404);
    server
        .create_repository("admin", "hidden", true, None, None, "main")
        .unwrap();
    assert_eq!(
        outsider(&server)
            .get(&format!("/repos/admin/hidden/check-suites/{id}"))
            .status,
        404
    );
    assert_eq!(api.get("/repos/nobody/checks/check-suites/1").status, 404);
}

#[test]
fn checks_list_for_suite_filters_and_paginates() {
    let (server, _repo, token) = setup();
    let api = Api::new(&server, Some(&token));
    let created = api.post(
        "/repos/admin/checks/check-suites",
        json!({ "head_sha": "abc" }),
    );
    let id = created.body["id"].as_i64().unwrap();
    let first = create_run(&api, "lint", "abc", json!({ "status": "queued" }));
    let second = create_run(&api, "lint", "abc", json!({ "conclusion": "success" }));
    create_run(&api, "build", "abc", json!({ "status": "in_progress" }));
    let path = format!("/repos/admin/checks/check-suites/{id}/check-runs");
    let latest = api.get(&path);
    assert_eq!(latest.status, 200, "{:?}", latest.body);
    assert_eq!(latest.body["total_count"], 2);
    let names: Vec<_> = latest.body["check_runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|run| run["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["lint", "build"]);
    assert_eq!(
        latest.body["check_runs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|run| run["name"] == "lint")
            .unwrap()["id"],
        second.body["id"]
    );
    let all = api.get(&format!("{path}?filter=all"));
    assert_eq!(all.body["total_count"], 3);
    let queued = api.get(&format!("{path}?status=queued&filter=all"));
    assert_eq!(queued.body["total_count"], 1);
    assert_eq!(queued.body["check_runs"][0]["id"], first.body["id"]);
    let named = api.get(&format!("{path}?check_name=build"));
    assert_eq!(named.body["total_count"], 1);
    assert_eq!(named.body["check_runs"][0]["name"], "build");
    let page = api.get(&format!("{path}?filter=all&per_page=1"));
    assert_eq!(page.body["check_runs"].as_array().unwrap().len(), 1);
    assert_eq!(page.body["total_count"], 3);
    assert!(page.header("link").unwrap().contains("rel=\"next\""));
    assert_eq!(
        api.get("/repos/admin/checks/check-suites/999/check-runs")
            .status,
        404
    );
    server
        .create_repository("admin", "hidden", true, None, None, "main")
        .unwrap();
    assert_eq!(
        outsider(&server)
            .get("/repos/admin/hidden/check-suites/1/check-runs")
            .status,
        404
    );
}

#[test]
fn checks_rerequest_suite_resets_status() {
    let (server, repo, token) = setup();
    repo.create_app_installation(8, 2).unwrap();
    let other = app_token(&server, 8);
    let api = Api::new(&server, Some(&token));
    let created = create_run(&api, "tests", "abc", json!({ "conclusion": "success" }));
    let suite_id = created.body["check_suite"]["id"].as_i64().unwrap();
    let path = format!("/repos/admin/checks/check-suites/{suite_id}/rerequest");
    let rerequested = api.post(&path, json!({}));
    assert_eq!(rerequested.status, 201, "{:?}", rerequested.body);
    assert_eq!(rerequested.body, json!({}));
    let suite = api.get(&format!("/repos/admin/checks/check-suites/{suite_id}"));
    assert_eq!(suite.body["status"], "queued");
    assert!(suite.body["conclusion"].is_null());
    let run = api.get(&format!(
        "/repos/admin/checks/check-runs/{}",
        created.body["id"]
    ));
    assert_eq!(run.body["conclusion"], "success");
    assert_eq!(
        Api::new(&server, Some(&other))
            .post(&path, json!({}))
            .status,
        403
    );
    assert_eq!(Api::admin(&server).post(&path, json!({})).status, 403);
    assert_eq!(
        api.post("/repos/admin/checks/check-suites/999/rerequest", json!({}))
            .status,
        404
    );
    assert_eq!(
        api.post("/repos/nobody/checks/check-suites/1/rerequest", json!({}))
            .status,
        404
    );
}

#[test]
fn checks_list_suites_for_ref_filters() {
    let (server, repo, token) = setup();
    repo.create_app_installation(8, 2).unwrap();
    let other = app_token(&server, 8);
    let api = Api::new(&server, Some(&token));
    let first = api.post(
        "/repos/admin/checks/check-suites",
        json!({ "head_sha": "abc" }),
    );
    assert_eq!(first.status, 201, "{:?}", first.body);
    let second = Api::new(&server, Some(&other)).post(
        "/repos/admin/checks/check-suites",
        json!({ "head_sha": "abc" }),
    );
    assert_eq!(second.status, 201, "{:?}", second.body);
    create_run(&api, "lint", "abc", json!({}));
    let listed = api.get("/repos/admin/checks/commits/abc/check-suites");
    assert_eq!(listed.status, 200, "{:?}", listed.body);
    assert_eq!(listed.body["total_count"], 2);
    let only_app = api.get("/repos/admin/checks/commits/abc/check-suites?app_id=2");
    assert_eq!(only_app.body["total_count"], 1);
    assert_eq!(only_app.body["check_suites"][0]["id"], second.body["id"]);
    let named = api.get("/repos/admin/checks/commits/abc/check-suites?check_name=lint");
    assert_eq!(named.body["total_count"], 1);
    assert_eq!(named.body["check_suites"][0]["id"], first.body["id"]);
    let page = api.get("/repos/admin/checks/commits/abc/check-suites?per_page=1");
    assert_eq!(page.body["check_suites"].as_array().unwrap().len(), 1);
    assert_eq!(page.body["total_count"], 2);
    assert!(page.header("link").unwrap().contains("rel=\"next\""));
    let missing = api.get("/repos/admin/checks/commits/missing-ref/check-suites");
    assert_eq!(missing.status, 200, "{:?}", missing.body);
    assert_eq!(missing.body["total_count"], 0);
    assert_eq!(
        api.get("/repos/nobody/checks/commits/abc/check-suites")
            .status,
        404
    );
    server
        .create_repository("admin", "hidden", true, None, None, "main")
        .unwrap();
    assert_eq!(
        outsider(&server)
            .get("/repos/admin/hidden/commits/abc/check-suites")
            .status,
        404
    );
}
