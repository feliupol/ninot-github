//! GitHub App authentication and installation access-token protocol.

use ninot_github::Server;
use reqwest::blocking::Client;
use serde_json::json;

#[test]
fn app_jwt_discovers_installation_and_mints_a_repository_scoped_token() {
    let server = Server::start().unwrap();
    server.add_user("alice");
    let repository = server
        .add_repository("alice", "private", true, None, None, "main")
        .unwrap();
    repository
        .add_installation(73)
        .expect("server is still running");

    let app_jwt = "eyJhbGciOiJSUzI1NiJ9.eyJpc3MiOiIxIn0.signature";
    let client = Client::new();
    let installation = client
        .get(format!("{}/repos/alice/private/installation", server.url()))
        .bearer_auth(app_jwt)
        .send()
        .unwrap();
    assert_eq!(installation.status(), 200);
    let installation: serde_json::Value = installation.json().unwrap();
    assert_eq!(installation["id"], 73);
    assert_eq!(installation["account"]["login"], "alice");

    let token_response = client
        .post(format!(
            "{}/app/installations/73/access_tokens",
            server.url()
        ))
        .bearer_auth(app_jwt)
        .json(&json!({}))
        .send()
        .unwrap();
    assert_eq!(token_response.status(), 201);
    let token_response: serde_json::Value = token_response.json().unwrap();
    let access_token = token_response["token"].as_str().unwrap();
    assert!(token_response["expires_at"].as_str().is_some());

    let repository_response = client
        .get(format!("{}/repos/alice/private", server.url()))
        .bearer_auth(access_token)
        .send()
        .unwrap();
    assert_eq!(repository_response.status(), 200);

    repository
        .add_issue(
            1,
            ninot_github::fixtures::IssueSeed {
                title: ninot_github::fixtures::SeedField::present("Issue".to_owned()),
                state: ninot_github::fixtures::SeedField::present("open".to_owned()),
                ..Default::default()
            },
        )
        .expect("issue fixture matches the API model");
    let comment = client
        .post(format!(
            "{}/repos/alice/private/issues/1/comments",
            server.url()
        ))
        .bearer_auth(access_token)
        .json(&json!({"body": "App installation comment"}))
        .send()
        .unwrap();
    assert_eq!(comment.status(), 201);
}

/// An installation access token for `installation`, minted with an App JWT.
fn installation_token(server: &Server, installation: u64) -> String {
    let response: serde_json::Value = Client::new()
        .post(format!(
            "{}/app/installations/{installation}/access_tokens",
            server.url()
        ))
        .bearer_auth("eyJhbGciOiJSUzI1NiJ9.eyJpc3MiOiIxIn0.signature")
        .json(&json!({}))
        .send()
        .unwrap()
        .json()
        .unwrap();
    response["token"].as_str().unwrap().to_owned()
}

/// Status and error message of an authenticated request.
fn send(
    server: &Server,
    token: &str,
    method: reqwest::Method,
    path: &str,
    body: Option<serde_json::Value>,
) -> (u16, serde_json::Value) {
    let mut request = Client::new()
        .request(method, format!("{}{path}", server.url()))
        .bearer_auth(token);
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().unwrap();
    let status = response.status().as_u16();
    let text = response.text().unwrap();
    (status, serde_json::from_str(&text).unwrap_or_default())
}

#[test]
fn installation_tokens_show_and_grant_the_installation_permissions() {
    let server = Server::start().unwrap();
    server.add_user("alice");
    let repository = server
        .add_repository("alice", "tool", true, None, None, "main")
        .unwrap();
    repository.add_installation(5).unwrap();
    let response: serde_json::Value = Client::new()
        .post(format!(
            "{}/app/installations/5/access_tokens",
            server.url()
        ))
        .bearer_auth("eyJhbGciOiJSUzI1NiJ9.eyJpc3MiOiIxIn0.signature")
        .json(&json!({}))
        .send()
        .unwrap()
        .json()
        .unwrap();
    assert_eq!(
        response["permissions"],
        json!({
            "checks": "write",
            "contents": "write",
            "issues": "write",
            "metadata": "read",
            "pull_requests": "write",
        })
    );
    let scopes = Client::new()
        .get(format!("{}/repos/alice/tool", server.url()))
        .bearer_auth(response["token"].as_str().unwrap())
        .send()
        .unwrap()
        .headers()["x-oauth-scopes"]
        .to_str()
        .unwrap()
        .to_owned();
    assert!(
        scopes.contains("permission:checks:write") && scopes.contains("permission:metadata:read"),
        "{scopes}"
    );
}

#[test]
fn installation_tokens_need_the_permission_the_github_apps_data_lists() {
    let server = Server::start().unwrap();
    server.add_user("alice");
    let repository = server
        .add_repository("alice", "tool", true, None, None, "main")
        .unwrap();
    repository.add_installation(5).unwrap();
    let token = installation_token(&server, 5);
    let integration = json!("Resource not accessible by integration");

    // repos/create-webhook needs repository_hooks, which the installation lacks.
    let (status, body) = send(
        &server,
        &token,
        reqwest::Method::POST,
        "/repos/alice/tool/hooks",
        Some(json!({"config": {"url": "http://127.0.0.1:9/hook"}})),
    );
    assert_eq!((status, &body["message"]), (403, &integration));
    // users/get-authenticated is not available to installation tokens at all.
    let (status, body) = send(&server, &token, reqwest::Method::GET, "/user", None);
    assert_eq!((status, &body["message"]), (403, &integration));
    // repos/list-workflow-runs-for-repo needs actions.
    let (status, body) = send(
        &server,
        &token,
        reqwest::Method::GET,
        "/repos/alice/tool/actions/runs",
        None,
    );
    assert_eq!((status, &body["message"]), (403, &integration));
    // pulls/list needs pull_requests, which the installation has.
    let (status, _) = send(
        &server,
        &token,
        reqwest::Method::GET,
        "/repos/alice/tool/pulls",
        None,
    );
    assert_eq!(status, 200);
}

