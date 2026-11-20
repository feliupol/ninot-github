//! Workflow YAML read from the default branch.

use std::path::Path;

/// A workflow file read from the default branch.
pub(crate) struct WorkflowFile {
    pub(crate) path: String,
    pub(crate) name: String,
    pub(crate) content: String,
}

/// `.github/workflows/*.{yml,yaml}` on `branch`. `None` when the branch cannot be read.
pub(crate) fn workflow_files(git_dir: Option<&Path>, branch: &str) -> Option<Vec<WorkflowFile>> {
    let git_dir = git_dir?;
    let repo = super::git_store::open_repo(git_dir).ok()?;
    let sha = ninot_common::git::resolve_commit(git_dir, branch)
        .ok()
        .flatten()?;
    let commit_id = gix::ObjectId::from_hex(sha.as_bytes()).ok()?;
    let tree = repo.find_commit(commit_id).ok()?.tree().ok()?;
    let Some(github) = child_tree(&repo, &tree, ".github") else {
        return Some(Vec::new());
    };
    let Some(workflows) = child_tree(&repo, &github, "workflows") else {
        return Some(Vec::new());
    };
    let mut files = Vec::new();
    for entry in workflows.iter() {
        let Ok(entry) = entry else { continue };
        if entry.mode().is_tree() {
            continue;
        }
        let filename = entry.filename().to_string();
        if !filename.ends_with(".yml") && !filename.ends_with(".yaml") {
            continue;
        }
        let Ok(blob) = repo.find_blob(entry.object_id()) else {
            continue;
        };
        let content = String::from_utf8_lossy(&blob.data).into_owned();
        let path = format!(".github/workflows/{filename}");
        let name = workflow_name(&content).unwrap_or_else(|| path.clone());
        files.push(WorkflowFile {
            path,
            name,
            content,
        });
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Some(files)
}

fn child_tree<'repo>(
    repo: &'repo gix::Repository,
    tree: &gix::Tree<'_>,
    name: &str,
) -> Option<gix::Tree<'repo>> {
    let id = tree.iter().find_map(|entry| {
        let entry = entry.ok()?;
        (entry.filename() == name && entry.mode().is_tree()).then(|| entry.object_id())
    })?;
    repo.find_tree(id).ok()
}

/// The top-level `name:` of a workflow file.
fn workflow_name(content: &str) -> Option<String> {
    for line in content.lines() {
        if line.is_empty() || line.starts_with(['#', ' ', '\t']) {
            continue;
        }
        let Some(rest) = line.strip_prefix("name:") else {
            continue;
        };
        let name = rest.trim().trim_matches(['"', '\'']);
        if !name.is_empty() {
            return Some(name.to_owned());
        }
    }
    None
}
