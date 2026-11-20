//! `gh label` against the fake.

use crate::common::TempDir;
use crate::{api, gh, gh_json, tls_server};

#[test]
fn label_create_edit_and_delete() {
    let server = tls_server();
    let config = TempDir::new();
    gh(
        &server,
        &config.0,
        &["repo", "create", "admin/tool", "--public"],
    );

    gh(
        &server,
        &config.0,
        &[
            "label",
            "create",
            "triage",
            "-R",
            "admin/tool",
            "--color",
            "ff0000",
            "--description",
            "Needs a look",
        ],
    );
    assert_eq!(
        api(
            &server,
            &config.0,
            "repos/admin/tool/labels/triage",
            "[.color, .description] | join(\",\")"
        ),
        "ff0000,Needs a look"
    );
    gh(
        &server,
        &config.0,
        &[
            "label",
            "create",
            "triage",
            "-R",
            "admin/tool",
            "--color",
            "00ff00",
            "--force",
        ],
    );
    assert_eq!(
        api(
            &server,
            &config.0,
            "repos/admin/tool/labels/triage",
            ".color"
        ),
        "00ff00"
    );

    gh(
        &server,
        &config.0,
        &[
            "label",
            "edit",
            "triage",
            "-R",
            "admin/tool",
            "--name",
            "needs-triage",
            "--color",
            "0000ff",
            "--description",
            "Unsorted",
        ],
    );
    assert_eq!(
        api(
            &server,
            &config.0,
            "repos/admin/tool/labels/needs-triage",
            "[.name, .color, .description] | join(\",\")"
        ),
        "needs-triage,0000ff,Unsorted"
    );

    gh(
        &server,
        &config.0,
        &[
            "label",
            "delete",
            "needs-triage",
            "-R",
            "admin/tool",
            "--yes",
        ],
    );
    assert_eq!(
        api(&server, &config.0, "repos/admin/tool/labels", "length"),
        "0"
    );
}

#[test]
fn label_list_shows_name_color_and_description() {
    let server = tls_server();
    let config = TempDir::new();
    gh(
        &server,
        &config.0,
        &["repo", "create", "admin/tool", "--public"],
    );
    gh(
        &server,
        &config.0,
        &[
            "label",
            "create",
            "bug",
            "-R",
            "admin/tool",
            "-c",
            "d73a4a",
            "-d",
            "Broken",
        ],
    );

    let listed = gh_json(
        &server,
        &config.0,
        &[
            "label",
            "list",
            "-R",
            "admin/tool",
            "--json",
            "name,color,description",
        ],
    );
    assert_eq!(
        listed,
        serde_json::json!([{ "name": "bug", "color": "d73a4a", "description": "Broken" }])
    );
}
