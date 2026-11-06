//! Git commits made by fixture setup, against the same repository clients fetch.

use std::fs;
use std::io;
use std::path::{Component, Path};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

/// Who authored or committed a commit, and when, as git accepts it in `GIT_*_DATE`.
pub(crate) struct Identity<'a> {
    pub name: &'a str,
    pub email: &'a str,
    pub date: &'a str,
}

/// Fixture commits are authored by the fake at a fixed date so their shas are reproducible.
const FIXTURE_IDENTITY: Identity<'static> = Identity {
    name: "ninot-github",
    email: "ninot-github@localhost",
    date: "2000-01-01T00:00:00+00:00",
};

/// A file to commit on a branch, and who commits it.
pub(crate) struct Commit<'a> {
    pub git_dir: &'a Path,
    pub branch: &'a str,
    /// Where a branch that does not exist yet starts from.
    pub default_branch: &'a str,
    pub path: &'a Path,
    pub bytes: &'a [u8],
    pub message: &'a str,
    pub author: &'a Identity<'a>,
    pub committer: &'a Identity<'a>,
}

/// Commits bytes on a repository branch, creating the branch from its default branch.
///
/// # Errors
/// Returns invalid paths, missing repositories, and Git failures without changing the target ref.
pub(crate) fn commit_file(
    git_dir: &Path,
    branch: &str,
    default_branch: &str,
    path: &Path,
    bytes: &[u8],
    message: &str,
) -> io::Result<String> {
    commit(Commit {
        git_dir,
        branch,
        default_branch,
        path,
        bytes,
        message,
        author: &FIXTURE_IDENTITY,
        committer: &FIXTURE_IDENTITY,
    })
}

/// Commits `commit.bytes` at `commit.path` and returns the new commit's sha.
///
/// # Errors
/// Returns invalid paths, missing repositories, and Git failures without changing the target ref.
pub(crate) fn commit(commit: Commit<'_>) -> io::Result<String> {
    validate_path(commit.path)?;
    let root = std::env::temp_dir().join(format!(
        "ninot-github-fixture-{}-{}",
        std::process::id(),
        NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&root)?;
    let isolation = root.with_extension("git-isolation");
    prepare_git_isolation(&isolation)?;
    let result = commit_in_worktree(commit, &root, &isolation);
    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(isolation);
    result
}

/// Copies branch refs and their objects from an upstream repo into a fork fixture.
pub(crate) fn copy_refs(target: &Path, source: &Path) -> io::Result<()> {
    let root = target
        .parent()
        .ok_or_else(|| io::Error::other("fixture repository has no parent"))?;
    prepare_git_isolation(root)?;
    let output = isolated_git(root)
        .args(["--git-dir"])
        .arg(target)
        .arg("fetch")
        .arg("--no-tags")
        .arg("--")
        .arg(source)
        .args(["+refs/heads/*:refs/heads/*"])
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "copying fixture fork refs failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

fn validate_path(path: &Path) -> io::Result<()> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
        || path
            .components()
            .any(|part| part.as_os_str().eq_ignore_ascii_case(".git"))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "fixture path must stay inside the repository",
        ));
    }
    Ok(())
}

fn commit_in_worktree(commit: Commit<'_>, worktree: &Path, isolation: &Path) -> io::Result<String> {
    run(isolated_git(isolation)
        .args(["clone", "--quiet", "--no-hardlinks"])
        .arg(commit.git_dir)
        .arg(worktree))?;
    let existing_branch = run(isolated_git(isolation).current_dir(worktree).args([
        "show-ref",
        "--verify",
        &format!("refs/remotes/origin/{}", commit.branch),
    ]));
    let source_branch = if existing_branch.is_ok() {
        commit.branch
    } else {
        commit.default_branch
    };
    let source_exists = run(isolated_git(isolation).current_dir(worktree).args([
        "show-ref",
        "--verify",
        &format!("refs/remotes/origin/{source_branch}"),
    ]));
    if source_exists.is_ok() {
        run(isolated_git(isolation).current_dir(worktree).args([
            "checkout",
            "--quiet",
            "-B",
            commit.branch,
            &format!("origin/{source_branch}"),
        ]))?;
    } else {
        run(isolated_git(isolation).current_dir(worktree).args([
            "checkout",
            "--quiet",
            "--orphan",
            commit.branch,
        ]))?;
    }
    let target = worktree.join(commit.path);
    let mut parent = worktree.to_owned();
    for component in commit.path.components() {
        parent.push(component);
        if fs::symlink_metadata(&parent).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "fixture path contains a symlink",
            ));
        }
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&target, commit.bytes)?;
    run(isolated_git(isolation)
        .current_dir(worktree)
        .args(["add", "--"])
        .arg(commit.path))?;
    let mut command = isolated_git(isolation);
    command
        .current_dir(worktree)
        .env("GIT_AUTHOR_NAME", commit.author.name)
        .env("GIT_AUTHOR_EMAIL", commit.author.email)
        .env("GIT_AUTHOR_DATE", commit.author.date)
        .env("GIT_COMMITTER_NAME", commit.committer.name)
        .env("GIT_COMMITTER_EMAIL", commit.committer.email)
        .env("GIT_COMMITTER_DATE", commit.committer.date)
        .args(["commit", "--quiet", "--allow-empty", "-m", commit.message]);
    run(&mut command)?;
    let sha = output(
        isolated_git(isolation)
            .current_dir(worktree)
            .args(["rev-parse", "HEAD"]),
    )?;
    run(isolated_git(isolation).current_dir(worktree).args([
        "push",
        "--quiet",
        "origin",
        &format!("HEAD:refs/heads/{}", commit.branch),
    ]))?;
    Ok(sha.trim().to_owned())
}

fn prepare_git_isolation(root: &Path) -> io::Result<()> {
    fs::create_dir_all(root.join("disabled-hooks"))?;
    fs::write(root.join("empty.gitconfig"), b"")
}

fn isolated_git(root: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", root.join("empty.gitconfig"))
        .arg("-c")
        .arg(format!(
            "core.hooksPath={}",
            root.join("disabled-hooks").display()
        ));
    command
}

fn run(command: &mut Command) -> io::Result<()> {
    let output = command
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()?;
    if output.status.success() {
        Ok(())
    } else {
        Err(io::Error::other(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ))
    }
}

fn output(command: &mut Command) -> io::Result<String> {
    let output = command.stderr(Stdio::piped()).output()?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(io::Error::other(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ))
    }
}
