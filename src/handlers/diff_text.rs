//! Unified `git diff` text (`--no-renames`, `core.quotepath=false`) from git trees.
//!
//! Binary files are reported as `Binary files … differ`. gix 0.87 has no base85
//! `GIT binary patch` encoder, so format-patch uses this text form too.

use std::cmp::Ordering;
use std::io;
use std::path::Path;

use gix::diff::blob::unified_diff::{ConsumeHunk, ContextSize, DiffLineKind, HunkHeader};
use gix::diff::blob::{
    Algorithm, InternedInput, UnifiedDiff, diff_with_slider_heuristics, sources::byte_lines,
};
use gix::object::tree::diff::ChangeDetached;
use gix::objs::tree::EntryMode;
use gix::prelude::ObjectIdExt;

use crate::api::ApiResponse;

use super::git_store::open_repo;

/// One changed path, its `git diff` text, and the counts a diffstat needs.
pub(crate) struct FileDiff {
    /// `diff --git` block, ending in a newline.
    pub text: String,
    /// Path shown in the diffstat.
    pub path: String,
    pub additions: u32,
    pub deletions: u32,
    pub binary: bool,
    pub old_size: u64,
    pub new_size: u64,
    pub created: Option<u16>,
    pub deleted: Option<u16>,
    /// `(old mode, new mode)` when both sides exist and the mode differs.
    pub mode_change: Option<(u16, u16)>,
}

/// `git diff-tree --root -p --no-renames` of `sha`.
///
/// A merge (more than one parent) is empty, matching `git diff-tree` without `-m`/`-c`.
pub(crate) fn commit_diff(git_dir: &Path, sha: &str) -> Result<Vec<u8>, ApiResponse> {
    let repo = open_repo(git_dir)?;
    let files = commit_files(&repo, parse_id(&repo, sha)?)?;
    Ok(render(&files).into_bytes())
}

/// `git diff base...head`: the merge-base tree against `head`.
pub(crate) fn range_diff(git_dir: &Path, base: &str, head: &str) -> Result<Vec<u8>, ApiResponse> {
    let repo = open_repo(git_dir)?;
    let files = range_files(&repo, base, head)?;
    Ok(render(&files).into_bytes())
}

pub(crate) fn commit_files(
    repo: &gix::Repository,
    id: gix::ObjectId,
) -> Result<Vec<FileDiff>, ApiResponse> {
    let commit = repo.find_commit(id).map_err(ApiResponse::internal)?;
    let parents: Vec<gix::ObjectId> = commit.parent_ids().map(|parent| parent.detach()).collect();
    if parents.len() > 1 {
        return Ok(Vec::new());
    }
    let new_tree = commit.tree().map_err(ApiResponse::internal)?;
    let old_tree = match parents.first() {
        Some(parent) => repo
            .find_commit(*parent)
            .map_err(ApiResponse::internal)?
            .tree()
            .map_err(ApiResponse::internal)?,
        None => repo.empty_tree(),
    };
    files_between(repo, &old_tree, &new_tree)
}

pub(crate) fn range_files(
    repo: &gix::Repository,
    base: &str,
    head: &str,
) -> Result<Vec<FileDiff>, ApiResponse> {
    let base_id = parse_id(repo, base)?;
    let head_id = parse_id(repo, head)?;
    let merge_base = repo
        .merge_base(base_id, head_id)
        .map_err(|_| ApiResponse::internal("no merge base"))?
        .detach();
    let old_tree = repo
        .find_commit(merge_base)
        .map_err(ApiResponse::internal)?
        .tree()
        .map_err(ApiResponse::internal)?;
    let new_tree = repo
        .find_commit(head_id)
        .map_err(ApiResponse::internal)?
        .tree()
        .map_err(ApiResponse::internal)?;
    files_between(repo, &old_tree, &new_tree)
}

