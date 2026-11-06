//! The official GitHub CLI against the fake over HTTPS. Needs `gh` on `PATH`.
//!
//! Unix only: on Windows `gh` reads certificates from the system store and ignores
//! `SSL_CERT_FILE`, so it would not trust the fake's certificate authority.
//!
//! Each test's `gh` invocations and the fake's hits are logged to [`COVERAGE_LOG`], which
//! `cargo run --example gh_coverage` turns into a report and a ratchet.
#![cfg(unix)]

mod common;

use std::fs::File;
use std::io::Write as _;
use std::ops::Deref;
use std::path::Path;
use std::process::Command;
use std::sync::{Mutex, OnceLock};

use common::TempDir;
use ninot_github::Server;
use ninot_github::server::Options;
use serde_json::{Value, json};

/// JSON lines of `{"test", "gh"}` and `{"test", "hit"}`, rewritten by each run of this binary.
const COVERAGE_LOG: &str = concat!(env!("CARGO_TARGET_TMPDIR"), "/gh-coverage/gh_cli.jsonl");

fn log_coverage(entry: Value) {
    static LOG: OnceLock<Mutex<File>> = OnceLock::new();
    let log = LOG.get_or_init(|| {
        let path = Path::new(COVERAGE_LOG);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        Mutex::new(File::create(path).unwrap())
    });
    writeln!(log.lock().unwrap(), "{entry}").unwrap();
}

/// Name of the running test, which the test harness gives its thread.
fn test_name() -> String {
    std::thread::current()
        .name()
        .unwrap_or("unknown")
        .to_owned()
}

/// A fake that logs what it served when dropped.
struct Fake(Server);

impl Deref for Fake {
    type Target = Server;

    fn deref(&self) -> &Server {
        &self.0
    }
}

impl Drop for Fake {
    fn drop(&mut self) {
        let test = test_name();
        for hit in self.0.hits() {
            log_coverage(json!({ "test": test, "hit": hit.to_string() }));
        }
    }
}

