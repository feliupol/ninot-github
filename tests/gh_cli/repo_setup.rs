//! Setting up a repository with `gh`: topics, Actions permissions, an environment with a
//! reviewer, a tag policy and a secret, a tag ruleset, and the refusals of a Free plan.

use ninot_github::state::Plan;
use serde_json::json;

use crate::common::TempDir;
use crate::{gh, gh_json, gh_output, tls_server};

#[test]
fn repo_edit_adds_topics() {
    let server = tls_server();
    let config = TempDir::new();
    gh(
        &server,
        &config.0,
        &["repo", "create", "admin/tool", "--private"],
    );

    gh(
        &server,
        &config.0,
        &["repo", "edit", "admin/tool", "--add-topic", "rust,testing"],
    );
    let topics = gh_json(
        &server,
        &config.0,
        &["api", "repos/admin/tool", "--jq", ".topics"],
    );
    assert_eq!(topics, json!(["rust", "testing"]));
}

#[test]
fn api_sets_actions_permissions() {
    let server = tls_server();
    let config = TempDir::new();
    gh(
        &server,
        &config.0,
        &["repo", "create", "admin/tool", "--private"],
    );

    gh(
        &server,
        &config.0,
        &[
            "api",
            "-X",
            "PUT",
            "repos/admin/tool/actions/permissions",
            "-F",
            "enabled=true",
            "-f",
            "allowed_actions=local_only",
        ],
    );
    let permissions = gh_json(
        &server,
        &config.0,
        &["api", "repos/admin/tool/actions/permissions"],
    );
    assert_eq!(permissions["allowed_actions"], "local_only");

    gh(
        &server,
        &config.0,
        &[
            "api",
            "-X",
            "PUT",
            "repos/admin/tool/actions/permissions/workflow",
            "-f",
            "default_workflow_permissions=read",
            "-F",
            "can_approve_pull_request_reviews=false",
        ],
    );
    let workflow = gh_json(
        &server,
        &config.0,
        &["api", "repos/admin/tool/actions/permissions/workflow"],
    );
    assert_eq!(
        workflow,
        json!({ "default_workflow_permissions": "read", "can_approve_pull_request_reviews": false })
    );
}

#[test]
fn environment_with_reviewer_tag_policy_and_secret() {
    let server = tls_server();
    let config = TempDir::new();
    gh(
        &server,
        &config.0,
        &["repo", "create", "admin/tool", "--private"],
    );
    let admin_id = gh(&server, &config.0, &["api", "users/admin", "--jq", ".id"]);
    let body = json!({
        "reviewers": [{ "type": "User", "id": admin_id.trim().parse::<u64>().unwrap() }],
        "prevent_self_review": false,
        "deployment_branch_policy": { "protected_branches": false, "custom_branch_policies": true },
    });
    std::fs::write(config.join("env.json"), body.to_string()).unwrap();
    gh(
        &server,
        &config.0,
        &[
            "api",
            "-X",
            "PUT",
            "repos/admin/tool/environments/crates-io",
            "--input",
            "env.json",
        ],
    );
    gh(
        &server,
        &config.0,
        &[
            "api",
            "-X",
            "POST",
            "repos/admin/tool/environments/crates-io/deployment-branch-policies",
            "-f",
            "name=v*",
            "-f",
            "type=tag",
        ],
    );
    let env = gh_json(
        &server,
        &config.0,
        &["api", "repos/admin/tool/environments/crates-io"],
    );
    assert_eq!(env["protection_rules"][0]["type"], "required_reviewers");
    assert_eq!(
        env["protection_rules"][0]["reviewers"][0]["reviewer"]["login"],
        "admin"
    );
    let policies = gh_json(
        &server,
        &config.0,
        &[
            "api",
            "repos/admin/tool/environments/crates-io/deployment-branch-policies",
        ],
    );
    assert_eq!(policies["branch_policies"][0]["type"], "tag");

    // `gh secret set` seals the value with the environment's public key.
    gh(
        &server,
        &config.0,
        &[
            "secret",
            "set",
            "CRATES_IO_TOKEN",
            "--env",
            "crates-io",
            "-R",
            "admin/tool",
            "--body",
            "value",
        ],
    );
    let secrets = gh_json(
        &server,
        &config.0,
        &["api", "repos/admin/tool/environments/crates-io/secrets"],
    );
    assert_eq!(secrets["secrets"][0]["name"], "CRATES_IO_TOKEN");
}

#[test]
fn api_creates_and_updates_a_tag_ruleset() {
    let server = tls_server();
    let config = TempDir::new();
    gh(
        &server,
        &config.0,
        &["repo", "create", "admin/tool", "--private"],
    );
    let mut ruleset = json!({
        "name": "protect-release-tags",
        "target": "tag",
        "enforcement": "active",
        "conditions": { "ref_name": { "include": ["refs/tags/v*"], "exclude": [] } },
        "rules": [{ "type": "deletion" }, { "type": "update" }, { "type": "non_fast_forward" }],
        "bypass_actors": [],
    });
    std::fs::write(config.join("ruleset.json"), ruleset.to_string()).unwrap();
    gh(
        &server,
        &config.0,
        &[
            "api",
            "-X",
            "POST",
            "repos/admin/tool/rulesets",
            "--input",
            "ruleset.json",
        ],
    );
    let id = gh(
        &server,
        &config.0,
        &["api", "repos/admin/tool/rulesets", "--jq", ".[0].id"],
    );
    let id = id.trim();

    ruleset["rules"] = json!([{ "type": "deletion" }]);
    std::fs::write(config.join("ruleset.json"), ruleset.to_string()).unwrap();
    gh(
        &server,
        &config.0,
        &[
            "api",
            "-X",
            "PUT",
            &format!("repos/admin/tool/rulesets/{id}"),
            "--input",
            "ruleset.json",
        ],
    );
    let got = gh_json(
        &server,
        &config.0,
        &["api", &format!("repos/admin/tool/rulesets/{id}")],
    );
    assert_eq!(got["rules"], json!([{ "type": "deletion" }]));
    assert_eq!(
        got["conditions"]["ref_name"]["include"],
        json!(["refs/tags/v*"])
    );
}

#[test]
fn free_plan_refusals_and_empty_repository_reach_gh() {
    let server = tls_server();
    let config = TempDir::new();
    server.add_org("acme", &["admin"]);
    server
        .org("acme")
        .unwrap()
        .set_plan(Some(Plan::Free))
        .unwrap();
    gh(
        &server,
        &config.0,
        &["repo", "create", "acme/tool", "--private"],
    );
    let plan = gh(
        &server,
        &config.0,
        &["api", "orgs/acme", "--jq", ".plan.name"],
    );
    assert_eq!(plan.trim(), "free");

    let refused = gh_output(&server, &config.0, &["api", "repos/acme/tool/rulesets"]);
    assert!(!refused.status.success());
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(
        stderr.contains("Upgrade to GitHub Pro or make this repository public")
            && stderr.contains("HTTP 403"),
        "{stderr}"
    );

    let empty = gh_output(&server, &config.0, &["api", "repos/acme/tool/git/refs"]);
    let stderr = String::from_utf8_lossy(&empty.stderr);
    assert!(
        stderr.contains("Git Repository is empty") && stderr.contains("HTTP 409"),
        "{stderr}"
    );
}