/// Non-merge commits reachable from `tip` but not from `hidden`, newest first.
pub(crate) fn non_merges(
    repo: &gix::Repository,
    tip: gix::ObjectId,
    hidden: Option<gix::ObjectId>,
) -> Result<Vec<gix::ObjectId>, ApiResponse> {
    let hidden: Vec<gix::ObjectId> = hidden.into_iter().collect();
    let walk = repo
        .rev_walk([tip])
        .sorting(gix::revision::walk::Sorting::ByCommitTime(
            Default::default(),
        ))
        .with_hidden(hidden)
        .all()
        .map_err(ApiResponse::internal)?;
    let mut ids = Vec::new();
    for info in walk {
        let info = info.map_err(ApiResponse::internal)?;
        if info.parent_ids.len() > 1 {
            continue;
        }
        ids.push(info.id);
    }
    Ok(ids)
}

pub(crate) fn parse_id(repo: &gix::Repository, spec: &str) -> Result<gix::ObjectId, ApiResponse> {
    Ok(repo
        .rev_parse_single(spec)
        .map_err(ApiResponse::internal)?
        .detach())
}

pub(crate) fn render(files: &[FileDiff]) -> String {
    let mut out = String::new();
    for file in files {
        out.push_str(&file.text);
    }
    out
}

fn files_between(
    repo: &gix::Repository,
    old_tree: &gix::Tree<'_>,
    new_tree: &gix::Tree<'_>,
) -> Result<Vec<FileDiff>, ApiResponse> {
    let mut options = gix::diff::Options::default().with_rewrites(None);
    options.track_path();
    let changes = repo
        .diff_tree_to_tree(old_tree, new_tree, options)
        .map_err(ApiResponse::internal)?;
    let mut files = Vec::new();
    for change in changes {
        if change.entry_mode().is_tree() {
            continue;
        }
        files.push(render_change(repo, &change)?);
    }
    // Tree walk order: a slash sorts before any other byte, so `dir/a` precedes `empty`.
    files.sort_by(|left, right| git_path_order(&left.path, &right.path));
    Ok(files)
}

fn git_path_order(left: &str, right: &str) -> Ordering {
    let rank = |byte: u8| if byte == b'/' { 0 } else { byte };
    left.bytes().map(rank).cmp(right.bytes().map(rank))
}

struct Sides {
    path: String,
    old_id: Option<gix::ObjectId>,
    new_id: Option<gix::ObjectId>,
    old_mode: Option<EntryMode>,
    new_mode: Option<EntryMode>,
}

fn sides(change: &ChangeDetached) -> Sides {
    match change {
        ChangeDetached::Addition {
            id,
            entry_mode,
            location,
            ..
        } => Sides {
            path: lossy(location),
            old_id: None,
            new_id: Some(*id),
            old_mode: None,
            new_mode: Some(*entry_mode),
        },
        ChangeDetached::Deletion {
            id,
            entry_mode,
            location,
            ..
        } => Sides {
            path: lossy(location),
            old_id: Some(*id),
            new_id: None,
            old_mode: Some(*entry_mode),
            new_mode: None,
        },
        ChangeDetached::Modification {
            previous_id,
            previous_entry_mode,
            id,
            entry_mode,
            location,
        } => Sides {
            path: lossy(location),
            old_id: Some(*previous_id),
            new_id: Some(*id),
            old_mode: Some(*previous_entry_mode),
            new_mode: Some(*entry_mode),
        },
        ChangeDetached::Rewrite {
            source_id,
            source_entry_mode,
            id,
            entry_mode,
            location,
            ..
        } => Sides {
            path: lossy(location),
            old_id: Some(*source_id),
            new_id: Some(*id),
            old_mode: Some(*source_entry_mode),
            new_mode: Some(*entry_mode),
        },
    }
}

fn lossy(bytes: impl AsRef<[u8]>) -> String {
    String::from_utf8_lossy(bytes.as_ref()).into_owned()
}

