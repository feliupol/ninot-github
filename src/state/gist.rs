//! Gists created through the Gists API.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};

/// A gist. URLs, sizes and languages are derived when it is rendered.
#[derive(Clone, Debug)]
pub struct Gist {
    /// Sequence number; the API id is [`Self::id`].
    pub number: u64,
    /// Description, when one was given.
    pub description: Option<String>,
    /// Whether the gist is public.
    pub public: bool,
    /// File contents by filename.
    pub files: BTreeMap<String, String>,
    /// When the gist was created.
    pub created_at: DateTime<Utc>,
    /// When the gist was last updated.
    pub updated_at: DateTime<Utc>,
}

impl Gist {
    /// The string id the API serves, such as `ninotgist000001`.
    pub fn id(&self) -> String {
        format!("ninotgist{:06}", self.number)
    }
}
