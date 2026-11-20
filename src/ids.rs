//! Identifiers shared by the REST and GraphQL representations of one object.

/// Database id packed into a pull request node id: repository id in the high bits, number in the low 32.
pub(crate) fn pull_database_id(repository_id: u64, number: u64) -> u64 {
    (repository_id << 32) | (number & u32::MAX as u64)
}

/// REST `node_id` and GraphQL `id` of a pull request. `node(id:)` resolves the same string.
pub(crate) fn pull_request_node_id(repository_id: u64, number: u64) -> String {
    crate::render::node_id("PullRequest", pull_database_id(repository_id, number))
}
