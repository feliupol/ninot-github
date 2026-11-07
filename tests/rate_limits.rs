//! Primary and secondary rate limits, driven through the test clock.

mod common;

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use common::{Api, Reply, TempDir, clone_url, git_ok};
use ninot_github::Server;
use ninot_github::faults::{Fault, Rule, Target};
use ninot_github::rate_limit::{Limits, Quota};
use ninot_github::server::Options;
use serde_json::json;

const HOUR: Duration = Duration::from_secs(3600);

fn start(limits: Limits) -> Server {
    Server::start_with(Options {
        rate_limits: limits,
        ..Options::default()
    })
    .unwrap()
}

fn quota(authenticated: u32, anonymous: u32) -> Quota {
    Quota {
        authenticated,
        anonymous,
        window: HOUR,
    }
}

fn number(reply: &Reply, name: &str) -> u64 {
    reply
        .header(name)
        .unwrap_or_else(|| panic!("no {name}"))
        .parse()
        .unwrap()
}

fn assert_window(reply: &Reply, resource: &str, limit: u64, used: u64) {
    assert_eq!(reply.header("x-ratelimit-resource"), Some(resource));
    assert_eq!(number(reply, "x-ratelimit-limit"), limit);
    assert_eq!(number(reply, "x-ratelimit-used"), used);
    assert_eq!(number(reply, "x-ratelimit-remaining"), limit - used);
}

fn assert_primary_limited(reply: &Reply, resource: &str, limit: u64) {
    assert_eq!(reply.status, 403);
    assert!(
        reply.body["message"]
            .as_str()
            .unwrap()
            .starts_with("API rate limit exceeded for "),
        "{}",
        reply.body
    );
    assert_window(reply, resource, limit, limit);
}

#[test]
fn responses_carry_github_default_windows() {
    let server = Server::start().unwrap();
    let admin = Api::admin(&server);
    let reply = admin.get("/user");
    assert_eq!(reply.status, 200);
    assert_window(&reply, "core", 5000, 1);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let reset = number(&reply, "x-ratelimit-reset");
    assert!((now..=now + 3601).contains(&reset), "{reset}");

    let overview = admin.get("/rate_limit");
    assert_eq!(overview.status, 200);
    assert_window(&overview, "core", 5000, 1);
    let core = &overview.body["resources"]["core"];
    assert_eq!(core["used"], 1);
    assert_eq!(core["remaining"], 4999);
    assert_eq!(core["reset"], reset);
    assert_eq!(overview.body["rate"], *core);
    assert_eq!(overview.body["resources"]["search"]["limit"], 30);
    assert_eq!(
        admin.get("/rate_limit").body["resources"]["core"]["used"],
        1
    );

    assert_window(&Api::new(&server, None).get("/zen"), "core", 60, 1);
}

#[test]
fn exhausted_core_window_resets_through_the_test_clock() {
    let server = start(Limits {
        core: quota(2, 1),
        ..Limits::default()
    });
    let admin = Api::admin(&server);
    assert_eq!(admin.get("/user").status, 200);
    assert_eq!(admin.get("/user").status, 200);
    assert_primary_limited(&admin.get("/user"), "core", 2);
    assert_eq!(
        admin.get("/rate_limit").body["resources"]["core"]["remaining"],
        0
    );

    let other = Api::new(&server, Some(&server.add_user("admin").add_token(&[])));
    assert_window(&other.get("/user"), "core", 2, 1);

    let anonymous = Api::new(&server, None);
    assert_eq!(anonymous.get("/zen").status, 200);
    let limited = anonymous.get("/zen");
    assert_primary_limited(&limited, "core", 1);
    assert!(
        limited.body["message"]
            .as_str()
            .unwrap()
            .contains("127.0.0.1")
    );

    server.advance_clock(HOUR);
    assert_window(&admin.get("/user"), "core", 2, 1);
    assert_window(&anonymous.get("/zen"), "core", 1, 1);
}

#[test]
fn search_and_graphql_have_their_own_windows() {
    let server = start(Limits {
        search: quota(1, 1),
        graphql: quota(1, 0),
        ..Limits::default()
    });
    let admin = Api::admin(&server);
    let searched = admin.get("/search/repositories?q=tool");
    assert_eq!(searched.status, 200);
    assert_window(&searched, "search", 1, 1);
    assert_primary_limited(&admin.get("/search/repositories?q=tool"), "search", 1);

    let query = json!({ "query": "{ viewer { login } }" });
    let answered = admin.post("/graphql", query.clone());
    assert_eq!(answered.status, 200);
    assert_window(&answered, "graphql", 1, 1);
    assert_primary_limited(&admin.post("/graphql", query), "graphql", 1);

    assert_window(&admin.get("/user"), "core", 5000, 1);
    let overview = admin.get("/rate_limit").body;
    assert_eq!(overview["resources"]["search"]["remaining"], 0);
    assert_eq!(overview["resources"]["graphql"]["remaining"], 0);

    server.advance_clock(HOUR);
    assert_eq!(admin.get("/search/repositories?q=tool").status, 200);
}

#[test]
fn git_operations_are_not_counted() {
    let server = start(Limits {
        core: quota(2, 1),
        ..Limits::default()
    });
    let admin = Api::admin(&server);
    admin.post("/user/repos", json!({ "name": "tool", "auto_init": true }));
    let url = clone_url(&server, "admin", "tool", Some(server.admin_token()));
    let work = TempDir::new();
    git_ok(&work.0, &["clone", "-q", &url, "c"]);
    git_ok(&work.join("c"), &["fetch", "-q"]);
    assert_window(&admin.get("/user"), "core", 2, 2);
}

#[test]
fn content_creation_hits_the_secondary_limit() {
    let server = start(Limits {
        content_creation_per_minute: 1,
        ..Limits::default()
    });
    let admin = Api::admin(&server);
    assert_eq!(
        admin.post("/user/repos", json!({ "name": "a" })).status,
        201
    );
    let limited = admin.post("/user/repos", json!({ "name": "b" }));
    assert_eq!(limited.status, 403);
    assert!(number(&limited, "retry-after") <= 60);
    assert!(
        limited.body["message"]
            .as_str()
            .unwrap()
            .contains("secondary rate limit")
    );
    assert_eq!(admin.get("/repos/admin/a").status, 200);
    server.advance_clock(Duration::from_secs(60));
    assert_eq!(
        admin.post("/user/repos", json!({ "name": "b" })).status,
        201
    );
}

#[test]
fn concurrent_requests_hit_the_secondary_limit() {
    let server = start(Limits {
        concurrent: 1,
        ..Limits::default()
    });
    server.add_fault(Rule::new(
        Target::Operation("users/get-authenticated".into()),
        Fault::LatencyMs(500),
    ));
    std::thread::scope(|scope| {
        let slow = scope.spawn(|| Api::admin(&server).get("/user").status);
        std::thread::sleep(Duration::from_millis(200));
        let limited = Api::admin(&server).get("/zen");
        assert_eq!(limited.status, 403);
        assert_eq!(limited.header("retry-after"), Some("60"));
        assert_eq!(slow.join().unwrap(), 200);
    });
    assert_eq!(Api::admin(&server).get("/zen").status, 200);
}
