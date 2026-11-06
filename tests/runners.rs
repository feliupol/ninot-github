//! Organization self-hosted runners, registration tokens, labels and runner groups.

mod common;

use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use common::Api;
use ninot_github::Server;
use serde_json::{Value, json};

fn org(org: &str, rest: &str) -> String {
    format!("/orgs/{org}/actions/{rest}")
}

fn registration_token(api: &Api<'_>, org_login: &str) -> String {
    let reply = api.post(&org(org_login, "runners/registration-token"), json!({}));
    assert_eq!(reply.status, 201, "{}", reply.body);
    reply.body["token"].as_str().unwrap().to_owned()
}

fn runner_names(body: &Value) -> Vec<String> {
    body["runners"]
        .as_array()
        .unwrap()
        .iter()
        .map(|runner| runner["name"].as_str().unwrap().to_owned())
        .collect()
}

fn group_names(body: &Value) -> Vec<String> {
    body["runner_groups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|group| group["name"].as_str().unwrap().to_owned())
        .collect()
}

fn label_count(body: &Value, name: &str) -> usize {
    body["labels"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|label| label["name"].as_str().unwrap().eq_ignore_ascii_case(name))
        .count()
}

fn has_label(body: &Value, name: &str, kind: &str) -> bool {
    body["labels"].as_array().unwrap().iter().any(|label| {
        label["name"].as_str().unwrap() == name && label["type"].as_str().unwrap() == kind
    })
}

/// Days since the Unix epoch, Howard Hinnant's `days_from_civil`.
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let yoe = (year - era * 400) as u64;
    let month_adj = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * month_adj as u64 + 2) / 5 + day as u64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe as i64 - 719_468
}

fn unix_of(stamp: &str) -> u64 {
    assert_eq!(stamp.len(), 20, "{stamp}");
    assert!(
        stamp.ends_with('Z') && stamp.as_bytes()[10] == b'T',
        "{stamp}"
    );
    let year: i64 = stamp[0..4].parse().unwrap();
    let month: u32 = stamp[5..7].parse().unwrap();
    let day: u32 = stamp[8..10].parse().unwrap();
    let hour: u64 = stamp[11..13].parse().unwrap();
    let minute: u64 = stamp[14..16].parse().unwrap();
    let second: u64 = stamp[17..19].parse().unwrap();
    let days = days_from_civil(year, month, day);
    (days as u64) * 86_400 + hour * 3_600 + minute * 60 + second
}

#[test]
fn authorization() {
    let server = Server::start().unwrap();
    server.add_user("alice");
    server.add_org("acme", &["alice"]);
    let alice = Api::new(&server, Some(&server.add_token("alice", &["admin:org"])));
    let repo_only = Api::new(&server, Some(&server.add_token("alice", &["repo"])));
    let anon = Api::new(&server, None);
    let admin = Api::admin(&server);

    assert_eq!(anon.get(&org("acme", "runners")).status, 401);
    assert_eq!(anon.get(&org("missing", "runners")).status, 401);
    assert_eq!(repo_only.get(&org("acme", "runners")).status, 403);
    assert_eq!(
        repo_only.get(&org("missing", "runners")).status,
        404,
        "an unknown organization is 404 before the scope is checked"
    );
    assert_eq!(alice.get(&org("missing", "runners")).status, 404);
    assert_eq!(alice.get(&org("alice", "runners")).status, 404);
    assert_eq!(
        repo_only
            .post(
                &org("acme", "runners/generate-jitconfig"),
                json!({"name": "box", "runner_group_id": 1, "labels": ["gpu"]})
            )
            .status,
        403
    );
    assert_eq!(
        alice
            .post(&org("acme", "runners/generate-jitconfig"), json!({}))
            .status,
        422
    );

    assert_eq!(alice.get(&org("acme", "runners")).status, 200);
    // The admin user is not a member of acme; `admin:org` is enough.
    assert_eq!(admin.get(&org("acme", "runners")).status, 200);
    assert_eq!(
        alice
            .post(&org("acme", "runners/registration-token"), json!({}))
            .status,
        201
    );
    assert_eq!(
        admin
            .post(&org("acme", "runners/remove-token"), json!({}))
            .status,
        201
    );
    assert_eq!(
        repo_only
            .post(&org("acme", "runners/registration-token"), json!({}))
            .status,
        403
    );
    assert_eq!(
        alice
            .post(&org("nope", "runners/registration-token"), json!({}))
            .status,
        404
    );
}