/// Runs `gh` against `server` with an isolated configuration, returning stdout or panicking
/// with stderr.
fn gh(server: &Server, config: &Path, args: &[&str]) -> String {
    let output = gh_output(server, config, args);
    assert!(
        output.status.success(),
        "gh {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Runs `gh` and parses its stdout as JSON.
fn gh_json(server: &Server, config: &Path, args: &[&str]) -> Value {
    serde_json::from_str(&gh(server, config, args)).unwrap()
}

fn gh_output(server: &Server, config: &Path, args: &[&str]) -> std::process::Output {
    log_coverage(json!({ "test": test_name(), "gh": args }));
    isolated(Command::new("gh"), server, config)
        .args(args)
        .current_dir(config)
        .output()
        .expect("cannot run gh; install the GitHub CLI to run these tests")
}

/// Runs `git` in `dir` with the configuration `gh` sees, returning stdout or panicking with
/// stderr.
fn git(server: &Server, config: &Path, dir: &Path, args: &[&str]) -> String {
    let output = isolated(Command::new("git"), server, config)
        .args([
            "-c",
            "user.name=Admin",
            "-c",
            "user.email=admin@example.com",
        ])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Points `command` at `server` with its configuration, including git's, under `config`.
fn isolated(mut command: Command, server: &Server, config: &Path) -> Command {
    let host = server.url().strip_prefix("https://").unwrap();
    let ca = server.ca_cert_path().unwrap();
    command
        .env("GH_CONFIG_DIR", config)
        .env("GH_HOST", host)
        .env("GH_ENTERPRISE_TOKEN", server.admin_token())
        .env("SSL_CERT_FILE", &ca)
        .env("GIT_SSL_CAINFO", &ca)
        .env("GH_PROMPT_DISABLED", "1")
        .env("NO_COLOR", "1")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", config.join(".gitconfig"))
        .env("GIT_TERMINAL_PROMPT", "0")
        .env_remove("GH_TOKEN")
        .env_remove("GITHUB_TOKEN");
    command
}

fn tls_server() -> Fake {
    Fake(
        Server::start_with(Options {
            tls: true,
            ..Default::default()
        })
        .unwrap(),
    )
}

#[test]
fn auth_status_reports_the_token_and_scopes() {
    let server = tls_server();
    let config = TempDir::new();
    let status = gh_output(&server, &config.0, &["auth", "status"]);
    let text = String::from_utf8_lossy(&status.stdout).into_owned()
        + &String::from_utf8_lossy(&status.stderr);
    assert!(status.status.success(), "{text}");
    assert!(text.contains("Logged in to"), "{text}");
    assert!(text.contains("'delete_repo'"), "{text}");

    let token = server.admin_token();
    assert_eq!(gh(&server, &config.0, &["auth", "token"]).trim(), token);
    let host = server.url().strip_prefix("https://").unwrap();
    let shown = gh(
        &server,
        &config.0,
        &[
            "auth",
            "status",
            "--active",
            "--hostname",
            host,
            "--show-token",
        ],
    );
    assert!(shown.contains(&format!("Token: {token}")), "{shown}");
    let hosts = gh_json(&server, &config.0, &["auth", "status", "--json", "hosts"]);
    let account = &hosts["hosts"][host][0];
    assert_eq!(account["login"], "admin");
    assert_eq!(account["state"], "success");
    assert_eq!(account["tokenSource"], "GH_ENTERPRISE_TOKEN");
}

#[test]
fn repository_lifecycle() {
    let server = tls_server();
    server.add_org("acme", &["admin"]);
    let config = TempDir::new();

    let created = gh(
        &server,
        &config.0,
        &["repo", "create", "admin/tool", "--private", "--add-readme"],
    );
    assert!(created.trim().ends_with("/admin/tool"), "{created}");
    gh(
        &server,
        &config.0,
        &["repo", "create", "acme/shared", "--public"],
    );

    let listed: Value = serde_json::from_str(&gh(
        &server,
        &config.0,
        &[
            "repo",
            "list",
            "admin",
            "--json",
            "nameWithOwner,visibility",
        ],
    ))
    .unwrap();
    assert_eq!(
        listed,
        serde_json::json!([{ "nameWithOwner": "admin/tool", "visibility": "PRIVATE" }])
    );

    let viewed: Value = serde_json::from_str(&gh(
        &server,
        &config.0,
        &[
            "repo",
            "view",
            "acme/shared",
            "--json",
            "nameWithOwner,sshUrl,isPrivate",
        ],
    ))
    .unwrap();
    assert_eq!(viewed["nameWithOwner"], "acme/shared");
    assert_eq!(viewed["isPrivate"], false);
    assert!(
        viewed["sshUrl"]
            .as_str()
            .unwrap()
            .ends_with(":acme/shared.git")
    );

    gh(
        &server,
        &config.0,
        &["repo", "clone", "admin/tool", "clone"],
    );
    assert!(config.join("clone/README.md").exists());

    gh(
        &server,
        &config.0,
        &["repo", "delete", "acme/shared", "--yes"],
    );
    let missing = gh_output(&server, &config.0, &["repo", "view", "acme/shared"]);
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("Could not resolve to a Repository"));
}

#[test]
fn api_passthrough_for_rest_and_graphql() {
    let server = tls_server();
    let config = TempDir::new();
    assert_eq!(
        gh(&server, &config.0, &["api", "user", "--jq", ".login"]).trim(),
        "admin"
    );
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
    let refs = gh(
        &server,
        &config.0,
        &[
            "api",
            "repos/admin/tool/git/matching-refs/heads/",
            "--jq",
            ".[].ref",
        ],
    );
    assert_eq!(refs.trim(), "refs/heads/main");
    let login = gh(
        &server,
        &config.0,
        &[
            "api",
            "graphql",
            "-f",
            "query={viewer{login}}",
            "--jq",
            ".data.viewer.login",
        ],
    );
    assert_eq!(login.trim(), "admin");
}

#[test]
fn repo_edit_changes_settings() {
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
            "repo",
            "edit",
            "admin/tool",
            "--homepage",
            "https://example.com",
            "--enable-wiki=false",
            "--enable-issues=false",
            "--delete-branch-on-merge",
            "--enable-squash-merge=false",
            "--enable-rebase-merge=false",
        ],
    );
    let rest: Value = serde_json::from_str(&gh(
        &server,
        &config.0,
        &[
            "api",
            "repos/admin/tool",
            "--jq",
            "{homepage, has_wiki, has_issues, delete_branch_on_merge, allow_squash_merge, allow_rebase_merge, allow_merge_commit}",
        ],
    ))
    .unwrap();
    assert_eq!(
        rest,
        serde_json::json!({
            "homepage": "https://example.com",
            "has_wiki": false,
            "has_issues": false,
            "delete_branch_on_merge": true,
            "allow_squash_merge": false,
            "allow_rebase_merge": false,
            "allow_merge_commit": true,
        })
    );
    let viewed: Value = serde_json::from_str(&gh(
        &server,
        &config.0,
        &[
            "repo",
            "view",
            "admin/tool",
            "--json",
            "hasWikiEnabled,hasIssuesEnabled,squashMergeAllowed,rebaseMergeAllowed,mergeCommitAllowed",
        ],
    ))
    .unwrap();
    assert_eq!(
        viewed,
        serde_json::json!({
            "hasWikiEnabled": false,
            "hasIssuesEnabled": false,
            "squashMergeAllowed": false,
            "rebaseMergeAllowed": false,
            "mergeCommitAllowed": true,
        })
    );
}

