//! Repository administration: topics, Actions permissions, environments, their branch
//! policies and secrets, rulesets, and the plan limits on private repositories.

mod common;

use common::Api;
use ninot_github::Server;
use ninot_github::state::Plan;
use serde_json::json;

const PLAN_LIMIT: &str =
    "Upgrade to GitHub Pro or make this repository public to enable this feature.";

/// `alice` owning `acme`, `acme/tool` (private) and a token with `repo`.
fn setup() -> (Server, String) {
    let server = Server::start().unwrap();
    let alice = server.add_user("alice");
    server.add_org("acme", &["alice"]);
    server
        .create_repository("acme", "tool", true, None, None, "main")
        .unwrap();
    let token = alice.add_token(&["repo"]);
    (server, token)
}

#[test]
fn topics_replace_and_show_on_the_repository() {
    let (server, token) = setup();
    let api = Api::new(&server, Some(&token));
    assert!(api.get("/repos/acme/tool").body.get("topics").is_none());

    let put = api.put(
        "/repos/acme/tool/topics",
        json!({ "names": ["Rust", "testing", "rust"] }),
    );
    assert_eq!(put.status, 200);
    assert_eq!(put.body, json!({ "names": ["rust", "testing"] }));
    assert_eq!(
        api.get("/repos/acme/tool").body["topics"],
        json!(["rust", "testing"])
    );

    let bad = api.put("/repos/acme/tool/topics", json!({ "names": ["no spaces"] }));
    assert_eq!(bad.status, 422);
    assert_eq!(api.put("/repos/acme/tool/topics", json!({})).status, 422);

    let cleared = api.put("/repos/acme/tool/topics", json!({ "names": [] }));
    assert_eq!(cleared.body, json!({ "names": [] }));
}

#[test]
fn actions_permissions_round_trip() {
    let (server, token) = setup();
    let api = Api::new(&server, Some(&token));
    let perms = api.get("/repos/acme/tool/actions/permissions");
    assert_eq!(perms.status, 200);
    assert_eq!(perms.body["enabled"], true);
    assert_eq!(perms.body["allowed_actions"], "all");

    let put = api.put(
        "/repos/acme/tool/actions/permissions",
        json!({ "enabled": true, "allowed_actions": "local_only" }),
    );
    assert_eq!(put.status, 204);
    let perms = api.get("/repos/acme/tool/actions/permissions");
    assert_eq!(perms.body["allowed_actions"], "local_only");
    assert!(perms.body.get("selected_actions_url").is_none());

    let off = api.put(
        "/repos/acme/tool/actions/permissions",
        json!({ "enabled": false }),
    );
    assert_eq!(off.status, 204);
    let perms = api.get("/repos/acme/tool/actions/permissions");
    assert_eq!(perms.body, json!({ "enabled": false }));

    let workflow = api.get("/repos/acme/tool/actions/permissions/workflow");
    assert_eq!(
        workflow.body,
        json!({ "default_workflow_permissions": "read", "can_approve_pull_request_reviews": false })
    );
    let put = api.put(
        "/repos/acme/tool/actions/permissions/workflow",
        json!({ "default_workflow_permissions": "write", "can_approve_pull_request_reviews": true }),
    );
    assert_eq!(put.status, 204);
    let workflow = api.get("/repos/acme/tool/actions/permissions/workflow");
    assert_eq!(
        workflow.body,
        json!({ "default_workflow_permissions": "write", "can_approve_pull_request_reviews": true })
    );
    let bad = api.put(
        "/repos/acme/tool/actions/permissions/workflow",
        json!({ "default_workflow_permissions": "admin" }),
    );
    assert_eq!(bad.status, 422);
}

#[test]
fn settings_need_admin_rights() {
    let (server, _) = setup();
    let bob = server.add_user("bob");
    let api = Api::new(&server, Some(&bob.add_token(&["repo"])));
    // Not a collaborator of a private repository: it does not exist for bob.
    assert_eq!(api.get("/repos/acme/tool/actions/permissions").status, 404);
    assert_eq!(api.get("/repos/acme/tool/rulesets").status, 404);
}