#[test]
fn registration_and_remove_tokens() {
    let server = Server::start().unwrap();
    server.add_user("alice");
    server.add_org("acme", &["alice"]);
    server.add_org("other", &[]);
    let alice = Api::new(&server, Some(&server.add_token("alice", &["admin:org"])));

    let registration = alice.post(&org("acme", "runners/registration-token"), json!({}));
    let removal = alice.post(&org("acme", "runners/remove-token"), json!({}));
    assert_eq!(registration.status, 201);
    assert_eq!(removal.status, 201);
    let registration_token = registration.body["token"].as_str().unwrap();
    let removal_token = removal.body["token"].as_str().unwrap();
    assert!(registration_token.starts_with("ghp_"));
    assert_ne!(registration_token, removal_token);

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    for body in [&registration.body, &removal.body] {
        let expires = unix_of(body["expires_at"].as_str().unwrap());
        let delta = expires.saturating_sub(now);
        assert!(
            (3_500..=3_700).contains(&delta),
            "token expires in {delta}s, expected about an hour"
        );
    }

    assert_eq!(
        server.register_runner("other", registration_token, "box", &[], "linux"),
        Err("registration token is invalid")
    );
    assert_eq!(
        server.register_runner("acme", removal_token, "box", &[], "linux"),
        Err("registration token is invalid")
    );
    let id = server
        .register_runner("acme", registration_token, "box", &["gpu"], "linux")
        .unwrap();
    assert!(id > 0);
    assert_eq!(
        server.register_runner("acme", registration_token, "other", &[], "linux"),
        Err("registration token has already been used")
    );
}

#[test]
fn register_runner_lists_and_goes_offline() {
    let server = Server::start().unwrap();
    server.add_org("acme", &[]);
    let api = Api::admin(&server);

    let token = registration_token(&api, "acme");
    assert_eq!(
        server.register_runner("acme", &token, "", &[], "linux"),
        Err("runner name is invalid")
    );
    assert_eq!(
        server.register_runner("acme", &token, "box", &["bad\n"], "linux"),
        Err("runner label is invalid")
    );
    assert_eq!(
        server.register_runner("acme", &token, "box", &[], ""),
        Err("runner os is invalid")
    );
    let id = server
        .register_runner("acme", &token, "box", &["gpu"], "linux")
        .unwrap();
    let again = registration_token(&api, "acme");
    assert_eq!(
        server.register_runner("acme", &again, "BOX", &[], "linux"),
        Err("a runner with this name already exists")
    );
    let mac = server
        .register_runner("acme", &again, "macbox", &[], "OSX")
        .unwrap();

    let listed = api.get(&org("acme", "runners"));
    assert_eq!(listed.status, 200);
    assert_eq!(listed.body["total_count"].as_u64(), Some(2));
    assert_eq!(runner_names(&listed.body), ["box", "macbox"]);

    let runner = api.get(&org("acme", &format!("runners/{id}")));
    assert_eq!(runner.status, 200);
    assert_eq!(runner.body["id"].as_u64(), Some(id));
    assert_eq!(runner.body["name"], "box");
    assert_eq!(runner.body["os"], "linux");
    assert_eq!(runner.body["status"], "online");
    assert_eq!(runner.body["busy"], false);
    assert_eq!(runner.body["ephemeral"], false);
    assert!(has_label(&runner.body, "self-hosted", "read-only"));
    assert!(has_label(&runner.body, "Linux", "read-only"));
    assert!(has_label(&runner.body, "gpu", "custom"));
    assert!(runner.body["runner_group_id"].as_u64().is_some());

    let mac_runner = api.get(&org("acme", &format!("runners/{mac}")));
    assert_eq!(mac_runner.body["os"], "macos");
    assert!(has_label(&mac_runner.body, "macOS", "read-only"));

    server.set_runner_offline("acme", id).unwrap();
    assert_eq!(
        api.get(&org("acme", &format!("runners/{id}"))).body["status"],
        "offline"
    );
    assert_eq!(
        server.set_runner_offline("acme", 9_999_999),
        Err("unknown runner")
    );
    assert_eq!(
        server.set_runner_offline("nope", id),
        Err("unknown organization")
    );

    let deleted = api.delete(&org("acme", &format!("runners/{id}")));
    assert_eq!(deleted.status, 204);
    assert!(deleted.body.is_null());
    assert_eq!(api.get(&org("acme", &format!("runners/{id}"))).status, 404);
    assert_eq!(
        api.delete(&org("acme", &format!("runners/{id}"))).status,
        404
    );
    assert_eq!(api.get(&org("acme", "runners/nope")).status, 404);
    assert_eq!(
        api.get(&org("acme", "runners")).body["total_count"].as_u64(),
        Some(1)
    );
}

