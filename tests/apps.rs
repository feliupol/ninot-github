//! GitHub App authentication and installation access-token protocol.

use ninot_github::Server;
use reqwest::blocking::Client;
use serde_json::json;

#[test]
fn app_jwt_discovers_installation_and_mints_a_repository_scoped_token() {
    let server = Server::start().unwrap();
    server.add_user("alice");
    let repository = server
        .create_repository("alice", "private", true, None, None, "main")
        .unwrap();
    repository
        .create_installation(73)
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
        .create_issue(
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

#[test]
fn installation_secrets_permission_bounds_and_scopes_its_tokens() {
    use ninot_github::fixtures::{ActionsPublicKeySeed, ActionsSecretsPermission};

    let server = Server::start().unwrap();
    server.add_user("alice");
    let repository = server
        .create_repository("alice", "app", true, None, None, "main")
        .unwrap();
    repository
        .create_installation_with_secrets_permission(74, Some(ActionsSecretsPermission::Read))
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
