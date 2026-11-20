//! `gh secret set`, `list`, and `delete`.

use serde_json::{Value, json};

use crate::common::TempDir;
use crate::{gh, gh_json, gh_output, gh_output_as, tls_server};

use super::{assert_http, create_environment, gh_stdin, seed_actions_key};

fn names(list: &Value) -> Vec<&str> {
    list.as_array()
        .unwrap()
        .iter()
        .map(|item| item["name"].as_str().unwrap())
        .collect()
}

fn assert_stamp(list: &Value) {
    for item in list.as_array().unwrap() {
        let stamp = item["updatedAt"].as_str().unwrap();
        assert!(stamp.ends_with('Z') && stamp.len() > 10, "{stamp}");
    }
}

#[test]
fn repository_and_environment_secrets() {
    let server = tls_server();
    let config = TempDir::new();
    gh(
        &server,
        &config.0,
        &["repo", "create", "admin/tool", "--public"],
    );
    seed_actions_key(&server, "admin", "tool");
    create_environment(&server, &config.0, "admin/tool", "release");

    gh(
        &server,
        &config.0,
        &[
            "secret",
            "set",
            "TOKEN",
            "-R",
            "admin/tool",
            "--body",
            "s3cret",
        ],
    );
    gh_stdin(
        &server,
        &config.0,
        &["secret", "set", "FROM_STDIN", "-R", "admin/tool"],
        "piped-secret",
    );
    gh(
        &server,
        &config.0,
        &[
            "secret",
            "set",
            "TOKEN",
            "-R",
            "admin/tool",
            "--body",
            "rotated",
        ],
    );

    let listed = gh_json(
        &server,
        &config.0,
        &[
            "secret",
            "list",
            "-R",
            "admin/tool",
            "--json",
            "name,updatedAt",
        ],
    );
    assert_eq!(names(&listed), ["FROM_STDIN", "TOKEN"]);
    assert_stamp(&listed);
    let got = gh_json(
        &server,
        &config.0,
        &["api", "repos/admin/tool/actions/secrets/TOKEN"],
    );
    assert_eq!(got["name"], "TOKEN");
    assert!(got.get("value").is_none(), "{got}");
    assert!(got.get("encrypted_value").is_none(), "{got}");

    gh(
        &server,
        &config.0,
        &[
            "secret",
            "set",
            "DEPLOY",
            "--env",
            "release",
            "-R",
            "admin/tool",
            "--body",
            "ship",
        ],
    );
    gh_stdin(
        &server,
        &config.0,
        &[
            "secret",
            "set",
            "DEPLOY_STDIN",
            "--env",
            "release",
            "-R",
            "admin/tool",
        ],
        "via-stdin",
    );
    let environment = gh_json(
        &server,
        &config.0,
        &[
            "secret",
            "list",
            "--env",
            "release",
            "-R",
            "admin/tool",
            "--json",
            "name,updatedAt",
        ],
    );
    assert_eq!(names(&environment), ["DEPLOY", "DEPLOY_STDIN"]);
    assert_stamp(&environment);
    let stored = gh_json(
        &server,
        &config.0,
        &["api", "repos/admin/tool/environments/release/secrets"],
    );
    assert_eq!(stored["total_count"], 2);

    gh(
        &server,
        &config.0,
        &[
            "secret",
            "delete",
            "DEPLOY",
            "--env",
            "release",
            "-R",
            "admin/tool",
        ],
    );
    gh(
        &server,
        &config.0,
        &["secret", "delete", "TOKEN", "-R", "admin/tool"],
    );
    gh(
        &server,
        &config.0,
        &["secret", "delete", "FROM_STDIN", "-R", "admin/tool"],
    );
    assert_eq!(
        names(&gh_json(
            &server,
            &config.0,
            &["secret", "list", "-R", "admin/tool", "--json", "name"],
        )),
        Vec::<&str>::new()
    );
    assert_eq!(
        names(&gh_json(
            &server,
            &config.0,
            &[
                "secret",
                "list",
                "--env",
                "release",
                "-R",
                "admin/tool",
                "--json",
                "name",
            ],
        )),
        ["DEPLOY_STDIN"]
    );
    let gone = gh_output(
        &server,
        &config.0,
        &["api", "repos/admin/tool/actions/secrets/TOKEN"],
    );
    assert_http(&gone, "404");
}