#[test]
fn runner_labels() {
    let server = Server::start().unwrap();
    server.add_org("acme", &[]);
    let api = Api::admin(&server);
    let token = registration_token(&api, "acme");
    let id = server
        .register_runner("acme", &token, "box", &["gpu"], "linux")
        .unwrap();
    let labels = org("acme", &format!("runners/{id}/labels"));

    let added = api.post(&labels, json!({ "labels": ["gpu", "fast"] }));
    assert_eq!(added.status, 200);
    assert_eq!(label_count(&added.body, "gpu"), 1);
    assert!(has_label(&added.body, "fast", "custom"));
    assert!(has_label(&added.body, "self-hosted", "read-only"));
    assert_eq!(
        added.body["total_count"].as_u64(),
        Some(added.body["labels"].as_array().unwrap().len() as u64)
    );

    let cleared = api.put(&labels, json!({ "labels": [] }));
    assert_eq!(cleared.status, 200);
    assert_eq!(label_count(&cleared.body, "gpu"), 0);
    assert_eq!(label_count(&cleared.body, "fast"), 0);
    assert!(has_label(&cleared.body, "Linux", "read-only"));

    assert_eq!(
        api.post(&labels, json!({ "labels": ["fast", "gpu"] }))
            .status,
        200
    );
    let removed = api.delete(&format!("{labels}/FAST"));
    assert_eq!(removed.status, 200);
    assert_eq!(label_count(&removed.body, "fast"), 0);
    assert_eq!(label_count(&removed.body, "gpu"), 1);

    assert_eq!(api.delete(&format!("{labels}/Linux")).status, 422);
    assert!(has_label(&api.get(&labels).body, "Linux", "read-only"));
    assert_eq!(api.delete(&format!("{labels}/missing")).status, 404);

    let readonly = api.delete(&labels);
    assert_eq!(readonly.status, 200);
    assert!(has_label(&readonly.body, "self-hosted", "read-only"));
    assert_eq!(label_count(&readonly.body, "gpu"), 0);

    assert_eq!(api.post(&labels, json!({ "labels": [] })).status, 422);
    assert_eq!(api.post(&labels, json!({})).status, 422);
    assert_eq!(api.post(&labels, json!({ "labels": [""] })).status, 422);
    assert_eq!(api.get(&org("acme", "runners/999999/labels")).status, 404);
    assert_eq!(
        api.post(
            &org("acme", "runners/999999/labels"),
            json!({ "labels": ["fast"] })
        )
        .status,
        404
    );
}

