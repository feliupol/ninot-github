//! `gh workflow`, `gh run`, and `gh cache` against the fake.

mod caches;
mod runs;
mod workflows;

use std::path::{Path, PathBuf};
use std::process::Output;

use ninot_github::Server;

use crate::{git, repo_with_branches};

const CI: &str = "\
name: CI
on:
  workflow_dispatch:
    inputs:
      name:
        description: who
        required: false
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi
";

const NIGHTLY: &str = "\
name: Nightly
on:
  workflow_dispatch:
jobs:
  nightly:
    runs-on: ubuntu-latest
    steps:
      - run: echo night
";

const LINT: &str = "\
name: Lint
on:
  push:
jobs:
  lint:
    runs-on: ubuntu-latest
    steps:
      - run: echo lint
";

fn assert_http(output: &Output, code: &str) {
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{stderr}");
    assert!(
        stderr.contains(code),
        "expected {code} in gh output:\n{stderr}"
    );
}

/// Pushes CI, Nightly, and Lint onto `main` and `feature` of `admin/tool`.
fn push_workflows(server: &Server, config: &Path) -> PathBuf {
    let work = repo_with_branches(server, config, &["feature"]);
    let workflows = work.join(".github/workflows");
    std::fs::create_dir_all(&workflows).unwrap();
    std::fs::write(workflows.join("ci.yml"), CI).unwrap();
    std::fs::write(workflows.join("nightly.yml"), NIGHTLY).unwrap();
    std::fs::write(workflows.join("lint.yml"), LINT).unwrap();
    git(server, config, &work, &["add", "."]);
    git(
        server,
        config,
        &work,
        &["commit", "-q", "-m", "Add workflows"],
    );
    git(server, config, &work, &["push", "-q", "origin", "main"]);
    // `feature` already has its own commit, so fast-forward it onto the workflow commit.
    git(server, config, &work, &["checkout", "-q", "feature"]);
    git(server, config, &work, &["merge", "-q", "main"]);
    git(server, config, &work, &["push", "-q", "origin", "feature"]);
    git(server, config, &work, &["checkout", "-q", "main"]);
    work
}

fn file_texts(dir: &Path) -> Vec<String> {
    let mut pending = vec![dir.to_path_buf()];
    let mut texts = Vec::new();
    while let Some(path) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&path) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if let Ok(text) = std::fs::read_to_string(&path) {
                texts.push(text);
            }
        }
    }
    texts
}
