//! Identifiers shared by the REST and GraphQL representations of one object.

/// Database id packed into a pull request node id: repository id in the high bits, number in the low 32.
pub(crate) fn pull_database_id(repository_id: u64, number: u64) -> u64 {
    (repository_id << 32) | (number & u32::MAX as u64)
}

/// REST `node_id` and GraphQL `id` of a pull request. `node(id:)` resolves the same string.
pub(crate) fn pull_request_node_id(repository_id: u64, number: u64) -> String {
    crate::render::node_id("PullRequest", pull_database_id(repository_id, number))
}

/// A GitHub global node id in the legacy format: base64 of `0<kind length>:<Kind><id>`,
/// e.g. `MDQ6VXNlcjE=` for `04:User1`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NodeId {
    pub(crate) kind: String,
    pub(crate) id: u64,
}

impl NodeId {
    pub(crate) fn new(kind: &str, id: u64) -> Self {
        Self {
            kind: kind.to_owned(),
            id,
        }
    }

    pub(crate) fn encode(&self) -> String {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(format!(
            "0{}:{}{}",
            self.kind.len(),
            self.kind,
            self.id
        ))
    }

    /// The node id `encoded` names, or `None` when it is not one this fake issues.
    pub(crate) fn decode(encoded: &str) -> Option<Self> {
        use base64::Engine as _;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .ok()?;
        let text = String::from_utf8(bytes).ok()?;
        let (length, rest) = text.split_once(':')?;
        let length: usize = length.parse().ok()?;
        let kind = rest.get(..length)?;
        let id = rest.get(length..)?.parse().ok()?;
        Some(Self::new(kind, id))
    }
}

/// A connection cursor: the offset of an item in its list, as opaque base64 JSON.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Cursor {
    pub(crate) offset: usize,
}

impl Cursor {
    pub(crate) fn encode(self) -> String {
        use base64::Engine as _;
        let json = serde_json::to_vec(&self).expect("a cursor serializes");
        base64::engine::general_purpose::STANDARD.encode(json)
    }

    /// The cursor `encoded` names, or `None` when it is not one this fake issues.
    pub(crate) fn decode(encoded: &str) -> Option<Self> {
        use base64::Engine as _;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .ok()?;
        serde_json::from_slice(&bytes).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_ids_round_trip_and_read_github_legacy_ids() {
        let id = NodeId::new("PullRequest", 1 << 40);
        assert_eq!(NodeId::decode(&id.encode()), Some(id));
        assert_eq!(NodeId::decode("MDQ6VXNlcjE="), Some(NodeId::new("User", 1)));
        assert_eq!(NodeId::decode("garbage"), None);
        assert_eq!(
            NodeId::decode(&NodeId::new("ProjectV2", 4).encode()).map(|n| n.id),
            Some(4)
        );
    }

    #[test]
    fn cursors_round_trip_and_reject_foreign_strings() {
        let cursor = Cursor { offset: 7 };
        assert_eq!(Cursor::decode(&cursor.encode()), Some(cursor));
        assert_eq!(Cursor::decode("not base64!"), None);
        assert_eq!(Cursor::decode("Y3Vyc29yOjc="), None);
    }
}