#[test]
fn runner_groups_can_be_managed() {
    let server = Server::start().unwrap();
    server.add_org("acme", &[]);
    let api = Api::admin(&server);

    let listed = api.get(&org("acme", "runner-groups"));
    assert_eq!(listed.status, 200);
    assert_eq!(group_names(&listed.body), ["Default"]);
    let default_group = &listed.body["runner_groups"][0];
    assert_eq!(default_group["default"], true);
    assert_eq!(default_group["visibility"], "all");
    assert_eq!(default_group["allows_public_repositories"], false);
    assert_eq!(default_group["inherited"], false);
    assert!(default_group.get("selected_repositories_url").is_none());
    let default_id = default_group["id"].as_u64().unwrap();
    let runners_url = default_group["runners_url"].as_str().unwrap();
    assert!(runners_url.contains(&format!(
        "/orgs/acme/actions/runner-groups/{default_id}/runners"
    )));

    let via_prefix = api.get(&format!("/api/v3{}", org("acme", "runner-groups")));
    assert_eq!(via_prefix.status, 200);
    assert!(
        via_prefix.body["runner_groups"][0]["runners_url"]
            .as_str()
            .unwrap()
            .contains(&format!(
                "/api/v3/orgs/acme/actions/runner-groups/{default_id}/runners"
            ))
    );

    let alpha_token = registration_token(&api, "acme");
    let alpha = server
        .register_runner("acme", &alpha_token, "alpha", &["gpu"], "linux")
        .unwrap();
    let beta_token = registration_token(&api, "acme");
    let beta = server
        .register_runner("acme", &beta_token, "beta", &[], "windows")
        .unwrap();
    assert_eq!(
        api.get(&org("acme", &format!("runners/{beta}"))).body["os"],
        "windows"
    );

    let created = api.post(
        &org("acme", "runner-groups"),
        json!({
            "name": "gpu",
            "visibility": "selected",
            "allows_public_repositories": true,
            "restricted_to_workflows": true,
            "selected_workflows": ["acme/app/.github/workflows/ci.yml@main"],
            "runners": [alpha],
            "network_configuration_id": "net-1",
        }),
    );
    assert_eq!(created.status, 201, "{}", created.body);
    assert_eq!(created.body["name"], "gpu");
    assert_eq!(created.body["default"], false);
    assert_eq!(created.body["visibility"], "selected");
    assert_eq!(created.body["allows_public_repositories"], true);
    assert_eq!(created.body["restricted_to_workflows"], true);
    assert_eq!(created.body["network_configuration_id"], "net-1");
    assert!(
        created.body["selected_repositories_url"]
            .as_str()
            .unwrap()
            .ends_with("/repositories")
    );
    let group_id = created.body["id"].as_u64().unwrap();
    assert_eq!(
        api.get(&org("acme", &format!("runners/{alpha}"))).body["runner_group_id"].as_u64(),
        Some(group_id)
    );
    assert_eq!(
        api.get(&org("acme", &format!("runners/{beta}"))).body["runner_group_id"].as_u64(),
        Some(default_id)
    );

    assert_eq!(
        api.post(&org("acme", "runner-groups"), json!({ "name": "GPU" }))
            .status,
        422
    );
    assert_eq!(
        api.post(&org("acme", "runner-groups"), json!({})).status,
        422
    );
    assert_eq!(
        api.post(
            &org("acme", "runner-groups"),
            json!({ "name": "odd", "visibility": "nope" })
        )
        .status,
        422
    );
    let before = api.get(&org("acme", "runner-groups")).body["total_count"].as_u64();
    assert_eq!(
        api.post(
            &org("acme", "runner-groups"),
            json!({ "name": "nope", "runners": [9_999_999] })
        )
        .status,
        404
    );
    assert_eq!(
        api.get(&org("acme", "runner-groups")).body["total_count"].as_u64(),
        before
    );

    let plain = api.post(&org("acme", "runner-groups"), json!({ "name": "plain" }));
    assert_eq!(plain.status, 201, "{}", plain.body);
    assert_eq!(plain.body["visibility"], "all");
    assert_eq!(plain.body["allows_public_repositories"], false);
    assert_eq!(plain.body["restricted_to_workflows"], false);
    assert_eq!(plain.body["selected_workflows"], json!([]));
    assert!(plain.body.get("network_configuration_id").is_none());
    let plain_id = plain.body["id"].as_u64().unwrap();

    let updated = api.patch(
        &org("acme", &format!("runner-groups/{group_id}")),
        json!({
            "name": "gpu-2",
            "visibility": "private",
            "network_configuration_id": null,
        }),
    );
    assert_eq!(updated.status, 200, "{}", updated.body);
    assert_eq!(updated.body["name"], "gpu-2");
    assert_eq!(updated.body["visibility"], "private");
    assert!(updated.body.get("selected_repositories_url").is_none());
    assert!(updated.body.get("network_configuration_id").is_none());
    assert_eq!(
        api.patch(
            &org("acme", &format!("runner-groups/{group_id}")),
            json!({ "name": "gpu-2" })
        )
        .status,
        200
    );
    assert_eq!(
        api.patch(
            &org("acme", &format!("runner-groups/{group_id}")),
            json!({ "name": "Default" })
        )
        .status,
        422
    );
    assert_eq!(
        api.get(&org("acme", &format!("runner-groups/{group_id}")))
            .body["name"],
        "gpu-2"
    );
    assert_eq!(api.get(&org("acme", "runner-groups/999999")).status, 404);
    assert_eq!(
        api.patch(
            &org("acme", "runner-groups/999999"),
            json!({ "name": "nope" })
        )
        .status,
        404
    );

    let moved = api.put(
        &org("acme", &format!("runner-groups/{group_id}/runners")),
        json!({ "runners": [beta] }),
    );
    assert_eq!(moved.status, 204);
    assert!(moved.body.is_null());
    assert_eq!(
        api.get(&org("acme", &format!("runners/{beta}"))).body["runner_group_id"].as_u64(),
        Some(group_id)
    );
    assert_eq!(
        api.get(&org("acme", &format!("runners/{alpha}"))).body["runner_group_id"].as_u64(),
        Some(default_id)
    );
    assert_eq!(
        api.put(
            &org("acme", &format!("runner-groups/{default_id}/runners")),
            json!({ "runners": [] })
        )
        .body["message"],
        "Runners cannot be removed from the default runner group."
    );
    assert_eq!(
        api.get(&org("acme", &format!("runners/{alpha}"))).body["runner_group_id"].as_u64(),
        Some(default_id)
    );
    assert_eq!(
        api.put(
            &org("acme", &format!("runner-groups/{default_id}/runners")),
            json!({ "runners": [alpha, 9_999_999] })
        )
        .status,
        404
    );
    assert_eq!(
        api.put(
            &org("acme", &format!("runner-groups/{group_id}/runners")),
            json!({})
        )
        .status,
        422
    );
    assert_eq!(
        api.put(
            &org("acme", "runner-groups/999999/runners"),
            json!({ "runners": [] })
        )
        .status,
        404
    );

    assert_eq!(
        api.delete(&org("acme", &format!("runner-groups/{default_id}")))
            .body["message"],
        "Cannot delete the default runner group."
    );
    assert_eq!(
        api.delete(&org("acme", &format!("runner-groups/{group_id}")))
            .status,
        204
    );
    assert_eq!(
        api.get(&org("acme", &format!("runner-groups/{group_id}")))
            .status,
        404
    );
    assert_eq!(
        api.get(&org("acme", &format!("runners/{beta}"))).body["runner_group_id"].as_u64(),
        Some(default_id)
    );
    assert_eq!(
        api.delete(&org("acme", &format!("runner-groups/{plain_id}")))
            .status,
        204
    );
    assert_eq!(api.delete(&org("acme", "runner-groups/999999")).status, 404);
}

