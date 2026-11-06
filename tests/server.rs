//! Protocol-level behaviour of the HTTP server.

mod common;

use ninot_github::Server;
use reqwest::blocking::Client;
use serde_json::{Value, json};

fn get(server: &Server, path: &str, token: Option<&str>) -> reqwest::blocking::Response {
    let mut request = Client::new().get(format!("{}{path}", server.url()));
    if let Some(token) = token {
        request = request.header("Authorization", format!("token {token}"));
    }
    request.send().unwrap()
}

#[test]
fn implemented_operation_is_served_at_root_and_enterprise_prefix() {
    let server = Server::start().unwrap();
    for path in ["/zen", "/api/v3/zen"] {
        let response = get(&server, path, None);
        assert_eq!(response.status(), 200, "{path}");
        assert_eq!(response.text().unwrap(), "Keep it logically awesome.");
    }
}

#[test]
fn unimplemented_operation_names_its_operation_id() {
    let server = Server::start().unwrap();
    let response = get(&server, "/repos/o/r/actions/workflows", None);
    assert_eq!(response.status(), 501);
    let body: Value = response.json().unwrap();
    assert_eq!(
        body["message"],
        "Not implemented by ninot-github: actions/list-repo-workflows"
    );
    assert_eq!(body["status"], "501");
    let mut keys: Vec<&String> = body.as_object().unwrap().keys().collect();
    keys.sort();
    assert_eq!(keys, ["documentation_url", "message", "status"]);
}

#[test]
fn unknown_path_is_github_shaped_404() {
    let server = Server::start().unwrap();
    let response = get(&server, "/no/such/thing", None);
    assert_eq!(response.status(), 404);
    let body: Value = response.json().unwrap();
    assert_eq!(
        body,
        json!({
            "message": "Not Found",
            "documentation_url": "https://docs.github.com/rest",
            "status": "404",
        })
    );
}

#[test]
fn bad_token_is_rejected_and_good_token_reports_scopes() {
    let server = Server::start().unwrap();
    let response = get(&server, "/rate_limit", Some("ghp_wrong"));
    assert_eq!(response.status(), 401);
    let body: Value = response.json().unwrap();
    assert_eq!(body["message"], "Bad credentials");
    assert_eq!(body["status"], "401");

    let response = get(&server, "/rate_limit", Some(server.admin_token()));
    assert_eq!(response.status(), 200);
    let scopes = response.headers()["x-oauth-scopes"]
        .to_str()
        .unwrap()
        .to_owned();
    assert!(
        scopes.split(", ").any(|scope| scope == "delete_repo"),
        "{scopes}"
    );
}

#[test]
fn basic_auth_accepts_the_token_as_password() {
    let server = Server::start().unwrap();
    let response = Client::new()
        .get(format!("{}/rate_limit", server.url()))
        .basic_auth("anyone", Some(server.admin_token()))
        .send()
        .unwrap();
    assert_eq!(response.status(), 200);
    assert!(response.headers().contains_key("x-oauth-scopes"));
}

#[test]
fn validation_works_outside_source_tree() {
    let original_dir = std::env::current_dir().unwrap();
    let tmp_name = format!(
        "ninot-github-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let tmp = std::env::temp_dir().join(&tmp_name);
    std::fs::create_dir_all(&tmp).unwrap();
    std::env::set_current_dir(&tmp).unwrap();
    let server = Server::start().unwrap();
    let response = get(&server, "/zen", None);
    assert_eq!(response.status(), 200);
    let _ = std::env::set_current_dir(original_dir);
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn unparseable_body_is_a_400() {
    let server = Server::start().unwrap();
    let reply = common::Api::admin(&server).post_raw("/user/repos", "not json");
    assert_eq!(
        reply.body,
        json!({
            "message": "Problems parsing JSON",
            "documentation_url": "https://docs.github.com/rest/repos/repos#create-a-repository-for-the-authenticated-user",
            "status": "400",
        })
    );
    assert_eq!(reply.status, 400);
}

#[test]
fn hits_record_operations_fields_and_unrouted_requests() {
    use ninot_github::server::Hit;
    let server = Server::start().unwrap();
    get(&server, "/zen", None);
    get(&server, "/repos/o/r/actions/workflows", None);
    get(&server, "/no/such/route", None);
    Client::new()
        .post(format!("{}/api/graphql", server.url()))
        .header("Authorization", format!("token {}", server.admin_token()))
        .body(r#"{"query": "{viewer{login __typename}}"}"#)
        .send()
        .unwrap();
    Client::new()
        .post(format!("{}/api/graphql", server.url()))
        .header("Authorization", format!("token {}", server.admin_token()))
        .body(r#"{"query": "{viewer{releases}}"}"#)
        .send()
        .unwrap();
    let hits = server.hits();
    for expected in [
        Hit::Rest {
            operation: "meta/get-zen",
            status: 200,
        },
        Hit::Rest {
            operation: "actions/list-repo-workflows",
            status: 501,
        },
        Hit::Unrouted {
            method: "GET".to_owned(),
            path: "/no/such/route".to_owned(),
        },
        Hit::GraphQl {
            field: "Query.viewer".to_owned(),
        },
        Hit::GraphQl {
            field: "User.login".to_owned(),
        },
        Hit::GraphQlError {
            field: Some("User.releases".to_owned()),
            message: r#"Unknown field "releases" on type "User"."#.to_owned(),
        },
    ] {
        assert!(hits.contains(&expected), "{expected} missing from {hits:?}");
    }
    assert_eq!(hits.len(), 6, "{hits:?}");
}
