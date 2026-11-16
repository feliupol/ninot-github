//! `gh search` and `gh api` against reactions, search and the user endpoints.

use ninot_github::fixtures::{PullRequestSeed, ReviewCommentSeed};

use crate::common::TempDir;
use crate::{api, gh, gh_as, gh_json, tls_server};

const KEY: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl";

#[test]
fn search_commits_code_users_labels_and_topics() {
    let server = tls_server();
    let config = TempDir::new();
    gh(
        &server,
        &config.0,
        &[
            "api",
            "user/repos",
            "-f",
            "name=tool",
            "-F",
            "auto_init=true",
        ],
    );
    gh(
        &server,
        &config.0,
        &[
            "api",
            "-X",
            "PUT",
            "repos/admin/tool/contents/src/lib.rs",
            "-f",
            "message=Add the widget",
            "-f",
            "content=Zm4gd2lkZ2V0KCkge30K",
        ],
    );
    let commits = gh_json(
        &server,
        &config.0,
        &[
            "search",
            "commits",
            "widget",
            "--repo",
            "admin/tool",
            "--json",
            "sha,commit",
        ],
    );
    assert_eq!(
        commits[0]["commit"]["message"], "Add the widget",
        "{commits}"
    );
    let code = gh_json(
        &server,
        &config.0,
        &[
            "search",
            "code",
            "widget",
            "--repo",
            "admin/tool",
            "--json",
            "path",
        ],
    );
    assert_eq!(code[0]["path"], "src/lib.rs", "{code}");

    assert_eq!(
        api(
            &server,
            &config.0,
            "search/users?q=admin",
            ".items[0].login"
        ),
        "admin"
    );
    gh(
        &server,
        &config.0,
        &[
            "api",
            "repos/admin/tool/labels",
            "-f",
            "name=bug",
            "-f",
            "description=Broken",
        ],
    );
    let id = api(&server, &config.0, "repos/admin/tool", ".id");
    assert_eq!(
        api(
            &server,
            &config.0,
            &format!("search/labels?repository_id={id}&q=broken"),
            ".items[0].name"
        ),
        "bug"
    );
    assert_eq!(
        api(&server, &config.0, "search/topics?q=rust", ".total_count"),
        "0"
    );
}

#[test]
fn reactions_through_the_api() {
    let server = tls_server();
    let config = TempDir::new();
    gh(
        &server,
        &config.0,
        &[
            "api",
            "user/repos",
            "-f",
            "name=tool",
            "-F",
            "auto_init=true",
        ],
    );
    gh(
        &server,
        &config.0,
        &["api", "repos/admin/tool/issues", "-f", "title=Bug"],
    );
    let comment = gh(
        &server,
        &config.0,
        &[
            "api",
            "repos/admin/tool/issues/1/comments",
            "-f",
            "body=Seen",
            "--jq",
            ".id",
        ],
    );
    let comment = comment.trim();
    let pull = server
        .repository("admin", "tool")
        .unwrap()
        .create_pull_request(2, PullRequestSeed::default())
        .unwrap();
    pull.create_review_comment(ReviewCommentSeed::default())
        .unwrap();
    let review_comment = api(
        &server,
        &config.0,
        "repos/admin/tool/pulls/comments",
        ".[0].id",
    );
    let release = gh(
        &server,
        &config.0,
        &[
            "api",
            "repos/admin/tool/releases",
            "-f",
            "tag_name=v1",
            "--jq",
            ".id",
        ],
    );
    for path in [
        "repos/admin/tool/issues/1/reactions".to_owned(),
        format!("repos/admin/tool/issues/comments/{comment}/reactions"),
        format!("repos/admin/tool/pulls/comments/{review_comment}/reactions"),
        format!("repos/admin/tool/releases/{}/reactions", release.trim()),
    ] {
        let id = gh(
            &server,
            &config.0,
            &["api", &path, "-f", "content=heart", "--jq", ".id"],
        );
        assert_eq!(
            api(&server, &config.0, &path, ".[0].content"),
            "heart",
            "{path}"
        );
        gh(
            &server,
            &config.0,
            &["api", "-X", "DELETE", &format!("{path}/{}", id.trim())],
        );
        assert_eq!(api(&server, &config.0, &path, "length"), "0", "{path}");
    }
}

#[test]
fn the_user_profile_keys_and_follows() {
    let server = tls_server();
    let config = TempDir::new();
    let token =
        server
            .add_user("alice")
            .add_token(&["user", "write:public_key", "read:public_key"]);
    let alice = |args: &[&str]| gh_as(&server, &config.0, &token, args);
    assert_eq!(
        alice(&[
            "api", "-X", "PATCH", "user", "-f", "bio=Bots", "--jq", ".bio"
        ])
        .trim(),
        "Bots"
    );
    assert_eq!(
        alice(&["api", "user/emails", "--jq", ".[0].primary"]).trim(),
        "true"
    );
    alice(&[
        "api",
        "user/keys",
        "-f",
        "title=laptop",
        "-f",
        &format!("key={KEY}"),
    ]);
    assert_eq!(
        alice(&["api", "user/keys", "--jq", ".[0].title"]).trim(),
        "laptop"
    );
    assert_eq!(api(&server, &config.0, "users/alice/keys", ".[0].key"), KEY);
    alice(&["api", "-X", "PUT", "user/following/admin"]);
    assert_eq!(
        api(&server, &config.0, "users/admin/followers", ".[0].login"),
        "alice"
    );
    let id = api(&server, &config.0, "users/alice", ".id");
    assert_eq!(
        api(&server, &config.0, &format!("user/{id}"), ".login"),
        "alice"
    );
    assert_eq!(
        api(
            &server,
            &config.0,
            "users?per_page=100",
            "map(.login) | index(\"alice\") != null"
        ),
        "true"
    );
}