#[test]
fn runner_group_visibility() {
    let server = Server::start().unwrap();
    server.add_user("alice");
    server.add_org("acme", &["alice"]);
    let alice = Api::new(
        &server,
        Some(&server.add_token("alice", &["repo", "admin:org"])),
    );
    assert_eq!(
        alice
            .post(
                "/orgs/acme/repos",
                json!({ "name": "open", "private": false })
            )
            .status,
        201
    );
    assert_eq!(
        alice
            .post(
                "/orgs/acme/repos",
                json!({ "name": "closed", "private": true })
            )
            .status,
        201
    );
    let open_id = alice.get("/repos/acme/open").body["id"].as_u64().unwrap();
    let closed_id = alice.get("/repos/acme/closed").body["id"].as_u64().unwrap();

    assert_eq!(
        group_names(
            &alice
                .get(&org("acme", "runner-groups?visible_to_repository=open"))
                .body
        ),
        Vec::<String>::new()
    );
    assert_eq!(
        group_names(
            &alice
                .get(&org("acme", "runner-groups?visible_to_repository=closed"))
                .body
        ),
        ["Default"]
    );

    for (name, body) in [
        (
            "public-all",
            json!({ "name": "public-all", "visibility": "all", "allows_public_repositories": true }),
        ),
        ("priv", json!({ "name": "priv", "visibility": "private" })),
        (
            "priv-public",
            json!({ "name": "priv-public", "visibility": "private", "allows_public_repositories": true }),
        ),
        (
            "picked-open",
            json!({
                "name": "picked-open",
                "visibility": "selected",
                "allows_public_repositories": true,
                "selected_repository_ids": [open_id],
            }),
        ),
        (
            "picked-closed",
            json!({
                "name": "picked-closed",
                "visibility": "selected",
                "selected_repository_ids": [closed_id],
            }),
        ),
    ] {
        assert_eq!(
            alice.post(&org("acme", "runner-groups"), body).status,
            201,
            "{name}"
        );
    }
    assert_eq!(
        alice
            .post(
                &org("acme", "runner-groups"),
                json!({ "name": "missing-repo", "selected_repository_ids": [9_999_999] })
            )
            .status,
        422
    );

    assert_eq!(
        group_names(
            &alice
                .get(&org("acme", "runner-groups?visible_to_repository=open"))
                .body
        ),
        ["public-all", "picked-open"]
    );
    assert_eq!(
        group_names(
            &alice
                .get(&org(
                    "acme",
                    "runner-groups?visible_to_repository=acme/closed"
                ))
                .body
        ),
        [
            "Default",
            "public-all",
            "priv",
            "priv-public",
            "picked-closed"
        ]
    );
    assert_eq!(
        alice
            .get(&org("acme", "runner-groups?visible_to_repository=missing"))
            .body["total_count"]
            .as_u64(),
        Some(0)
    );
}

