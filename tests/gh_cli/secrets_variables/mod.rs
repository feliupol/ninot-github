//! `gh secret` and `gh variable` against the fake.

mod secrets;
mod variables;

use std::io::Write as _;
use std::path::Path;
use std::process::{Output, Stdio};

use ninot_github::Server;
use ninot_github::fixtures::ActionsPublicKeySeed;

use crate::{gh, gh_command};

/// 32-byte key `gh` can seal a repository secret to.
const KEY_ID: &str = "actions-key";
const PUBLIC_KEY: &str = "GBUauaBeS8oiYyuueOkybe/FFwjgFR5kwtyCWokXq7Q=";

fn seed_actions_key(server: &Server, owner: &str, repo: &str) {
    server
        .repository(owner, repo)
        .unwrap()
        .set_actions_public_key(ActionsPublicKeySeed {
            key_id: KEY_ID.to_owned(),
            key: PUBLIC_KEY.to_owned(),
        })
        .unwrap();
}

fn create_environment(server: &Server, config: &Path, repo: &str, name: &str) {
    std::fs::write(config.join("environment.json"), "{}").unwrap();
    gh(
        server,
        config,
        &[
            "api",
            "-X",
            "PUT",
            &format!("repos/{repo}/environments/{name}"),
            "--input",
            "environment.json",
        ],
    );
}

/// Runs `gh` with `body` on stdin. `Command::output` would close stdin empty.
fn gh_stdin(server: &Server, config: &Path, args: &[&str], body: &str) -> String {
    let mut child = gh_command(server, config, args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("cannot run gh");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(body.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "gh {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn assert_http(output: &Output, code: &str) {
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{stderr}");
    assert!(stderr.contains(code), "{stderr}");
}
