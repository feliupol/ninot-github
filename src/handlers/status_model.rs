//! Commit statuses rendered from their stored records.

use crate::render;
use crate::state::CommitStatusRecord;
use crate::time;
use crate::types;
use crate::urls::RepoUrls;

/// The REST model of a status set on `sha`.
pub(crate) fn status(urls: &RepoUrls, sha: &str, status: &CommitStatusRecord) -> types::Status {
    types::Status {
        avatar_url: None,
        context: status.context.clone(),
        created_at: time::format(&status.created_at),
        creator: status.creator.clone().map(Into::into),
        description: status.description.clone(),
        id: status.id,
        node_id: node_id(status.id),
        state: status.state.name().to_owned(),
        target_url: status.target_url.clone(),
        updated_at: time::format(&status.updated_at),
        url: urls.status(sha, status.id),
    }
}

/// The smaller model the combined status lists.
pub(crate) fn simple_status(
    urls: &RepoUrls,
    sha: &str,
    status: &CommitStatusRecord,
) -> types::SimpleCommitStatus {
    types::SimpleCommitStatus {
        avatar_url: None,
        context: status.context.clone(),
        created_at: status.created_at,
        description: status.description.clone(),
        id: status.id,
        node_id: node_id(status.id),
        required: None,
        state: status.state.name().to_owned(),
        target_url: status.target_url.clone(),
        updated_at: status.updated_at,
        url: urls.status(sha, status.id),
    }
}

fn node_id(id: i64) -> String {
    render::node_id("Status", u64::try_from(id).unwrap_or(0))
}