#[test]
fn repo_create_pushes_a_master_source_as_the_default_branch() {
    let server = tls_server();
    let config = TempDir::new();
    let source = config.join("source");
    common::git_ok(&config.0, &["init", "-q", "-b", "master", "source"]);
    common::commit_file(&source, "one.txt", "1");
    gh(
        &server,
        &config.0,
        &[
            "repo",
            "create",
            "admin/tool",
            "--private",
            "--source",
            "source",
            "--push",
        ],
    );
    let viewed: Value = serde_json::from_str(&gh(
        &server,
        &config.0,
        &["repo", "view", "admin/tool", "--json", "defaultBranchRef"],
    ))
    .unwrap();
    assert_eq!(viewed["defaultBranchRef"]["name"], "master");
    assert_eq!(
        gh(
            &server,
            &config.0,
            &["api", "repos/admin/tool", "--jq", ".default_branch"],
        )
        .trim(),
        "master"
    );
}

#[test]
fn repository_settings() {
    let server = tls_server();
    let config = TempDir::new();
    gh(
        &server,
        &config.0,
        &["repo", "create", "admin/tool", "--private", "--add-readme"],
    );
    gh(
        &server,
        &config.0,
        &["repo", "create", "admin/other", "--private"],
    );

    gh(
        &server,
        &config.0,
        &[
            "repo",
            "edit",
            "admin/tool",
            "--description",
            "A tool",
            "--visibility",
            "public",
            "--accept-visibility-change-consequences",
        ],
    );
    let viewed = gh_json(
        &server,
        &config.0,
        &[
            "repo",
            "view",
            "admin/tool",
            "--json",
            "description,visibility,isPrivate",
        ],
    );
    assert_eq!(
        viewed,
        json!({ "description": "A tool", "visibility": "PUBLIC", "isPrivate": false })
    );
    let public = gh_json(
        &server,
        &config.0,
        &[
            "repo",
            "list",
            "admin",
            "--visibility",
            "public",
            "--json",
            "name",
        ],
    );
    assert_eq!(public, json!([{ "name": "tool" }]));
    let limited = gh_json(
        &server,
        &config.0,
        &["repo", "list", "admin", "--limit", "1", "--json", "name"],
    );
    assert_eq!(limited.as_array().unwrap().len(), 1);

    gh(
        &server,
        &config.0,
        &["repo", "rename", "gadget", "--repo", "admin/tool", "--yes"],
    );
    let renamed = gh_json(
        &server,
        &config.0,
        &["repo", "view", "admin/gadget", "--json", "name,url"],
    );
    assert_eq!(renamed["name"], "gadget");
    assert!(renamed["url"].as_str().unwrap().ends_with("/admin/gadget"));
}