fn render_change(repo: &gix::Repository, change: &ChangeDetached) -> Result<FileDiff, ApiResponse> {
    let sides = sides(change);
    let old_raw = blob_bytes(repo, sides.old_id, sides.old_mode)?;
    let new_raw = blob_bytes(repo, sides.new_id, sides.new_mode)?;
    let binary = is_binary(&old_raw) || is_binary(&new_raw);
    let (hunks, additions, deletions) = if binary || sides.old_id == sides.new_id {
        (String::new(), 0, 0)
    } else {
        unified(&old_raw, &new_raw)?
    };
    let text = diff_header(repo, &sides, binary, &hunks);
    let mode_change = match (sides.old_mode, sides.new_mode) {
        (Some(old), Some(new)) if old != new => Some((old.value(), new.value())),
        _ => None,
    };
    Ok(FileDiff {
        text,
        path: sides.path,
        additions,
        deletions,
        binary,
        old_size: old_raw.len() as u64,
        new_size: new_raw.len() as u64,
        created: if sides.old_id.is_none() {
            sides.new_mode.map(|mode| mode.value())
        } else {
            None
        },
        deleted: if sides.new_id.is_none() {
            sides.old_mode.map(|mode| mode.value())
        } else {
            None
        },
        mode_change,
    })
}

fn blob_bytes(
    repo: &gix::Repository,
    id: Option<gix::ObjectId>,
    mode: Option<EntryMode>,
) -> Result<Vec<u8>, ApiResponse> {
    let Some(id) = id else {
        return Ok(Vec::new());
    };
    if mode.is_some_and(|mode| mode.is_commit()) {
        return Ok(format!("Subproject commit {id}\n").into_bytes());
    }
    Ok(repo
        .find_object(id)
        .map_err(ApiResponse::internal)?
        .detach()
        .data)
}

fn is_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8000).any(|byte| *byte == 0)
}

fn diff_header(repo: &gix::Repository, sides: &Sides, binary: bool, hunks: &str) -> String {
    let (old_ab, new_ab) = abbrev(repo, sides.old_id, sides.new_id);
    let mut text = format!(
        "diff --git {} {}\n",
        quoted("a/", &sides.path),
        quoted("b/", &sides.path)
    );
    let content_changed = sides.old_id != sides.new_id;
    match (sides.old_mode, sides.new_mode) {
        (None, Some(mode)) => {
            text.push_str(&format!("new file mode {:06o}\n", mode.value()));
            text.push_str(&format!("index {old_ab}..{new_ab}\n"));
        }
        (Some(mode), None) => {
            text.push_str(&format!("deleted file mode {:06o}\n", mode.value()));
            text.push_str(&format!("index {old_ab}..{new_ab}\n"));
        }
        (Some(old), Some(new)) if old != new && !content_changed => {
            text.push_str(&format!("old mode {:06o}\n", old.value()));
            text.push_str(&format!("new mode {:06o}\n", new.value()));
        }
        (Some(old), Some(new)) if old != new => {
            text.push_str(&format!("old mode {:06o}\n", old.value()));
            text.push_str(&format!("new mode {:06o}\n", new.value()));
            text.push_str(&format!("index {old_ab}..{new_ab}\n"));
        }
        (_, Some(mode)) => {
            text.push_str(&format!("index {old_ab}..{new_ab} {:06o}\n", mode.value()));
        }
        _ => {}
    }
    if binary {
        let left = if sides.old_id.is_none() {
            "/dev/null".to_owned()
        } else {
            quoted("a/", &sides.path)
        };
        let right = if sides.new_id.is_none() {
            "/dev/null".to_owned()
        } else {
            quoted("b/", &sides.path)
        };
        text.push_str(&format!("Binary files {left} and {right} differ\n"));
    } else if !hunks.is_empty() {
        let old_label = if sides.old_id.is_none() {
            "/dev/null".to_owned()
        } else {
            quoted("a/", &sides.path)
        };
        let new_label = if sides.new_id.is_none() {
            "/dev/null".to_owned()
        } else {
            quoted("b/", &sides.path)
        };
        text.push_str(&format!("--- {old_label}\n+++ {new_label}\n{hunks}"));
    }
    text
}

