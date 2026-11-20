//! Relay `edges` synthesized from a connection's `nodes`.

#![expect(
    clippy::disallowed_types,
    reason = "connection JSON is untyped at the schema boundary"
)]

use serde_json::Value as Json;

use super::{Map, cursor_index, cursor_of};

/// `edges` for a connection that stores `nodes` and `pageInfo.startCursor`.
///
/// An empty `nodes` array is an empty edge list. A missing `nodes` field is not a connection.
pub(super) fn from_nodes(parent: &Map) -> Option<Json> {
    let nodes = parent.get("nodes")?.as_array()?;
    let start = parent
        .get("pageInfo")
        .and_then(|info| info.get("startCursor"))
        .and_then(Json::as_str)
        .and_then(cursor_index)
        .unwrap_or(0);
    let mut edges = Vec::with_capacity(nodes.len());
    for (offset, node) in nodes.iter().enumerate() {
        let mut edge = Map::new();
        edge.insert("cursor".to_owned(), Json::String(cursor_of(start + offset)));
        edge.insert("node".to_owned(), node.clone());
        edges.push(Json::Object(edge));
    }
    Some(Json::Array(edges))
}