#[test]
fn release_lifecycle() {
    let server = tls_server();
    let config = TempDir::new();
    std::fs::write(config.join("notes.txt"), "hello asset\n").unwrap();
    std::fs::write(config.join("second.bin"), "second\n").unwrap();
    let repo = ["--repo", "admin/tool"];
    let with_repo = |args: &[&'static str]| [args, &repo[..]].concat();
    gh(
        &server,
        &config.0,
        &["repo", "create", "admin/tool", "--private", "--add-readme"],
    );

    let created = gh(
        &server,
        &config.0,
        &with_repo(&[
            "release", "create", "v1.0", "--title", "First", "--notes", "One",
        ]),
    );
    assert!(
        created.trim().ends_with("/admin/tool/releases/tag/v1.0"),
        "{created}"
    );
    gh(
        &server,
        &config.0,
        &with_repo(&[
            "release",
            "create",
            "v1.1",
            "notes.txt#Release notes",
            "--title",
            "Second",
            "--notes",
            "Two",
            "--prerelease",
        ]),
    );
    let viewed = gh_json(
        &server,
        &config.0,
        &with_repo(&[
            "release",
            "view",
            "v1.1",
            "--json",
            "tagName,name,body,isPrerelease,assets",
        ]),
    );
    assert_eq!(viewed["tagName"], "v1.1");
    assert_eq!(viewed["name"], "Second");
    assert_eq!(viewed["body"], "Two");
    assert_eq!(viewed["isPrerelease"], true);
    let asset = &viewed["assets"][0];
    assert_eq!(asset["name"], "notes.txt");
    assert_eq!(asset["label"], "Release notes");
    assert_eq!(asset["size"], 12);
    let latest = gh_json(
        &server,
        &config.0,
        &with_repo(&["release", "view", "--json", "tagName"]),
    );
    assert_eq!(latest, json!({ "tagName": "v1.0" }));
    let listed = gh_json(
        &server,
        &config.0,
        &[
            "api",
            "repos/admin/tool/releases",
            "--jq",
            "map({tag_name, id})",
        ],
    );
    let second = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|release| release["tag_name"] == "v1.1")
        .unwrap_or_else(|| panic!("{listed}"));
    let release = format!("repos/admin/tool/releases/{}", second["id"]);
    let by_id = gh(&server, &config.0, &["api", &release, "--jq", ".name"]);
    assert_eq!(by_id.trim(), "Second");
    let assets = gh_json(
        &server,
        &config.0,
        &["api", &format!("{release}/assets"), "--jq", "[.[].name]"],
    );
    assert_eq!(assets, json!(["notes.txt"]));

    gh(
        &server,
        &config.0,
        &with_repo(&["release", "upload", "v1.0", "second.bin"]),
    );
    gh(
        &server,
        &config.0,
        &with_repo(&["release", "upload", "v1.0", "second.bin", "--clobber"]),
    );
    gh(
        &server,
        &config.0,
        &with_repo(&["release", "download", "v1.1", "--dir", "downloads"]),
    );
    assert_eq!(
        std::fs::read_to_string(config.join("downloads/notes.txt")).unwrap(),
        "hello asset\n"
    );
    gh(
        &server,
        &config.0,
        &with_repo(&[
            "release",
            "download",
            "v1.0",
            "--pattern",
            "*.bin",
            "--output",
            "out.bin",
        ]),
    );
    assert_eq!(
        std::fs::read_to_string(config.join("out.bin")).unwrap(),
        "second\n"
    );

    gh(
        &server,
        &config.0,
        &with_repo(&[
            "release", "edit", "v1.0", "--title", "Renamed", "--notes", "Edited",
        ]),
    );
    gh(
        &server,
        &config.0,
        &with_repo(&["release", "delete-asset", "v1.0", "second.bin", "--yes"]),
    );
    let edited = gh_json(
        &server,
        &config.0,
        &with_repo(&["release", "view", "v1.0", "--json", "name,body,assets"]),
    );
    assert_eq!(
        edited,
        json!({ "name": "Renamed", "body": "Edited", "assets": [] })
    );

    gh(
        &server,
        &config.0,
        &with_repo(&["release", "delete", "v1.1", "--yes"]),
    );
    let missing = gh_output(&server, &config.0, &with_repo(&["release", "view", "v1.1"]));
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("release not found"));
}

