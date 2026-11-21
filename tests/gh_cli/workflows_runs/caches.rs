//! `gh cache list` and `gh cache delete`.

use serde_json::Value;

use super::assert_http;
use crate::common::TempDir;
use crate::{gh, gh_json, gh_output, gh_output_as, tls_server};

fn by_key<'a>(caches: &'a Value, key: &str) -> &'a Value {
    caches
        .as_array()
        .unwrap()
        .iter()
        .find(|cache| cache["key"] == key)
        .unwrap_or_else(|| panic!("no cache {key} in {caches}"))
}

#[test]
fn cache_list_filters_and_delete_by_id_and_key() {
    let server = tls_server();
    let config = TempDir::new();
    let repo = server
        .add_repository("admin", "tool", false, None, None, "main")
        .unwrap();
    repo.add_actions_cache("cargo-registry", "refs/heads/main", 12)
        .unwrap();
    repo.add_actions_cache("npm", "refs/heads/feature", 34)
        .unwrap();

    let caches = gh_json(
        &server,
        &config.0,
        &[
            "cache",
            "list",
            "-R",
            "admin/tool",
            "--json",
            "id,key,ref,sizeInBytes,version",
        ],
    );
    assert_eq!(caches.as_array().unwrap().len(), 2, "{caches}");
    let cargo = by_key(&caches, "cargo-registry");
    assert_eq!(cargo["ref"], "refs/heads/main");
    assert_eq!(cargo["sizeInBytes"], 12);
    assert_eq!(cargo["version"], "v1");
    let id = cargo["id"].as_i64().unwrap().to_string();

    let filtered = gh_json(
        &server,
        &config.0,
        &[
            "cache",
            "list",
            "-R",
            "admin/tool",
            "--key",
            "cargo",
            "--ref",
            "refs/heads/main",
            "--json",
            "key",
        ],
    );
    assert_eq!(filtered, serde_json::json!([{"key": "cargo-registry"}]));

    gh(
        &server,
        &config.0,
        &["cache", "delete", &id, "-R", "admin/tool"],
    );
    gh(
        &server,
        &config.0,
        &["cache", "delete", "npm", "-R", "admin/tool"],
    );
    let left = gh_json(
        &server,
        &config.0,
        &["cache", "list", "-R", "admin/tool", "--json", "key"],
    );
    assert_eq!(left, serde_json::json!([]));
}

#[test]
fn cache_delete_is_not_found_or_forbidden() {
    let server = tls_server();
    let config = TempDir::new();
    let repo = server
        .add_repository("admin", "tool", false, None, None, "main")
        .unwrap();
    let id = repo
        .add_actions_cache("cargo", "refs/heads/main", 8)
        .unwrap();
    let missing = gh_output(
        &server,
        &config.0,
        &["cache", "delete", "999999", "-R", "admin/tool"],
    );
    assert_http(&missing, "Could not find a cache");
    let bob = server.add_user("bob").add_token(&["repo"]);
    let forbidden = gh_output_as(
        &server,
        &config.0,
        &bob,
        &["cache", "delete", &id.to_string(), "-R", "admin/tool"],
    );
    assert_http(&forbidden, "403");
}
