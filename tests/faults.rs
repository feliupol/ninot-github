//! Injected latency and failures, against the REST API and git.

mod common;

use std::time::{Duration, Instant};

use common::{Api, TempDir, clone_url, commit_file, git, git_ok};
use ninot_github::Server;
use ninot_github::faults::{Fault, Rule, Target};
use reqwest::blocking::Client;
use serde_json::json;

fn get_repo(server: &Server) -> reqwest::Result<reqwest::blocking::Response> {
    Client::new()
        .get(format!("{}/repos/admin/tool", server.url()))
        .header("Authorization", format!("token {}", server.admin_token()))
        .send()
}

fn server_with_repo() -> Server {
    let server = Server::start().unwrap();
    Api::admin(&server).post("/user/repos", json!({ "name": "tool", "auto_init": true }));
    server
}

fn operation(id: &str) -> Target {
    Target::Operation(id.to_owned())
}

#[test]
fn latency_delays_matching_requests_only() {
    let server = server_with_repo();
    // Warms up response validation, which compiles each operation's schema on first use.
    assert_eq!(Api::admin(&server).get("/user").status, 200);
    assert_eq!(get_repo(&server).unwrap().status(), 200);
    server.add_fault(Rule::new(operation("repos/get"), Fault::LatencyMs(300)));
    let started = Instant::now();
    assert_eq!(get_repo(&server).unwrap().status(), 200);
    assert!(started.elapsed() >= Duration::from_millis(300));
    let started = Instant::now();
    assert_eq!(Api::admin(&server).get("/user").status, 200);
    assert!(started.elapsed() < Duration::from_millis(300));
}

#[test]
fn statuses_and_secondary_rate_limits() {
    let server = server_with_repo();
    server.add_fault(Rule::new(operation("repos/get"), Fault::Status(502)).times(1));
    server.add_fault(Rule::new(operation("repos/get"), Fault::SecondaryRateLimit).times(1));
    let failed = get_repo(&server).unwrap();
    assert_eq!(failed.status(), 502);
    let limited = get_repo(&server).unwrap();
    assert_eq!(limited.status(), 403);
    assert_eq!(limited.headers()["retry-after"], "60");
    assert!(limited.text().unwrap().contains("secondary rate limit"));
    assert_eq!(get_repo(&server).unwrap().status(), 200);
    assert_eq!(server.applied_faults().len(), 2);
}

#[test]
fn dropped_and_truncated_api_responses() {
    let server = server_with_repo();
    server.add_fault(Rule::new(operation("repos/get"), Fault::Drop).times(1));
    let dropped = get_repo(&server);
    assert!(dropped.is_err(), "{dropped:?}");
    server.add_fault(Rule::new(operation("repos/get"), Fault::Truncate).times(1));
    let truncated = get_repo(&server).unwrap();
    assert_eq!(truncated.status(), 200);
    assert!(truncated.text().is_err());
    assert_eq!(get_repo(&server).unwrap().status(), 200);
}

#[test]
fn git_push_and_fetch_fail_until_the_fault_expires() {
    let server = server_with_repo();
    let url = clone_url(&server, "admin", "tool", Some(server.admin_token()));
    let work = TempDir::new();
    git_ok(&work.0, &["clone", "-q", &url, "c"]);
    let clone = work.join("c");
    commit_file(&clone, "x.txt", "x");

    server.add_fault(Rule::new(Target::Git("receive-pack".into()), Fault::Drop).times(1));
    assert!(
        !git(&clone, &["push", "-q", "origin", "main"])
            .status
            .success()
    );
    git_ok(&clone, &["push", "-q", "origin", "main"]);

    server.add_fault(Rule::new(Target::Git("upload-pack".into()), Fault::Truncate).times(1));
    assert!(!git(&work.0, &["clone", "-q", &url, "d"]).status.success());
    let _ = std::fs::remove_dir_all(work.join("d"));
    git_ok(&work.0, &["clone", "-q", &url, "d"]);
    assert!(work.join("d/x.txt").exists());

    server.add_fault(Rule::new(
        Target::Git("upload-pack".into()),
        Fault::LatencyMs(200),
    ));
    let started = Instant::now();
    git_ok(&clone, &["fetch", "-q"]);
    assert!(
        started.elapsed() >= Duration::from_millis(400),
        "info/refs and upload-pack are both delayed"
    );
}

#[test]
fn seeded_probabilities_are_reproducible() {
    let run = |seed| {
        let server = server_with_repo();
        server.seed_faults(seed);
        server
            .add_fault(Rule::new(operation("repos/get"), Fault::Status(500)).with_probability(0.5));
        (0..30)
            .map(|_| get_repo(&server).unwrap().status().as_u16())
            .collect::<Vec<_>>()
    };
    let first = run(42);
    assert_eq!(first, run(42));
    assert!(first.contains(&500) && first.contains(&200), "{first:?}");
}