#[test]
fn pushed_commits_reach_the_api_and_webhooks() {
    let server = tls_server();
    let config = TempDir::new();
    let host = server.url().strip_prefix("https://").unwrap();
    gh(
        &server,
        &config.0,
        &["repo", "create", "admin/tool", "--private", "--add-readme"],
    );
    let hook = gh(
        &server,
        &config.0,
        &[
            "api",
            "repos/admin/tool/hooks",
            "-f",
            "config[url]=http://127.0.0.1:9/hook",
            "-f",
            "events[]=push",
            "--jq",
            ".id",
        ],
    );
    gh(
        &server,
        &config.0,
        &["auth", "setup-git", "--hostname", host],
    );
    gh(&server, &config.0, &["repo", "clone", "admin/tool", "work"]);

    let work = config.join("work");
    std::fs::write(work.join("lib.rs"), "fn main() {}\n").unwrap();
    git(&server, &config.0, &work, &["add", "lib.rs"]);
    git(&server, &config.0, &work, &["commit", "-m", "Add lib"]);
    git(&server, &config.0, &work, &["tag", "v1.0"]);
    git(
        &server,
        &config.0,
        &work,
        &["push", "origin", "main:feature", "v1.0"],
    );
    let head = git(&server, &config.0, &work, &["rev-parse", "HEAD"]);

    let feature = gh(
        &server,
        &config.0,
        &[
            "api",
            "repos/admin/tool/git/ref/heads/feature",
            "--jq",
            ".object.sha",
        ],
    );
    assert_eq!(feature.trim(), head.trim());
    let branches = gh_json(
        &server,
        &config.0,
        &["api", "repos/admin/tool/branches", "--jq", "[.[].name]"],
    );
    assert_eq!(branches, json!(["feature", "main"]));
    let tags = gh_json(
        &server,
        &config.0,
        &[
            "api",
            "repos/admin/tool/git/matching-refs/tags/",
            "--jq",
            "[.[].ref]",
        ],
    );
    assert_eq!(tags, json!(["refs/tags/v1.0"]));
    let size = gh(
        &server,
        &config.0,
        &[
            "api",
            "repos/admin/tool/contents/lib.rs?ref=feature",
            "--jq",
            ".size",
        ],
    );
    assert_eq!(size.trim(), "13");

    gh(
        &server,
        &config.0,
        &["repo", "edit", "admin/tool", "--default-branch", "feature"],
    );
    let default = gh(
        &server,
        &config.0,
        &[
            "repo",
            "view",
            "admin/tool",
            "--json",
            "defaultBranchRef",
            "--jq",
            ".defaultBranchRef.name",
        ],
    );
    assert_eq!(default.trim(), "feature");

    let events = gh_json(
        &server,
        &config.0,
        &[
            "api",
            &format!("repos/admin/tool/hooks/{}/deliveries", hook.trim()),
            "--jq",
            "[.[].event]",
        ],
    );
    assert!(
        events.as_array().unwrap().contains(&json!("push")),
        "{events}"
    );
}