#[test]
fn jitconfig() {
    let server = Server::start().unwrap();
    server.add_org("acme", &[]);
    let api = Api::admin(&server);
    let groups = api.get(&org("acme", "runner-groups"));
    let default_id = groups.body["runner_groups"][0]["id"].as_u64().unwrap();

    let created = api.post(
        &org("acme", "runners/generate-jitconfig"),
        json!({
            "name": "jit-1",
            "runner_group_id": default_id,
            "labels": ["build"],
        }),
    );
    assert_eq!(created.status, 201, "{}", created.body);
    let runner = &created.body["runner"];
    assert_eq!(runner["name"], "jit-1");
    assert_eq!(runner["os"], "unknown");
    assert_eq!(runner["status"], "offline");
    assert_eq!(runner["busy"], false);
    assert_eq!(runner["ephemeral"], true);
    assert_eq!(runner["runner_group_id"].as_u64(), Some(default_id));
    assert!(has_label(runner, "self-hosted", "read-only"));
    assert!(has_label(runner, "build", "custom"));
    let runner_id = runner["id"].as_u64().unwrap();

    let bytes = STANDARD
        .decode(created.body["encoded_jit_config"].as_str().unwrap())
        .unwrap();
    let config: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(config["runnerName"], "jit-1");
    assert_eq!(config["runnerGroup"], "Default");
    assert_eq!(config["workFolder"], "_work");
    assert_eq!(config["useV2Flow"], true);
    assert_eq!(config["runnerId"].as_u64(), Some(runner_id));
    assert_eq!(config["runnerGroupId"].as_u64(), Some(default_id));
    assert_eq!(
        config["gitHubUrl"].as_str().unwrap(),
        format!("{}/acme", server.url())
    );
    let labels: Vec<&str> = config["runnerLabels"]
        .as_array()
        .unwrap()
        .iter()
        .map(|label| label.as_str().unwrap())
        .collect();
    assert!(labels.contains(&"build"));
    assert!(labels.contains(&"self-hosted"));
    assert_eq!(config["labels"], config["runnerLabels"]);

    let listed = api.get(&org("acme", "runners"));
    assert_eq!(listed.body["total_count"].as_u64(), Some(1));
    assert_eq!(listed.body["runners"][0]["status"], "offline");
    assert_eq!(listed.body["runners"][0]["os"], "unknown");

    assert_eq!(
        api.post(
            &org("acme", "runners/generate-jitconfig"),
            json!({ "name": "JIT-1", "runner_group_id": default_id, "labels": ["build"] })
        )
        .body["message"],
        "A runner with this name already exists."
    );
    assert_eq!(
        api.post(
            &org("acme", "runners/generate-jitconfig"),
            json!({ "name": "jit-2", "runner_group_id": 9_999_999, "labels": ["build"] })
        )
        .status,
        404
    );
    assert_eq!(
        api.post(&org("acme", "runners/generate-jitconfig"), json!({}))
            .status,
        422
    );
    assert_eq!(
        api.post(
            &org("acme", "runners/generate-jitconfig"),
            json!({ "name": "a/b", "runner_group_id": default_id, "labels": ["build"] })
        )
        .status,
        422
    );
    assert_eq!(
        api.post(
            &org("acme", "runners/generate-jitconfig"),
            json!({ "name": "jit-3", "runner_group_id": default_id, "labels": [] })
        )
        .status,
        422
    );
    let custom = api.post(
        &org("acme", "runners/generate-jitconfig"),
        json!({
            "name": "jit-2",
            "runner_group_id": default_id,
            "labels": ["build"],
            "work_folder": "jobs",
        }),
    );
    assert_eq!(custom.status, 201, "{}", custom.body);
    let custom_config: Value = serde_json::from_slice(
        &STANDARD
            .decode(custom.body["encoded_jit_config"].as_str().unwrap())
            .unwrap(),
    )
    .unwrap();
    assert_eq!(custom_config["workFolder"], "jobs");
    assert_eq!(
        api.post(
            &org("acme", "runners/generate-jitconfig"),
            json!({
                "name": "jit-4",
                "runner_group_id": default_id,
                "labels": ["build"],
                "work_folder": "",
            })
        )
        .status,
        422
    );
}

