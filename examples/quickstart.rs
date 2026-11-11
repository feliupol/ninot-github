//! Start a fake, commit one file and read it back over HTTP.
//!
//! Needs `git` on `PATH` because [`Repository::commit_file`](ninot_github::Repository::commit_file)
//! writes a real commit. Prints `skip: git is not installed` and exits successfully when it is
//! not.

#![deny(missing_docs)]

use std::path::Path;
use std::process::Command;

fn main() {
    if !git_installed() {
        println!("skip: git is not installed");
        return;
    }

    let server = ninot_github::Server::start().expect("start the fake");
    let alice = server.add_user("alice");
    let token = alice.add_token(&["repo"]);
    let repo = server
        .create_repository("alice", "hello", false, None, None, "main")
        .expect("create the repository");
    repo.commit_file("main", Path::new("README.md"), b"hello\n", "Initial commit")
        .expect("commit README.md");

    let url = format!("{}/repos/alice/hello/contents/README.md", server.url());
    let body = reqwest::blocking::Client::new()
        .get(url)
        .header("Authorization", format!("token {token}"))
        .header("Accept", "application/vnd.github.raw")
        .send()
        .expect("GET the file")
        .error_for_status()
        .expect("contents status")
        .text()
        .expect("contents body");
    assert_eq!(body, "hello\n");
    println!("README.md: {body}");
}

fn git_installed() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
}