#[test]
fn installation_tokens_reach_org_operations_only_with_org_permissions() {
    let server = Server::start().unwrap();
    server.add_user("alice");
    server.add_org("acme", &["alice"]);
    server
        .add_repository("acme", "tool", true, None, None, "main")
        .unwrap()
        .add_installation(5)
        .unwrap();
    let token = installation_token(&server, 5);
    // orgs/list-members needs members, which the installation lacks.
    let (status, body) = send(
        &server,
        &token,
        reqwest::Method::GET,
        "/orgs/acme/members",
        None,
    );
    assert_eq!(
        (status, &body["message"]),
        (403, &json!("Resource not accessible by integration"))
    );
}

#[test]
fn a_secrets_read_grant_reads_but_does_not_write_secrets() {
    let server = Server::start().unwrap();
    server.add_user("alice");
    let repository = server
        .add_repository("alice", "tool", true, None, None, "main")
        .unwrap();
    repository
        .add_installation_with_secrets_permission(
            5,
            Some(ninot_github::fixtures::ActionsSecretsPermission::Read),
        )
        .unwrap();
    repository
        .add_installation_with_secrets_permission(
            6,
            Some(ninot_github::fixtures::ActionsSecretsPermission::Write),
        )
        .unwrap();
    let reader = installation_token(&server, 5);
    let writer = installation_token(&server, 6);
    let none = {
        repository.add_installation(7).unwrap();
        installation_token(&server, 7)
    };
    let list = "/repos/alice/tool/actions/secrets";
    assert_eq!(
        send(&server, &reader, reqwest::Method::GET, list, None).0,
        200
    );
    assert_eq!(
        send(&server, &none, reqwest::Method::GET, list, None).0,
        403
    );
    let delete = "/repos/alice/tool/actions/secrets/MISSING";
    let (status, body) = send(&server, &reader, reqwest::Method::DELETE, delete, None);
    assert_eq!(
        (status, &body["message"]),
        (403, &json!("Resource not accessible by integration"))
    );
    assert_ne!(
        send(&server, &writer, reqwest::Method::DELETE, delete, None).0,
        403
    );
}

#[test]
fn classic_tokens_need_the_repo_scope_for_secrets() {
    let server = Server::start().unwrap();
    let alice = server.add_user("alice");
    server
        .add_repository("alice", "tool", true, None, None, "main")
        .unwrap();
    let list = "/repos/alice/tool/actions/secrets";
    let secrets_only = alice.add_token(&["actions:secrets:read"]);
    assert_eq!(
        send(&server, &secrets_only, reqwest::Method::GET, list, None).0,
        403
    );
    let repo = alice.add_token(&["repo"]);
    assert_eq!(
        send(&server, &repo, reqwest::Method::GET, list, None).0,
        200
    );
}

#[test]
fn installation_secrets_permission_bounds_and_scopes_its_tokens() {
    use ninot_github::fixtures::{ActionsPublicKeySeed, ActionsSecretsPermission};

    let server = Server::start().unwrap();
    server.add_user("alice");
    let repository = server
        .add_repository("alice", "app", true, None, None, "main")
        .unwrap();
    repository
        .add_installation_with_secrets_permission(74, Some(ActionsSecretsPermission::Read))
        .unwrap();
    repository
        .set_actions_public_key(ActionsPublicKeySeed {
            key_id: "kid".to_owned(),
            key: "a2V5".to_owned(),
        })
        .unwrap();
    let app_jwt = "eyJhbGciOiJSUzI1NiJ9.eyJpc3MiOiIxIn0.signature";
    let client = Client::new();
    let mint = |permissions: serde_json::Value| {
        client
            .post(format!(
                "{}/app/installations/74/access_tokens",
                server.url()
            ))
            .bearer_auth(app_jwt)
            .json(&json!({ "permissions": permissions }))
            .send()
            .unwrap()
    };
    assert_eq!(mint(json!({"secrets": "write"})).status(), 422);
    let minted: serde_json::Value = mint(json!({"secrets": "read"})).json().unwrap();
    assert_eq!(minted["permissions"]["secrets"], "read");
    let token = minted["token"].as_str().unwrap();
    let key = client
        .get(format!(
            "{}/repos/alice/app/actions/secrets/public-key",
            server.url()
        ))
        .bearer_auth(token)
        .send()
        .unwrap();
    assert_eq!(key.status(), 200);
    let key: serde_json::Value = key.json().unwrap();
    assert_eq!(key, json!({"key_id": "kid", "key": "a2V5"}));
    let put = client
        .put(format!(
            "{}/repos/alice/app/actions/secrets/TOKEN",
            server.url()
        ))
        .bearer_auth(token)
        .json(&json!({"encrypted_value": "eA==", "key_id": "kid"}))
        .send()
        .unwrap();
    assert_eq!(put.status(), 403);
}
