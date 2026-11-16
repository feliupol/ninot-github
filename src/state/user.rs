//! What a user keeps on their own profile: public details, SSH keys and followers.

use chrono::{DateTime, Utc};

/// Profile fields a user changes with `PATCH /user`.
/// Source: <https://docs.github.com/en/rest/users/users#update-the-authenticated-user>
#[derive(Clone, Debug, Default)]
pub struct UserProfile {
    pub bio: Option<String>,
    pub blog: Option<String>,
    pub company: Option<String>,
    /// Publicly visible email; one of the user's verified addresses.
    pub email: Option<String>,
    pub hireable: Option<bool>,
    pub location: Option<String>,
    pub twitter_username: Option<String>,
}

/// A public SSH key a user added to their account.
#[derive(Clone, Debug)]
pub struct SshKey {
    /// Key id, unique across the server.
    pub id: u64,
    pub title: String,
    /// The key as `<type> <base64>`, without a trailing comment.
    pub key: String,
    pub created_at: DateTime<Utc>,
}