#[test]
fn runner_list_is_filtered_and_paginated() {
    let server = Server::start().unwrap();
    server.add_org("acme", &[]);
    let api = Api::admin(&server);
    for index in 0..5 {
        let token = registration_token(&api, "acme");
        server
            .register_runner("acme", &token, &format!("r{index}"), &[], "linux")
            .unwrap();
    }
    let page = api.get(&org("acme", "runners?per_page=2&page=2"));
    assert_eq!(page.status, 200);
    assert_eq!(page.body["total_count"].as_u64(), Some(5));
    assert_eq!(runner_names(&page.body), ["r2", "r3"]);
    let link = page.header("link").unwrap();
    assert!(
        link.contains("rel=\"next\"") && link.contains("rel=\"prev\""),
        "{link}"
    );

    let filtered = api.get(&org("acme", "runners?name=R3"));
    assert_eq!(filtered.body["total_count"].as_u64(), Some(1));
    assert_eq!(runner_names(&filtered.body), ["r3"]);
    assert_eq!(
        api.get(&org("acme", "runners?name=missing")).body["total_count"].as_u64(),
        Some(0)
    );
    assert_eq!(
        api.get(&format!("/api/v3{}", org("acme", "runners"))).body["total_count"].as_u64(),
        Some(5)
    );
}
