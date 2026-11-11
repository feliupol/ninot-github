//! An organization, a team and a self-hosted runner.
//!
//! The registration token comes from
//! `POST /orgs/{org}/actions/runners/registration-token`, which needs `admin:org`.
//! [`Org::register_runner`](ninot_github::Org::register_runner) then records the runner the way
//! `config.sh` would.

#![deny(missing_docs)]

use serde_json::{Value, json};

fn main() {
    let server = ninot_github::Server::start().expect("start the fake");
    let alice = server.add_user("alice");
    let token = alice.add_token(&["admin:org", "repo"]);
    let org = server.add_org("acme", &["alice"]);
    let team = org
        .add_team("engineering", &["alice"])
        .expect("add the team");
    println!("org {} team {}", org.login(), team.slug());

    let response = reqwest::blocking::Client::new()
        .post(format!(
            "{}/orgs/acme/actions/runners/registration-token",
            server.url()
        ))
        .header("Authorization", format!("token {token}"))
        .json(&json!({}))
        .send()
        .expect("request a registration token");
    assert_eq!(response.status(), 201, "registration token");
    let body: Value = response.json().expect("registration token JSON");
    let registration = body["token"].as_str().expect("registration token field");
    let runner = org
        .register_runner(registration, "ci-1", &["self-hosted", "Linux"], "linux")
        .expect("register the runner");
    println!("runner {}", runner.id());
}