#[test]
fn environment_with_reviewers_and_tag_policies() {
    let (server, token) = setup();
    let api = Api::new(&server, Some(&token));
    let alice_id = api.get("/users/alice").body["id"].clone();
    assert_eq!(
        api.get("/repos/acme/tool/environments/crates-io").status,
        404
    );

    let put = api.put(
        "/repos/acme/tool/environments/crates-io",
        json!({
            "reviewers": [{ "type": "User", "id": alice_id }],
            "prevent_self_review": true,
            "deployment_branch_policy": { "protected_branches": false, "custom_branch_policies": true },
        }),
    );
    assert_eq!(put.status, 200);
    assert_eq!(put.body["name"], "crates-io");
    let env = api.get("/repos/acme/tool/environments/crates-io");
    assert_eq!(env.status, 200);
    let rules = env.body["protection_rules"].as_array().unwrap();
    let reviewers = rules
        .iter()
        .find(|rule| rule["type"] == "required_reviewers")
        .unwrap();
    assert_eq!(reviewers["prevent_self_review"], true);
    assert_eq!(reviewers["reviewers"][0]["type"], "User");
    assert_eq!(reviewers["reviewers"][0]["reviewer"]["id"], alice_id);
    assert_eq!(reviewers["reviewers"][0]["reviewer"]["login"], "alice");
    assert!(rules.iter().any(|rule| rule["type"] == "branch_policy"));
    assert_eq!(
        env.body["deployment_branch_policy"],
        json!({ "protected_branches": false, "custom_branch_policies": true })
    );

    let policies = "/repos/acme/tool/environments/crates-io/deployment-branch-policies";
    let created = api.post(policies, json!({ "name": "v*", "type": "tag" }));
    assert_eq!(created.status, 200);
    assert_eq!(created.body["name"], "v*");
    assert_eq!(created.body["type"], "tag");
    let duplicate = api.post(policies, json!({ "name": "v*", "type": "tag" }));
    assert_eq!(duplicate.status, 303);
    let branch = api.post(policies, json!({ "name": "main" }));
    assert_eq!(branch.body["type"], "branch");
    let listed = api.get(policies);
    assert_eq!(listed.body["total_count"], 2);
    assert_eq!(listed.body["branch_policies"][0]["name"], "v*");
    assert_eq!(listed.body["branch_policies"][0]["type"], "tag");

    // Dropping the custom policy hides the listing again.
    let put = api.put(
        "/repos/acme/tool/environments/crates-io",
        json!({ "reviewers": [], "prevent_self_review": false, "deployment_branch_policy": null }),
    );
    assert_eq!(put.status, 200);
    let env = api.get("/repos/acme/tool/environments/crates-io");
    // The generated model omits a `None` policy and an empty rule list.
    assert!(env.body.get("deployment_branch_policy").is_none());
    assert!(env.body.get("protection_rules").is_none());
    assert_eq!(api.get(policies).status, 404);
    assert_eq!(
        api.post(policies, json!({ "name": "v*", "type": "tag" }))
            .status,
        404
    );
}

#[test]
fn environment_with_an_unknown_reviewer_fails() {
    let (server, token) = setup();
    let api = Api::new(&server, Some(&token));
    let put = api.put(
        "/repos/acme/tool/environments/prod",
        json!({ "reviewers": [{ "type": "User", "id": 999_999 }] }),
    );
    assert_eq!(put.status, 422);
    assert_eq!(api.get("/repos/acme/tool/environments/prod").status, 404);
}

#[test]
fn environment_secrets() {
    let (server, token) = setup();
    let api = Api::new(&server, Some(&token));
    let env = "/repos/acme/tool/environments/crates-io";
    assert_eq!(api.get(&format!("{env}/secrets")).status, 404);
    api.put(env, json!({}));

    let key = api.get(&format!("{env}/secrets/public-key"));
    assert_eq!(key.status, 200);
    let key_id = key.body["key_id"].as_str().unwrap().to_owned();
    assert_eq!(key.body["key"].as_str().unwrap().len(), 44);
    let secrets = api.get(&format!("{env}/secrets"));
    assert_eq!(secrets.body, json!({ "total_count": 0, "secrets": [] }));

    let body = json!({ "encrypted_value": "c2VjcmV0", "key_id": key_id });
    let created = api.put(&format!("{env}/secrets/TOKEN"), body.clone());
    assert_eq!(created.status, 201);
    assert_eq!(api.put(&format!("{env}/secrets/TOKEN"), body).status, 204);
    let wrong = api.put(
        &format!("{env}/secrets/OTHER"),
        json!({ "encrypted_value": "c2VjcmV0", "key_id": "nope" }),
    );
    assert_eq!(wrong.status, 422);
    let secrets = api.get(&format!("{env}/secrets"));
    assert_eq!(secrets.body["total_count"], 1);
    assert_eq!(secrets.body["secrets"][0]["name"], "TOKEN");
    assert!(secrets.body["secrets"][0]["created_at"].is_string());
}

fn tag_ruleset() -> serde_json::Value {
    json!({
        "name": "protect-release-tags",
        "target": "tag",
        "enforcement": "active",
        "conditions": { "ref_name": { "include": ["refs/tags/v*"], "exclude": [] } },
        "rules": [{ "type": "deletion" }, { "type": "update" }, { "type": "non_fast_forward" }],
        "bypass_actors": [],
    })
}

