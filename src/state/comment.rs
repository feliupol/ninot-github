//! Comments on an issue or pull request conversation.

use chrono::{DateTime, Utc};

use super::Name;

/// A conversation comment, as `GET .../issues/{number}/comments` lists it.
///
/// The URLs, node id, author association and the parent's comment count are derived when the
/// comment is rendered.
#[derive(Clone, Debug)]
pub struct Comment {
    /// Comment id, unique across the server.
    pub id: u64,
    /// Login of the author. Absent for a fixture comment seeded without a user.
    pub author: Option<Name>,
    /// Markdown text.
    pub body: String,
    /// When the comment was created.
    pub created_at: DateTime<Utc>,
    /// When the comment was last edited.
    pub updated_at: DateTime<Utc>,
}
