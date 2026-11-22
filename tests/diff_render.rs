//! Diff and patch media types compared with `git diff` and `git format-patch`.

mod common;

use std::fs;

use common::{Api, TempDir, clone_url, git_ok, git_ok_bytes};
use ninot_github::Server;
use serde_json::json;

/// Runs git with the output settings `git diff` and `format-patch` need for a stable comparison.
fn git_bytes(dir: &std::path::Path, args: &[&str]) -> Vec<u8> {
    let mut all = vec![
        "-c",
        "color.ui=false",
        "-c",
        "diff.mnemonicPrefix=false",
        "-c",
        "diff.noprefix=false",
        "-c",
        "core.quotepath=false",
    ];
    all.extend_from_slice(args);
    git_ok_bytes(dir, &all)
}

/// Drops the `2.x.y` line format-patch prints after `-- `.
fn without_version(text: &str) -> String {
    let mut out = String::new();
    let bytes = text.as_bytes();
    let mut index = 0;
    let marker = b"-- \n";
    while index < bytes.len() {
        if bytes[index..].starts_with(marker) {
            out.push_str("-- \n");
            index += marker.len();
            if index < bytes.len() && bytes[index].is_ascii_digit() {
                while index < bytes.len() && bytes[index] != b'\n' {
                    index += 1;
                }
                if index < bytes.len() && bytes[index] == b'\n' {
                    index += 1;
                }
            }
        } else {
            out.push(bytes[index] as char);
            index += 1;
        }
    }
    out
}

fn body_text(reply: &common::Reply) -> String {
    reply.body.as_str().unwrap().to_owned()
}

#[test]
fn diff_and_patch_text_matches_git() {
    let server = Server::start().unwrap();
    let admin = Api::admin(&server);
    assert_eq!(
        admin
            .post("/user/repos", json!({ "name": "tool", "auto_init": true }))
            .status,
        201
    );
    let url = clone_url(&server, "admin", "tool", Some(server.admin_token()));
    let work = TempDir::new();
    git_ok(&work.0, &["clone", "-q", &url, "c"]);
    let clone = work.join("c");
    git_ok(&clone, &["checkout", "-q", "-b", "feature"]);

    fs::write(clone.join("feature.txt"), "feature\n").unwrap();
    git_ok(&clone, &["add", "feature.txt"]);
    git_ok(&clone, &["commit", "-q", "-m", "Add feature"]);

    fs::write(clone.join("feature.txt"), "feature\nmore").unwrap();
    git_ok(&clone, &["add", "feature.txt"]);
    git_ok(&clone, &["commit", "-q", "-m", "Tweak\n\nbody line"]);

    fs::create_dir(clone.join("dir")).unwrap();
    fs::write(clone.join("dir/nested.txt"), "nested\n").unwrap();
    fs::write(clone.join("empty"), "").unwrap();
    git_ok(&clone, &["add", "dir/nested.txt", "empty"]);
    git_ok(&clone, &["commit", "-q", "-m", "Add nested and empty"]);

    git_ok(&clone, &["rm", "-q", "feature.txt"]);
    git_ok(&clone, &["commit", "-q", "-m", "Remove feature"]);
    git_ok(&clone, &["push", "-q", "origin", "feature"]);
    let text_patch = admin.get_accept(
        "/repos/admin/tool/compare/main...feature",
        "application/vnd.github.patch",
    );
    let base = git_ok(&clone, &["merge-base", "origin/main", "feature"]);
    let expected = git_bytes(
        &clone,
        &["format-patch", "--stdout", &format!("{base}..feature")],
    );
    assert_eq!(
        without_version(&body_text(&text_patch)),
        without_version(&String::from_utf8(expected).unwrap()),
        "text range patch"
    );

    fs::write(clone.join("bin.dat"), b"a\0b").unwrap();
    git_ok(&clone, &["add", "bin.dat"]);
    git_ok(&clone, &["commit", "-q", "-m", "Add binary"]);
    fs::write(clone.join("bin.dat"), b"a\0bc").unwrap();
    git_ok(&clone, &["add", "bin.dat"]);
    git_ok(&clone, &["commit", "-q", "-m", "Change binary"]);

    fs::write(clone.join("run.sh"), "y\n").unwrap();
    git_ok(&clone, &["add", "run.sh"]);
    git_ok(&clone, &["commit", "-q", "-m", "Add script"]);
    fs::write(clone.join("run.sh"), "y\nz\n").unwrap();
    git_ok(&clone, &["add", "--chmod=+x", "run.sh"]);
    git_ok(&clone, &["commit", "-q", "-m", "Change script and mode"]);
    git_ok(&clone, &["add", "--chmod=-x", "run.sh"]);
    git_ok(&clone, &["commit", "-q", "-m", "Mode only"]);

    let mut big = String::new();
    for line in 0..80 {
        big.push_str(&format!("line {line}\n"));
    }
    fs::write(clone.join("big.txt"), big).unwrap();
    git_ok(&clone, &["add", "big.txt"]);
    git_ok(&clone, &["commit", "-q", "-m", "Add big"]);

    git_ok(&clone, &["push", "-q", "origin", "feature"]);

    let shas = git_ok(&clone, &["rev-list", "--reverse", "origin/main..feature"]);
    for sha in shas.split_whitespace() {
        let diff = admin.get_accept(
            &format!("/repos/admin/tool/commits/{sha}"),
            "application/vnd.github.diff",
        );
        assert_eq!(diff.status, 200, "{sha}");
        let expected = git_bytes(
            &clone,
            &[
                "diff-tree",
                "--root",
                "--no-commit-id",
                "-p",
                "--no-color",
                "--no-renames",
                "--src-prefix=a/",
                "--dst-prefix=b/",
                sha,
            ],
        );
        assert_eq!(body_text(&diff).into_bytes(), expected, "diff {sha}");

        let patch = admin.get_accept(
            &format!("/repos/admin/tool/commits/{sha}"),
            "application/vnd.github.patch",
        );
        let expected = git_bytes(&clone, &["format-patch", "-1", "--stdout", sha]);
        let got = without_version(&body_text(&patch));
        let expected = without_version(&String::from_utf8(expected).unwrap());
        // format-patch of a binary commit is a GIT binary patch, which gix cannot emit.
        if expected.contains("GIT binary patch") {
            assert!(got.contains("Binary files "), "{got}");
            assert!(got.contains("Bin "), "{got}");
            continue;
        }
        assert_eq!(got, expected, "patch {sha}");
    }

    let range_diff = admin.get_accept(
        "/repos/admin/tool/compare/main...feature",
        "application/vnd.github.diff",
    );
    let expected = git_bytes(
        &clone,
        &[
            "diff",
            "--no-color",
            "--no-renames",
            "--src-prefix=a/",
            "--dst-prefix=b/",
            "origin/main...feature",
        ],
    );
    assert_eq!(body_text(&range_diff).into_bytes(), expected, "range diff");

    let range_patch = body_text(&admin.get_accept(
        "/repos/admin/tool/compare/main...feature",
        "application/vnd.github.patch",
    ));
    assert!(range_patch.contains("Subject: [PATCH "));
    assert!(range_patch.contains("Binary files "));

    assert_eq!(
        admin
            .delete("/repos/admin/tool/git/refs/heads/missing")
            .status,
        404
    );
    assert_eq!(
        admin
            .delete("/repos/admin/tool/git/refs/heads/foo..bar")
            .status,
        422
    );
    assert_eq!(
        admin
            .delete("/repos/admin/tool/git/refs/heads/feature")
            .status,
        204
    );
}
