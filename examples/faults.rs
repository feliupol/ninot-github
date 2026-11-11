//! Install one fault rule and show the request it fails.
//!
//! `GET /zen` normally answers 200 with no token. A rule aimed at `meta/get-zen` makes that
//! call answer 503 instead.

#![deny(missing_docs)]

use ninot_github::faults::{Fault, Rule, Target};

fn main() {
    let server = ninot_github::Server::start().expect("start the fake");
    server.add_fault(Rule::new(
        Target::Operation("meta/get-zen".to_owned()),
        Fault::Status(503),
    ));

    let response = reqwest::blocking::Client::new()
        .get(format!("{}/zen", server.url()))
        .send()
        .expect("GET /zen");
    let status = response.status();
    let body = response.text().unwrap_or_default();
    assert_eq!(status, 503);
    println!("GET /zen -> {status} {body}");
}