#[test]
fn organization_secrets() {
    let server = tls_server();
    let config = TempDir::new();
    server.add_org("acme", &["admin"]);
    gh(
        &server,
        &config.0,
        &["repo", "create", "acme/tool", "--public"],
    );

    gh(
        &server,
        &config.0,
        &[
            "secret",
            "set",
            "ORG_ALL",
            "--org",
            "acme",
            "--visibility",
            "all",
            "--body",
            "everyone",
        ],
    );
    gh(
        &server,
        &config.0,
        &[
            "secret",
            "set",
            "ORG_PRIVATE",
            "--org",
            "acme",
            "--body",
            "owners",
        ],
    );
    gh_stdin(
        &server,
        &config.0,
        &[
            "secret",
            "set",
            "ORG_STDIN",
            "--org",
            "acme",
            "--visibility",
            "private",
        ],
        "piped-org",
    );
    gh(
        &server,
        &config.0,
        &[
            "secret",
            "set",
            "ORG_SELECTED",
            "--org",
            "acme",
            "--visibility",
            "selected",
            "--repos",
            "acme/tool",
            "--body",
            "just-tool",
        ],
    );
    gh(
        &server,
        &config.0,
        &[
            "secret",
            "set",
            "ORG_ALL",
            "--org",
            "acme",
            "--visibility",
            "private",
            "--body",
            "rotated",
        ],
    );

    let listed = gh_json(
        &server,
        &config.0,
        &[
            "secret",
            "list",
            "--org",
            "acme",
            "--json",
            "name,visibility,numSelectedRepos,updatedAt",
        ],
    );
    assert_stamp(&listed);
    let by_name = |name: &str| -> Value {
        listed
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["name"] == name)
            .unwrap_or_else(|| panic!("missing {name} in {listed}"))
            .clone()
    };
    assert_eq!(by_name("ORG_ALL")["visibility"], "private");
    assert_eq!(by_name("ORG_ALL")["numSelectedRepos"], 0);
    assert_eq!(by_name("ORG_PRIVATE")["visibility"], "private");
    assert_eq!(by_name("ORG_STDIN")["visibility"], "private");
    assert_eq!(by_name("ORG_SELECTED")["visibility"], "selected");
    assert_eq!(by_name("ORG_SELECTED")["numSelectedRepos"], 1);

    let repos = gh_json(
        &server,
        &config.0,
        &["api", "orgs/acme/actions/secrets/ORG_SELECTED/repositories"],
    );
    assert_eq!(repos["total_count"], 1);
    assert_eq!(repos["repositories"][0]["full_name"], "acme/tool");
    let selected = gh_json(
        &server,
        &config.0,
        &["api", "orgs/acme/actions/secrets/ORG_SELECTED"],
    );
    assert!(
        selected["selected_repositories_url"]
            .as_str()
            .unwrap()
            .ends_with("/orgs/acme/actions/secrets/ORG_SELECTED/repositories"),
        "{selected}"
    );
    let one = gh_json(
        &server,
        &config.0,
        &["api", "orgs/acme/actions/secrets/ORG_PRIVATE"],
    );
    assert_eq!(one["name"], "ORG_PRIVATE");
    assert!(one.get("value").is_none(), "{one}");
    assert!(one.get("encrypted_value").is_none(), "{one}");

    for name in ["ORG_ALL", "ORG_PRIVATE", "ORG_STDIN", "ORG_SELECTED"] {
        gh(
            &server,
            &config.0,
            &["secret", "delete", name, "--org", "acme"],
        );
    }
    assert_eq!(
        gh_json(
            &server,
            &config.0,
            &["secret", "list", "--org", "acme", "--json", "name"],
        ),
        json!([])
    );
}

#[test]
fn secret_permission_and_missing_name() {
    let server = tls_server();
    let config = TempDir::new();
    gh(
        &server,
        &config.0,
        &["repo", "create", "admin/tool", "--public"],
    );
    seed_actions_key(&server, "admin", "tool");
    let missing = gh_output(
        &server,
        &config.0,
        &["secret", "delete", "NO_SUCH", "-R", "admin/tool"],
    );
    assert_http(&missing, "404");

    let bob = server.add_user("bob").add_token(&["repo", "admin:org"]);
    let denied = gh_output_as(
        &server,
        &config.0,
        &bob,
        &[
            "secret",
            "set",
            "TOKEN",
            "-R",
            "admin/tool",
            "--body",
            "nope",
        ],
    );
    assert_http(&denied, "403");

    server.add_org("acme", &["admin", "bob"]);
    gh(
        &server,
        &config.0,
        &[
            "api",
            "-X",
            "PUT",
            "orgs/acme/memberships/bob",
            "-f",
            "role=member",
        ],
    );
    let org_denied = gh_output_as(
        &server,
        &config.0,
        &bob,
        &["secret", "set", "TOKEN", "--org", "acme", "--body", "nope"],
    );
    assert_http(&org_denied, "403");
}