fn abbrev(
    repo: &gix::Repository,
    old: Option<gix::ObjectId>,
    new: Option<gix::ObjectId>,
) -> (String, String) {
    let old_prefix = old.map(|id| id.attach(repo).shorten_or_id());
    let new_prefix = new.map(|id| id.attach(repo).shorten_or_id());
    let len = old_prefix
        .map(|prefix| prefix.hex_len())
        .unwrap_or(0)
        .max(new_prefix.map(|prefix| prefix.hex_len()).unwrap_or(0))
        .max(7);
    let show = |id: Option<gix::ObjectId>| match id {
        None => "0".repeat(len),
        Some(id) => id.to_hex_with_len(len).to_string(),
    };
    (show(old), show(new))
}

/// Quote `prefix` + `path` the way `core.quotepath=false` does: only controls and escapes.
fn quoted(prefix: &str, path: &str) -> String {
    let full = format!("{prefix}{path}");
    if full
        .bytes()
        .any(|byte| byte < 0x20 || byte == 0x7f || byte == b'"' || byte == b'\\')
    {
        let mut out = String::from("\"");
        for byte in full.bytes() {
            match byte {
                b'\\' | b'"' => {
                    out.push('\\');
                    out.push(byte as char);
                }
                b'\n' => out.push_str("\\n"),
                b'\t' => out.push_str("\\t"),
                0x00..=0x1f | 0x7f => out.push_str(&format!("\\{byte:03o}")),
                _ => out.push(byte as char),
            }
        }
        out.push('"');
        out
    } else {
        full
    }
}

struct Hunks {
    out: String,
    additions: u32,
    deletions: u32,
}

impl ConsumeHunk for Hunks {
    type Out = (String, u32, u32);

    fn consume_hunk(
        &mut self,
        header: HunkHeader,
        lines: &[(DiffLineKind, &[u8])],
    ) -> io::Result<()> {
        let before = hunk_side(header.before_hunk_start, header.before_hunk_len);
        let after = hunk_side(header.after_hunk_start, header.after_hunk_len);
        self.out.push_str(&format!("@@ -{before} +{after} @@\n"));
        for (kind, content) in lines {
            let prefix = match kind {
                DiffLineKind::Context => ' ',
                DiffLineKind::Add => {
                    self.additions += 1;
                    '+'
                }
                DiffLineKind::Remove => {
                    self.deletions += 1;
                    '-'
                }
            };
            self.out.push(prefix);
            self.out.push_str(&String::from_utf8_lossy(content));
            if !content.ends_with(b"\n") {
                self.out.push_str("\n\\ No newline at end of file\n");
            }
        }
        Ok(())
    }

    fn finish(self) -> Self::Out {
        (self.out, self.additions, self.deletions)
    }
}

/// Git omits `,len` when the length is 1, and uses start 0 when the length is 0.
fn hunk_side(start: u32, len: u32) -> String {
    if len == 0 {
        "0,0".to_owned()
    } else if len == 1 {
        start.to_string()
    } else {
        format!("{start},{len}")
    }
}

fn unified(before: &[u8], after: &[u8]) -> Result<(String, u32, u32), ApiResponse> {
    let input = InternedInput::new(byte_lines(before), byte_lines(after));
    let diff = diff_with_slider_heuristics(Algorithm::Myers, &input);
    UnifiedDiff::new(
        &diff,
        &input,
        Hunks {
            out: String::new(),
            additions: 0,
            deletions: 0,
        },
        ContextSize::default(),
    )
    .consume()
    .map_err(ApiResponse::internal)
}
