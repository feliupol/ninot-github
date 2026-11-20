//! `gh variable set`, `get`, `list`, and `delete`.

use serde_json::{Value, json};

use crate::common::TempDir;
use crate::{api, gh, gh_json, gh_output, gh_output_as, tls_server};

use super::{assert_http, create_environment, gh_stdin};

fn names(list: &Value) -> Vec<&str> {
    list.as_array()
        .unwrap()
        .iter()
        .map(|item| item["name"].as_str().unwrap())
        .collect()
}

#[test]
fn repository_and_environment_variables() {
    let server = tls_server();
    let config = TempDir::new();
    gh(
        &server,
        &config.0,
        &["repo", "create", "admin/tool", "--public"],
    );
    create_environment(&server, &config.0, "admin/tool", "release");

    gh(
        &server,
        &config.0,
        &[
            "variable",
            "set",
            "build_env",
            "-R",
            "admin/tool",
            "--body",
            "ci",
        ],
    );
    let got = gh_json(
        &server,
        &config.0,
        &[
            "variable",
            "get",
            "BUILD_ENV",
            "-R",
            "admin/tool",
            "--json",
            "name,value,createdAt,updatedAt",
        ],
    );
    assert_eq!(got["name"], "BUILD_ENV");
    assert_eq!(got["value"], "ci");
    assert!(got["createdAt"].as_str().unwrap().ends_with('Z'));
    assert!(got["updatedAt"].as_str().unwrap().ends_with('Z'));

    gh(
        &server,
        &config.0,
        &[
            "variable",
            "set",
            "Build_Env",
            "-R",
            "admin/tool",
            "--body",
            "ci-2",
        ],
    );
    gh_stdin(
        &server,
        &config.0,
        &["variable", "set", "FROM_STDIN", "-R", "admin/tool"],
        "piped-value",
    );
    let listed = gh_json(
        &server,
        &config.0,
        &[
            "variable",
            "list",
            "-R",
            "admin/tool",
            "--json",
            "name,value",
        ],
    );
    assert_eq!(
        listed,
        json!([
            {"name": "BUILD_ENV", "value": "ci-2"},
            {"name": "FROM_STDIN", "value": "piped-value"},
        ])
    );
    assert_eq!(
        api(
            &server,
            &config.0,
            "repos/admin/tool/actions/variables/BUILD_ENV",
            ".value",
        ),
        "ci-2"
    );

    gh(
        &server,
        &config.0,
        &[
            "variable",
            "set",
            "CHANNEL",
            "--env",
            "release",
            "-R",
            "admin/tool",
            "--body",
            "beta",
        ],
    );
    gh(
        &server,
        &config.0,
        &[
            "variable",
            "set",
            "CHANNEL",
            "--env",
            "release",
            "-R",
            "admin/tool",
            "--body",
            "stable",
        ],
    );
    gh_stdin(
        &server,
        &config.0,
        &[
            "variable",
            "set",
            "NOTES",
            "--env",
            "release",
            "-R",
            "admin/tool",
        ],
        "hello",
    );
    assert_eq!(
        gh_json(
            &server,
            &config.0,
            &[
                "variable",
                "get",
                "CHANNEL",
                "--env",
                "release",
                "-R",
                "admin/tool",
                "--json",
                "value",
            ],
        )["value"],
        "stable"
    );
    let environment = gh_json(
        &server,
        &config.0,
        &[
            "variable",
            "list",
            "--env",
            "release",
            "-R",
            "admin/tool",
            "--json",
            "name,value",
        ],
    );
    assert_eq!(
        environment,
        json!([
            {"name": "CHANNEL", "value": "stable"},
            {"name": "NOTES", "value": "hello"},
        ])
    );

    gh(
        &server,
        &config.0,
        &["variable", "delete", "BUILD_ENV", "-R", "admin/tool"],
    );
    gh(
        &server,
        &config.0,
        &[
            "variable",
            "delete",
            "channel",
            "--env",
            "release",
            "-R",
            "admin/tool",
        ],
    );
    assert_eq!(
        names(&gh_json(
            &server,
            &config.0,
            &["variable", "list", "-R", "admin/tool", "--json", "name"],
        )),
        ["FROM_STDIN"]
    );
    assert_eq!(
        names(&gh_json(
            &server,
            &config.0,
            &[
                "variable",
                "list",
                "--env",
                "release",
                "-R",
                "admin/tool",
                "--json",
                "name",
            ],
        )),
        ["NOTES"]
    );
}