#[test]
fn files_written_through_the_contents_api_reach_git() {
    let server = tls_server();
    let config = TempDir::new();
    let host = server.url().strip_prefix("https://").unwrap();
    gh(
        &server,
        &config.0,
        &["repo", "create", "admin/tool", "--private", "--add-readme"],
    );
    let path = "repos/admin/tool/contents/docs/notes.md";
    let created = gh_json(
        &server,
        &config.0,
        &[
            "api",
            "--method",
            "PUT",
            path,
            "--raw-field",
            "message=Add notes",
            "--raw-field",
            "content=bm90ZXMK",
            "--jq",
            "{path: .content.path, sha: .content.sha}",
        ],
    );
    assert_eq!(created["path"], "docs/notes.md");
    let sha = created["sha"].as_str().unwrap();
    gh(
        &server,
        &config.0,
        &[
            "api",
            "--method",
            "PUT",
            path,
            "--raw-field",
            "message=Edit notes",
            "--raw-field",
            "content=ZWRpdGVkCg==",
            "--raw-field",
            &format!("sha={sha}"),
        ],
    );

    gh(
        &server,
        &config.0,
        &["auth", "setup-git", "--hostname", host],
    );
    gh(&server, &config.0, &["repo", "clone", "admin/tool", "work"]);
    let work = config.join("work");
    assert_eq!(
        std::fs::read_to_string(work.join("docs/notes.md")).unwrap(),
        "edited\n"
    );
    let log = git(&server, &config.0, &work, &["log", "--format=%s"]);
    assert_eq!(
        log.lines().take(2).collect::<Vec<_>>(),
        ["Edit notes", "Add notes"]
    );
}

#[test]
fn webhook_lifecycle_through_the_api() {
    let server = tls_server();
    let config = TempDir::new();
    gh(
        &server,
        &config.0,
        &["repo", "create", "admin/tool", "--private"],
    );
    let id = gh(
        &server,
        &config.0,
        &[
            "api",
            "repos/admin/tool/hooks",
            "-f",
            "name=web",
            "-f",
            "config[url]=http://127.0.0.1:9/hook",
            "-f",
            "config[content_type]=json",
            "-f",
            "events[]=push",
            "--jq",
            ".id",
        ],
    );
    let hook = format!("repos/admin/tool/hooks/{}", id.trim());
    let urls = gh_json(
        &server,
        &config.0,
        &["api", "repos/admin/tool/hooks", "--jq", "[.[].config.url]"],
    );
    assert_eq!(urls, json!(["http://127.0.0.1:9/hook"]));
    let fetched = gh_json(&server, &config.0, &["api", &hook, "--jq", ".events"]);
    assert_eq!(fetched, json!(["push"]));
    let active = gh(
        &server,
        &config.0,
        &[
            "api",
            "-X",
            "PATCH",
            &hook,
            "-F",
            "active=false",
            "--jq",
            ".active",
        ],
    );
    assert_eq!(active.trim(), "false");

    let pinged = gh(
        &server,
        &config.0,
        &["api", "-X", "POST", &format!("{hook}/pings"), "--include"],
    );
    assert!(pinged.starts_with("HTTP/1.1 204"), "{pinged}");
    let deliveries = gh_json(
        &server,
        &config.0,
        &[
            "api",
            &format!("{hook}/deliveries"),
            "--jq",
            "[.[]|{event,status_code}]",
        ],
    );
    assert_eq!(deliveries, json!([{ "event": "ping", "status_code": 0 }]));

    gh(&server, &config.0, &["api", "-X", "DELETE", &hook]);
    let remaining = gh(
        &server,
        &config.0,
        &["api", "repos/admin/tool/hooks", "--jq", "length"],
    );
    assert_eq!(remaining.trim(), "0");
}

