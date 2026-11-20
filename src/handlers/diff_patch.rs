//! `git format-patch --stdout` mbox text for the patch media type.
//!
//! Patches are the non-merge commits between the merge base and the head, oldest
//! first. A single patch uses `Subject: [PATCH]`. The signature line is `-- `
//! without a git version. Binary blobs use the text diff (`Binary files … differ`)
//! and a `Bin old -> new bytes` diffstat; gix 0.87 cannot emit a `GIT binary patch`.

use std::path::Path;

use gix::bstr::ByteSlice;

use crate::api::ApiResponse;

use super::diff_text::{self, FileDiff};
use super::git_store::open_repo;

/// `git format-patch -1 --stdout SHA`: the newest non-merge reachable from `sha`.
pub(crate) fn commit_patch(git_dir: &Path, sha: &str) -> Result<Vec<u8>, ApiResponse> {
    let repo = open_repo(git_dir)?;
    let tip = diff_text::parse_id(&repo, sha)?;
    let mut commits = diff_text::non_merges(&repo, tip, None)?;
    commits.truncate(1);
    Ok(mbox(&repo, &commits)?.into_bytes())
}

/// `git format-patch --stdout MERGEBASE..HEAD`.
pub(crate) fn range_patch(git_dir: &Path, base: &str, head: &str) -> Result<Vec<u8>, ApiResponse> {
    let repo = open_repo(git_dir)?;
    let base_id = diff_text::parse_id(&repo, base)?;
    let head_id = diff_text::parse_id(&repo, head)?;
    let merge_base = repo
        .merge_base(base_id, head_id)
        .map_err(|_| ApiResponse::internal("no merge base"))?
        .detach();
    let mut commits = diff_text::non_merges(&repo, head_id, Some(merge_base))?;
    commits.reverse();
    Ok(mbox(&repo, &commits)?.into_bytes())
}

fn mbox(repo: &gix::Repository, commits: &[gix::ObjectId]) -> Result<String, ApiResponse> {
    let total = commits.len();
    let mut out = String::new();
    for (index, id) in commits.iter().copied().enumerate() {
        out.push_str(&one_patch(repo, id, index + 1, total)?);
        // The signature already ends with a blank line. format-patch adds
        // another one before the next message.
        if index + 1 != total {
            out.push('\n');
        }
    }
    Ok(out)
}

fn one_patch(
    repo: &gix::Repository,
    id: gix::ObjectId,
    index: usize,
    total: usize,
) -> Result<String, ApiResponse> {
    let commit = repo.find_commit(id).map_err(ApiResponse::internal)?;
    let files = diff_text::commit_files(repo, id)?;
    let author = commit.author().map_err(ApiResponse::internal)?;
    let (subject, body) = subject_body(commit.message_raw_sloppy().as_bytes());
    let prefix = if total > 1 {
        format!("[PATCH {index}/{total}]")
    } else {
        "[PATCH]".to_owned()
    };
    let subject_line = if subject.is_empty() {
        format!("Subject: {prefix}")
    } else {
        format!("Subject: {prefix} {subject}")
    };
    let date = author
        .time()
        .map_err(ApiResponse::internal)?
        .format_or_unix(gix::date::time::format::GIT_RFC2822);
    let mut out = format!(
        "From {id} Mon Sep 17 00:00:00 2001\nFrom: {} <{}>\nDate: {date}\n{subject_line}\n\n",
        author.name, author.email
    );
    if !body.is_empty() {
        out.push_str(&body);
    }
    if !files.is_empty() {
        out.push_str("---\n");
        out.push_str(&diffstat(&files));
        out.push('\n');
        out.push_str(&diff_text::render(&files));
    }
    // `-- ` and a blank line. The version line git prints here is omitted.
    out.push_str("-- \n\n");
    Ok(out)
}

/// First paragraph (newlines folded to spaces) is the subject. The body follows `\n\n`.
fn subject_body(message: &[u8]) -> (String, String) {
    let message = String::from_utf8_lossy(message);
    let message = message.strip_suffix('\n').unwrap_or(&message);
    let (title, body) = message.split_once("\n\n").unwrap_or((message, ""));
    let subject = title.replace('\n', " ");
    let body = if body.is_empty() {
        String::new()
    } else if body.ends_with('\n') {
        body.to_owned()
    } else {
        format!("{body}\n")
    };
    (subject, body)
}

/// format-patch's default stat width is 72, and the rendered line stops one column short of it.
fn diffstat(files: &[FileDiff]) -> String {
    const MAX_LINE: usize = 71;
    let name_width = files.iter().map(|file| file.path.len()).max().unwrap_or(0);
    let max_change = files
        .iter()
        .map(|file| {
            if file.binary {
                0
            } else {
                file.additions + file.deletions
            }
        })
        .max()
        .unwrap_or(0);
    let number_width = digits(max_change).max(1);
    let mut graph_width = MAX_LINE.saturating_sub(name_width + number_width + 5);
    if graph_width == 0 {
        graph_width = 1;
    }
    let mut out = String::new();
    for file in files {
        out.push(' ');
        out.push_str(&format!("{:<name_width$}", file.path));
        out.push_str(" | ");
        if file.binary {
            out.push_str(&format!(
                "Bin {} -> {} bytes\n",
                file.old_size, file.new_size
            ));
            continue;
        }
        let total = file.additions + file.deletions;
        out.push_str(&format!("{total:>number_width$}"));
        if total > 0 {
            out.push(' ');
            let (plus, minus) = if max_change as usize <= graph_width {
                (file.additions as usize, file.deletions as usize)
            } else {
                (
                    scale(file.additions, graph_width, max_change),
                    scale(file.deletions, graph_width, max_change),
                )
            };
            out.push_str(&"+".repeat(plus));
            out.push_str(&"-".repeat(minus));
        }
        out.push('\n');
    }
    out.push_str(&summary(files));
    out.push('\n');
    for file in files {
        if let Some(mode) = file.created {
            out.push_str(&format!(" create mode {mode:06o} {}\n", file.path));
        } else if let Some(mode) = file.deleted {
            out.push_str(&format!(" delete mode {mode:06o} {}\n", file.path));
        } else if let Some((old, new)) = file.mode_change {
            out.push_str(&format!(
                " mode change {old:06o} => {new:06o} {}\n",
                file.path
            ));
        }
    }
    out
}

fn summary(files: &[FileDiff]) -> String {
    let count = files.len();
    let files_word = if count == 1 { "file" } else { "files" };
    let additions: u32 = files.iter().map(|file| file.additions).sum();
    let deletions: u32 = files.iter().map(|file| file.deletions).sum();
    let mut line = format!(" {count} {files_word} changed");
    if additions == 0 && deletions == 0 {
        line.push_str(", 0 insertions(+), 0 deletions(-)");
    } else {
        if additions > 0 {
            line.push_str(&format!(
                ", {additions} insertion{}(+)",
                if additions == 1 { "" } else { "s" }
            ));
        }
        if deletions > 0 {
            line.push_str(&format!(
                ", {deletions} deletion{}(-)",
                if deletions == 1 { "" } else { "s" }
            ));
        }
    }
    line
}

fn digits(value: u32) -> usize {
    if value == 0 {
        1
    } else {
        value.ilog10() as usize + 1
    }
}

/// Git's `scale_linear`: at least one mark when `count` is non-zero.
fn scale(count: u32, width: usize, max_change: u32) -> usize {
    if count == 0 || width == 0 || max_change == 0 {
        return 0;
    }
    1 + (count as usize * (width - 1)) / max_change as usize
}