#[test]
fn organization_variables() {
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
            "variable",
            "set",
            "GLOBAL",
            "--org",
            "acme",
            "--visibility",
            "all",
            "--body",
            "yes",
        ],
    );
    gh(
        &server,
        &config.0,
        &["variable", "set", "HIDDEN", "--org", "acme", "--body", "no"],
    );
    gh_stdin(
        &server,
        &config.0,
        &[
            "variable",
            "set",
            "PIPED",
            "--org",
            "acme",
            "--visibility",
            "private",
        ],
        "pipe",
    );
    gh(
        &server,
        &config.0,
        &[
            "variable",
            "set",
            "region",
            "--org",
            "acme",
            "--visibility",
            "selected",
            "--repos",
            "acme/tool",
            "--body",
            "eu",
        ],
    );
    gh(
        &server,
        &config.0,
        &[
            "variable",
            "set",
            "REGION",
            "--org",
            "acme",
            "--visibility",
            "selected",
            "--repos",
            "acme/tool",
            "--body",
            "eu-west",
        ],
    );

    let got = gh_json(
        &server,
        &config.0,
        &[
            "variable",
            "get",
            "region",
            "--org",
            "acme",
            "--json",
            "name,value,visibility,numSelectedRepos",
        ],
    );
    assert_eq!(got["name"], "REGION");
    assert_eq!(got["value"], "eu-west");
    assert_eq!(got["visibility"], "selected");
    assert_eq!(got["numSelectedRepos"], 1);

    let listed = gh_json(
        &server,
        &config.0,
        &[
            "variable",
            "list",
            "--org",
            "acme",
            "--json",
            "name,value,visibility,numSelectedRepos",
        ],
    );
    let region = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["name"] == "REGION")
        .unwrap();
    assert_eq!(region["value"], "eu-west");
    assert_eq!(region["numSelectedRepos"], 1);
    let hidden = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["name"] == "HIDDEN")
        .unwrap();
    assert_eq!(hidden["visibility"], "private");
    assert_eq!(hidden["value"], "no");
    assert_eq!(
        api(
            &server,
            &config.0,
            "orgs/acme/actions/variables/PIPED",
            ".value",
        ),
        "pipe"
    );
    let repos = gh_json(
        &server,
        &config.0,
        &["api", "orgs/acme/actions/variables/REGION/repositories"],
    );
    assert_eq!(repos["total_count"], 1);
    assert_eq!(repos["repositories"][0]["full_name"], "acme/tool");

    for name in ["GLOBAL", "HIDDEN", "PIPED", "REGION"] {
        gh(
            &server,
            &config.0,
            &["variable", "delete", name, "--org", "acme"],
        );
    }
    assert_eq!(
        gh_json(
            &server,
            &config.0,
            &["variable", "list", "--org", "acme", "--json", "name"],
        ),
        json!([])
    );
}

#[test]
fn variable_permission_and_missing_name() {
    let server = tls_server();
    let config = TempDir::new();
    gh(
        &server,
        &config.0,
        &["repo", "create", "admin/tool", "--public"],
    );
    let missing = gh_output(
        &server,
        &config.0,
        &["variable", "get", "NO_SUCH", "-R", "admin/tool"],
    );
    // `gh variable get` reports a missing name without the status code.
    assert_http(&missing, "not found");

    let bob = server.add_user("bob").add_token(&["repo", "admin:org"]);
    let denied = gh_output_as(
        &server,
        &config.0,
        &bob,
        &[
            "variable",
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
        &[
            "variable",
            "set",
            "TOKEN",
            "--org",
            "acme",
            "--visibility",
            "all",
            "--body",
            "nope",
        ],
    );
    assert_http(&org_denied, "403");
}
