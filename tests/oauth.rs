//! The OAuth web flow of a GitHub App and the user access tokens it issues.

mod common;

use common::Api;
use ninot_github::Server;
use reqwest::blocking::Client;
use reqwest::redirect::Policy;
use serde_json::{Value, json};

const REDIRECT: &str = "http://localhost:3000/callback";

fn client() -> Client {
    Client::builder().redirect(Policy::none()).build().unwrap()
}

/// Approves an authorization as `login` and returns the `code` of the redirect, checking `state`.
fn authorize(server: &Server, login: &str) -> String {
    let response = client()
        .get(format!("{}/login/oauth/authorize", server.url()))
        .query(&[
            ("client_id", "Iv1.abc"),
            ("redirect_uri", REDIRECT),
            ("state", "xyz"),
            ("login", login),
        ])
        .send()
        .unwrap();
    assert_eq!(response.status(), 302);
    let location = response.headers()["location"].to_str().unwrap().to_owned();
    let location = url::Url::parse(&location).unwrap();
    assert_eq!(
        location.origin().ascii_serialization(),
        "http://localhost:3000"
    );
    assert_eq!(location.path(), "/callback");
    let pairs: std::collections::HashMap<_, _> = location.query_pairs().into_owned().collect();
    assert_eq!(pairs["state"], "xyz");
    pairs["code"].clone()
}

fn exchange(server: &Server, accept: Option<&str>, form: &[(&str, &str)]) -> (String, String) {
    let mut request = client()
        .post(format!("{}/login/oauth/access_token", server.url()))
        .form(form);
    if let Some(accept) = accept {
        request = request.header("accept", accept);
    }
    let response = request.send().unwrap();
    assert_eq!(response.status(), 200);
    let content_type = response.headers()["content-type"]
        .to_str()
        .unwrap()
        .to_owned();
    (content_type, response.text().unwrap())
}

fn seeded() -> Server {
    let server = Server::start().unwrap();
    server.add_user("alice");
    server.add_oauth_app("Iv1.abc", "shh", 9);
    server
}

#[test]
fn a_code_is_exchanged_once_for_a_user_token_in_json_or_form() {
    let server = seeded();
    let repository = server
        .add_repository("alice", "tool", true, None, None, "main")
        .unwrap();
    repository.add_app_installation(73, 9).unwrap();

    let code = authorize(&server, "alice");
    let (content_type, body) = exchange(
        &server,
        Some("application/json"),
        &[
            ("client_id", "Iv1.abc"),
            ("client_secret", "shh"),
            ("code", &code),
            ("redirect_uri", REDIRECT),
        ],
    );
    assert!(content_type.starts_with("application/json"));
    let reply: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(reply["token_type"], "bearer");
    assert_eq!(reply["scope"], "");
    let token = reply["access_token"].as_str().unwrap();
    assert!(token.starts_with("ghu_"));

    let api = Api::new(&server, Some(token));
    assert_eq!(api.get("/user").body["login"], "alice");
    assert_eq!(
        api.get("/user/installations").body["installations"][0]["id"],
        73
    );

    let (_, replay) = exchange(
        &server,
        Some("application/json"),
        &[
            ("client_id", "Iv1.abc"),
            ("client_secret", "shh"),
            ("code", &code),
        ],
    );
    assert_eq!(
        serde_json::from_str::<Value>(&replay).unwrap()["error"],
        "bad_verification_code"
    );

    let code = authorize(&server, "alice");
    let (content_type, body) = exchange(
        &server,
        None,
        &[
            ("client_id", "Iv1.abc"),
            ("client_secret", "shh"),
            ("code", &code),
        ],
    );
    assert!(content_type.starts_with("application/x-www-form-urlencoded"));
    let pairs: std::collections::HashMap<_, _> = url::form_urlencoded::parse(body.as_bytes())
        .into_owned()
        .collect();
    assert!(pairs["access_token"].starts_with("ghu_"));
    assert_eq!(pairs["token_type"], "bearer");
    assert_eq!(pairs["scope"], "");
}

#[test]
fn the_exchange_accepts_a_json_body_and_reports_errors_with_status_200() {
    let server = seeded();
    let code = authorize(&server, "alice");
    let post = |body: Value| {
        let response = client()
            .post(format!("{}/login/oauth/access_token", server.url()))
            .header("accept", "application/json")
            .json(&body)
            .send()
            .unwrap();
        assert_eq!(response.status(), 200);
        response.json::<Value>().unwrap()
    };
    let wrong_secret = post(json!({ "client_id": "Iv1.abc", "client_secret": "no", "code": code }));
    assert_eq!(wrong_secret["error"], "incorrect_client_credentials");
    assert!(wrong_secret["error_description"].is_string());
    assert!(
        wrong_secret["error_uri"]
            .as_str()
            .unwrap()
            .starts_with("https://docs.github.com/")
    );
    let mismatch = post(
        json!({ "client_id": "Iv1.abc", "client_secret": "shh", "code": code, "redirect_uri": "http://localhost:1/other" }),
    );
    assert_eq!(mismatch["error"], "redirect_uri_mismatch");
    let ok = post(json!({ "client_id": "Iv1.abc", "client_secret": "shh", "code": code }));
    assert!(ok["access_token"].as_str().unwrap().starts_with("ghu_"));
}

#[test]
fn authorize_refuses_unknown_apps_and_users_and_a_missing_callback() {
    let server = seeded();
    let get = |query: &[(&str, &str)]| {
        client()
            .get(format!("{}/login/oauth/authorize", server.url()))
            .query(query)
            .send()
            .unwrap()
            .status()
            .as_u16()
    };
    assert_eq!(
        get(&[("client_id", "other"), ("redirect_uri", REDIRECT)]),
        404
    );
    assert_eq!(
        get(&[
            ("client_id", "Iv1.abc"),
            ("redirect_uri", REDIRECT),
            ("login", "nobody")
        ]),
        404
    );
    assert_eq!(get(&[("client_id", "Iv1.abc")]), 422);
}
