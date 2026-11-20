//! Git commits made by fixture setup, written straight into the bare repository.
//!
//! Hooks stay disabled because nothing here runs `git` or an isolated worktree.

use std::collections::HashSet;
use std::io;
use std::path::{Component, Path};

use gix::objs::Write;
use gix::objs::tree::EntryKind;

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
    let path = commit.path.to_str().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "fixture path must stay inside the repository",
        )
    })?;
    let repo = open(commit.git_dir)?;
    let branch_tip = tip(&repo, commit.branch)?;
    let parent = match branch_tip {
        Some(tip) => Some(tip),
        None => tip(&repo, commit.default_branch)?,
    };
    let tree_id = match parent {
        Some(parent) => repo
            .find_commit(parent)
            .map_err(other)?
            .tree_id()
            .map_err(other)?
            .detach(),
        None => repo.empty_tree().id().detach(),
    };
    if parent.is_some() && contains_symlink(&repo, tree_id, path)? {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "fixture path contains a symlink",
        ));
    }
    let blob = repo.write_blob(commit.bytes).map_err(other)?.detach();
    let mut editor = repo
        .find_tree(tree_id)
        .map_err(other)?
        .edit()
        .map_err(other)?;
    editor.upsert(path, EntryKind::Blob, blob).map_err(other)?;
    let tree = editor.write().map_err(other)?.detach();
    let author = signature(commit.author)?;
    let committer = signature(commit.committer)?;
    let mut author_time = gix::date::parse::TimeBuf::default();
    let mut committer_time = gix::date::parse::TimeBuf::default();
    let parents = parent.into_iter().collect::<Vec<_>>();
    let id = repo
        .commit_as(
            committer.to_ref(&mut committer_time),
            author.to_ref(&mut author_time),
            format!("refs/heads/{}", commit.branch).as_str(),
            complete_line(commit.message).as_str(),
            tree,
            parents,
        )
        .map_err(other)?;
    Ok(id.detach().to_string())
}