#[test]
fn rulesets_create_list_get_and_update() {
    let (server, token) = setup();
    let api = Api::new(&server, Some(&token));
    assert_eq!(api.get("/repos/acme/tool/rulesets").body, json!([]));

    let created = api.post("/repos/acme/tool/rulesets", tag_ruleset());
    assert_eq!(created.status, 201);
    let id = created.body["id"].as_i64().unwrap();
    assert_eq!(created.body["source_type"], "Repository");
    assert_eq!(created.body["source"], "acme/tool");
    assert_eq!(
        api.post("/repos/acme/tool/rulesets", tag_ruleset()).status,
        422
    );

    let listed = api.get("/repos/acme/tool/rulesets");
    assert_eq!(listed.body[0]["id"], id);
    assert_eq!(listed.body[0]["name"], "protect-release-tags");
    assert!(listed.body[0].get("rules").is_none());

    let got = api.get(&format!("/repos/acme/tool/rulesets/{id}"));
    assert_eq!(got.status, 200);
    assert_eq!(got.body["target"], "tag");
    assert_eq!(got.body["enforcement"], "active");
    assert_eq!(
        got.body["conditions"],
        json!({ "ref_name": { "include": ["refs/tags/v*"] } })
    );
    assert_eq!(
        got.body["rules"],
        json!([{ "type": "deletion" }, { "type": "update" }, { "type": "non_fast_forward" }])
    );
    assert!(got.body.get("bypass_actors").is_none());

    let mut changed = tag_ruleset();
    changed["conditions"]["ref_name"]["include"] = json!(["refs/tags/v*", "refs/tags/r*"]);
    changed["rules"] = json!([{ "type": "deletion" }]);
    let updated = api.put(&format!("/repos/acme/tool/rulesets/{id}"), changed);
    assert_eq!(updated.status, 200);
    let got = api.get(&format!("/repos/acme/tool/rulesets/{id}"));
    assert_eq!(
        got.body["conditions"]["ref_name"]["include"],
        json!(["refs/tags/v*", "refs/tags/r*"])
    );
    assert_eq!(got.body["rules"], json!([{ "type": "deletion" }]));
    assert_eq!(api.get("/repos/acme/tool/rulesets/12345").status, 404);
}

#[test]
fn free_plan_refuses_rulesets_and_reviewers_on_private_repositories() {
    let (server, token) = setup();
    server
        .org("acme")
        .unwrap()
        .set_plan(Some(Plan::Free))
        .unwrap();
    server
        .create_repository("acme", "open", false, None, None, "main")
        .unwrap();
    let api = Api::new(&server, Some(&token));
    assert_eq!(api.get("/orgs/acme").body["plan"]["name"], "free");
    let alice_id = api.get("/users/alice").body["id"].clone();
    let with_reviewer = json!({ "reviewers": [{ "type": "User", "id": alice_id }] });

    for reply in [
        api.get("/repos/acme/tool/rulesets"),
        api.post("/repos/acme/tool/rulesets", tag_ruleset()),
    ] {
        assert_eq!(reply.status, 403);
        assert_eq!(reply.body["message"], PLAN_LIMIT);
    }
    let refused = api.put(
        "/repos/acme/tool/environments/crates-io",
        with_reviewer.clone(),
    );
    assert_eq!(refused.status, 422);
    let env = api.put("/repos/acme/tool/environments/crates-io", json!({}));
    assert_eq!(env.status, 200);
    assert_eq!(
        api.get("/repos/acme/tool/environments/crates-io/secrets")
            .status,
        200
    );
    // Public repositories get every feature on every plan.
    assert_eq!(
        api.post("/repos/acme/open/rulesets", tag_ruleset()).status,
        201
    );
    assert_eq!(
        api.put("/repos/acme/open/environments/crates-io", with_reviewer)
            .status,
        200
    );
}

#[test]
fn team_plan_refuses_required_reviewers_on_private_repositories() {
    let (server, token) = setup();
    server
        .org("acme")
        .unwrap()
        .set_plan(Some(Plan::Team))
        .unwrap();
    let api = Api::new(&server, Some(&token));
    let alice_id = api.get("/users/alice").body["id"].clone();
    let refused = api.put(
        "/repos/acme/tool/environments/crates-io",
        json!({ "reviewers": [{ "type": "User", "id": alice_id }] }),
    );
    assert_eq!(refused.status, 422);
    assert!(
        refused.body["message"]
            .as_str()
            .unwrap()
            .contains("billing plan supports the required reviewers protection rule")
    );
    assert_eq!(
        api.get("/repos/acme/tool/environments/crates-io").status,
        404
    );
    assert_eq!(
        api.put(
            "/repos/acme/tool/environments/crates-io",
            json!({ "reviewers": [] })
        )
        .status,
        200
    );
    assert_eq!(
        api.post("/repos/acme/tool/rulesets", tag_ruleset()).status,
        201
    );
}

#[test]
fn unset_plan_is_not_rendered() {
    let (server, token) = setup();
    let api = Api::new(&server, Some(&token));
    assert!(api.get("/orgs/acme").body.get("plan").is_none());
}

#[test]
fn listing_all_refs_of_an_empty_repository_is_a_conflict() {
    let (server, token) = setup();
    let api = Api::new(&server, Some(&token));
    let refs = api.get("/repos/acme/tool/git/refs");
    assert_eq!(refs.status, 409);
    assert_eq!(refs.body["message"], "Git Repository is empty.");

    let repo = server.repository("acme", "tool").unwrap();
    repo.commit_file("main", std::path::Path::new("a"), b"a\n", "a")
        .unwrap();
    let refs = api.get("/repos/acme/tool/git/refs");
    assert_eq!(refs.status, 200);
    assert_eq!(refs.body[0]["ref"], "refs/heads/main");
}
