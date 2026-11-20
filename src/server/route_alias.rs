//! Paths GitHub still serves that the pinned description does not route.

use crate::operation::Method;

/// `GET /repos/{owner}/{repo}/contents` and the trailing-slash form list the repository root.
/// The pinned route requires a non-empty `{path}`, so this rewrites the root to `.`, which the
/// contents handler reads as the root tree. Git rejects a tree entry named `.`.
pub(super) fn contents_root(method: Method, path: &str) -> Option<String> {
    if method != Method::Get {
        return None;
    }
    let rest = path.strip_prefix("/repos/")?;
    let mut parts = rest.split('/');
    let owner = parts.next()?;
    let repo = parts.next()?;
    if parts.next()? != "contents" {
        return None;
    }
    let extra = parts.next();
    if extra.is_some_and(|part| !part.is_empty()) || parts.next().is_some() {
        return None;
    }
    Some(format!("/repos/{owner}/{repo}/contents/."))
}