/// Copies branch refs and the objects they reach from `source` into `target`.
///
/// Tags and `HEAD` stay where they are. Existing target branches are overwritten.
pub(crate) fn copy_refs(target: &Path, source: &Path) -> io::Result<()> {
    let src = open(source)?;
    let dst = open(target)?;
    let mut refs = Vec::new();
    for reference in src
        .references()
        .map_err(other)?
        .local_branches()
        .map_err(other)?
    {
        let reference = reference.map_err(other)?;
        let Some(id) = reference.try_id() else {
            continue;
        };
        refs.push((reference.name().as_bstr().to_string(), id.detach()));
    }
    let roots: Vec<gix::ObjectId> = refs.iter().map(|(_, id)| *id).collect();
    copy_reachable(&src, &dst, &roots)?;
    for (name, id) in refs {
        dst.reference(
            name.as_str(),
            id,
            gix::refs::transaction::PreviousValue::Any,
            "fixture copy",
        )
        .map_err(other)?;
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

fn open(git_dir: &Path) -> io::Result<gix::Repository> {
    gix::open_opts(git_dir, gix::open::Options::isolated()).map_err(other)
}

fn tip(repo: &gix::Repository, branch: &str) -> io::Result<Option<gix::ObjectId>> {
    Ok(repo
        .rev_parse_single(format!("refs/heads/{branch}").as_str())
        .ok()
        .map(|id| id.detach()))
}

fn contains_symlink(
    repo: &gix::Repository,
    mut tree_id: gix::ObjectId,
    path: &str,
) -> io::Result<bool> {
    let parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    for (index, part) in parts.iter().enumerate() {
        let tree = repo.find_tree(tree_id).map_err(other)?;
        let Some(entry) = tree.find_entry(*part) else {
            return Ok(false);
        };
        if entry.mode().is_link() {
            return Ok(true);
        }
        if index + 1 == parts.len() {
            return Ok(false);
        }
        if !entry.mode().is_tree() {
            return Err(io::Error::other(format!(
                "fixture path passes through a non-directory: {part}"
            )));
        }
        tree_id = entry.id().detach();
    }
    Ok(false)
}

fn signature(identity: &Identity<'_>) -> io::Result<gix::actor::Signature> {
    let time = gix::date::parse(identity.date, None)
        .or_else(|_| {
            chrono::DateTime::parse_from_rfc3339(identity.date).map(|parsed| {
                gix::date::Time::new(parsed.timestamp(), parsed.offset().local_minus_utc())
            })
        })
        .map_err(|error| io::Error::other(error.to_string()))?;
    Ok(gix::actor::Signature {
        name: identity.name.into(),
        email: identity.email.into(),
        time,
    })
}

/// Git stores a trailing newline on the message.
fn complete_line(message: &str) -> String {
    if message.ends_with('\n') {
        message.to_owned()
    } else {
        format!("{message}\n")
    }
}

fn copy_reachable(
    src: &gix::Repository,
    dst: &gix::Repository,
    roots: &[gix::ObjectId],
) -> io::Result<()> {
    let mut pending = roots.to_vec();
    let mut seen = HashSet::new();
    while let Some(id) = pending.pop() {
        if !seen.insert(id) {
            continue;
        }
        let object = src.find_object(id).map_err(other)?.detach();
        match object.kind {
            gix::objs::Kind::Commit => {
                let commit = gix::objs::CommitRef::from_bytes(&object.data, src.object_hash())
                    .map_err(other)?;
                pending.push(commit.tree());
                pending.extend(commit.parents());
            }
            gix::objs::Kind::Tree => {
                for entry in gix::objs::TreeRefIter::from_bytes(&object.data, src.object_hash()) {
                    pending.push(entry.map_err(other)?.oid.to_owned());
                }
            }
            gix::objs::Kind::Tag => {
                pending.push(
                    gix::objs::TagRef::from_bytes(&object.data, src.object_hash())
                        .map_err(other)?
                        .target(),
                );
            }
            gix::objs::Kind::Blob => {}
        }
        if !dst.has_object(id) {
            dst.objects
                .write_buf(object.kind, &object.data)
                .map_err(other)?;
        }
    }
    Ok(())
}

fn other(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}

#[cfg(test)]
mod tests {
    use gix::bstr::ByteSlice;

    use super::*;

    fn bare(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("ninot-fixture-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        ninot_common::git::init_bare(&dir, "main").unwrap();
        dir
    }

    #[test]
    fn commits_files_branches_and_copies_refs_without_git() {
        let dir = bare("commit");
        let sha = commit_file(
            &dir,
            "main",
            "main",
            Path::new("dir/a.txt"),
            b"hello\n",
            "hello",
        )
        .unwrap();
        let feature = commit_file(
            &dir,
            "feature",
            "main",
            Path::new("b.txt"),
            b"b\n",
            "feature",
        )
        .unwrap();
        let repo = open(&dir).unwrap();
        let main = repo
            .find_commit(gix::ObjectId::from_hex(sha.as_bytes()).unwrap())
            .unwrap();
        assert_eq!(main.message_raw_sloppy().as_bytes(), b"hello\n");
        assert_eq!(main.author().unwrap().name, "ninot-github");
        assert_eq!(main.author().unwrap().seconds(), 946_684_800);
        let blob = repo
            .find_blob(
                main.tree()
                    .unwrap()
                    .lookup_entry_by_path("dir/a.txt")
                    .unwrap()
                    .unwrap()
                    .id(),
            )
            .unwrap();
        assert_eq!(blob.data.as_slice(), b"hello\n");
        let feature_commit = repo
            .find_commit(gix::ObjectId::from_hex(feature.as_bytes()).unwrap())
            .unwrap();
        assert_eq!(feature_commit.parent_ids().next().unwrap().to_string(), sha);

        let again = commit(Commit {
            git_dir: &dir,
            branch: "main",
            default_branch: "main",
            path: Path::new("dir/a.txt"),
            bytes: b"hello\n",
            message: "empty change",
            author: &Identity {
                name: "A",
                email: "a@b",
                date: "2026-09-29T20:38:27Z",
            },
            committer: &Identity {
                name: "C",
                email: "c@b",
                date: "2026-09-29T22:38:27+02:00",
            },
        })
        .unwrap();
        let again_commit = repo
            .find_commit(gix::ObjectId::from_hex(again.as_bytes()).unwrap())
            .unwrap();
        assert_eq!(
            again_commit.author().unwrap().time().unwrap().seconds,
            1_790_714_307
        );
        assert_eq!(
            again_commit.committer().unwrap().time().unwrap().offset,
            7_200
        );

        let fork = bare("fork");
        copy_refs(&fork, &dir).unwrap();
        let copied = open(&fork).unwrap();
        assert!(copied.rev_parse_single("refs/heads/feature").is_ok());
        assert_eq!(
            copied
                .rev_parse_single("refs/heads/main")
                .unwrap()
                .detach()
                .to_string(),
            again
        );
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&fork);
    }

    #[test]
    fn rejects_paths_that_escape_or_follow_a_symlink() {
        let dir = bare("paths");
        for path in ["", "..", ".git/config", "a/../../b", "foo/.GIT/x"] {
            let error = commit_file(&dir, "main", "main", Path::new(path), b"x", "no").unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput, "{path}: {error}");
        }
        let repo = open(&dir).unwrap();
        let blob = repo.write_blob(b"target").unwrap().detach();
        let mut editor = repo.empty_tree().edit().unwrap();
        editor.upsert("link", EntryKind::Link, blob).unwrap();
        let tree = editor.write().unwrap().detach();
        let author = signature(&FIXTURE_IDENTITY).unwrap();
        let mut author_time = gix::date::parse::TimeBuf::default();
        let mut committer_time = gix::date::parse::TimeBuf::default();
        repo.commit_as(
            author.to_ref(&mut committer_time),
            author.to_ref(&mut author_time),
            "refs/heads/main",
            "link\n",
            tree,
            Vec::<gix::ObjectId>::new(),
        )
        .unwrap();
        let error =
            commit_file(&dir, "main", "main", Path::new("link/file"), b"x", "no").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("symlink"), "{error}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