#[test]
fn api_request_options() {
    let server = tls_server();
    server.add_org("acme", &["admin"]);
    let config = TempDir::new();
    for name in ["one", "two", "three"] {
        gh(
            &server,
            &config.0,
            &[
                "api",
                "user/repos",
                "-f",
                &format!("name={name}"),
                "-F",
                "auto_init=true",
                "--silent",
            ],
        );
    }

    let paged = gh(
        &server,
        &config.0,
        &[
            "api",
            "user/repos?per_page=1",
            "--paginate",
            "--jq",
            ".[].name",
        ],
    );
    let mut names: Vec<&str> = paged.lines().collect();
    names.sort_unstable();
    assert_eq!(names, ["one", "three", "two"]);
    let graphql = gh(
        &server,
        &config.0,
        &[
            "api",
            "graphql",
            "--paginate",
            "-f",
            "query=query($endCursor: String) { viewer { repositories(first: 1, after: $endCursor) \
             { nodes { name } pageInfo { hasNextPage endCursor } } } }",
            "--jq",
            ".data.viewer.repositories.nodes[].name",
        ],
    );
    let mut names: Vec<&str> = graphql.lines().collect();
    names.sort_unstable();
    assert_eq!(names, ["one", "three", "two"]);

    let patched = gh(
        &server,
        &config.0,
        &[
            "api",
            "-X",
            "PATCH",
            "repos/admin/one",
            "-f",
            "description=patched",
            "--jq",
            ".description",
        ],
    );
    assert_eq!(patched.trim(), "patched");
    std::fs::write(config.join("body.json"), r#"{"description":"from input"}"#).unwrap();
    let from_input = gh(
        &server,
        &config.0,
        &[
            "api",
            "--method",
            "PATCH",
            "repos/admin/one",
            "--input",
            "body.json",
            "--jq",
            ".description",
        ],
    );
    assert_eq!(from_input.trim(), "from input");

    let raw = gh(
        &server,
        &config.0,
        &[
            "api",
            "repos/admin/one/contents/README.md",
            "-H",
            "Accept: application/vnd.github.raw",
        ],
    );
    assert_eq!(raw.trim(), "# one");
    let included = gh(&server, &config.0, &["api", "--include", "repos/admin/one"]);
    assert!(included.starts_with("HTTP/1.1 200 OK"), "{included}");
    assert!(included.contains("X-Oauth-Scopes: "), "{included}");
    assert!(gh(&server, &config.0, &["api", "--silent", "repos/admin/one"]).is_empty());
    assert_eq!(
        gh(
            &server,
            &config.0,
            &["api", "--template", "{{.login}}", "user"]
        ),
        "admin"
    );
    assert_eq!(
        gh(&server, &config.0, &["api", "orgs/acme", "--jq", ".login"]).trim(),
        "acme"
    );
    gh(
        &server,
        &config.0,
        &["api", "orgs/acme/repos", "-f", "name=shared", "--silent"],
    );
    assert_eq!(
        gh(
            &server,
            &config.0,
            &["api", "orgs/acme/repos", "--jq", ".[].name"]
        )
        .trim(),
        "shared"
    );
    assert_eq!(
        gh(
            &server,
            &config.0,
            &["api", "users/admin/repos", "--jq", ".[].full_name"]
        )
        .lines()
        .count(),
        3
    );
    let branch_head = gh(
        &server,
        &config.0,
        &[
            "api",
            "repos/admin/one/branches/main",
            "--jq",
            ".commit.sha",
        ],
    );
    let commit = gh(
        &server,
        &config.0,
        &["api", "repos/admin/one/commits/main", "--jq", ".sha"],
    );
    assert_eq!(commit, branch_head);
    assert!(!gh(&server, &config.0, &["api", "zen"]).trim().is_empty());
    assert_eq!(
        gh(
            &server,
            &config.0,
            &["api", "rate_limit", "--jq", ".resources.core.limit"]
        )
        .trim(),
        "5000"
    );

    let transferred = gh(
        &server,
        &config.0,
        &[
            "api",
            "-X",
            "POST",
            "repos/admin/two/transfer",
            "-f",
            "new_owner=acme",
            "--jq",
            ".full_name",
        ],
    );
    assert_eq!(transferred.trim(), "acme/two");
    gh(
        &server,
        &config.0,
        &["api", "-X", "DELETE", "repos/acme/two", "--silent"],
    );
    let gone = gh_output(&server, &config.0, &["api", "repos/acme/two"]);
    assert!(!gone.status.success());
    assert!(String::from_utf8_lossy(&gone.stderr).contains("HTTP 404"));
}
