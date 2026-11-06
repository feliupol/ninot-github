//! Releases and their assets.

/// A release and its uploaded assets.
pub struct Release {
    pub id: u64,
    pub tag_name: String,
    pub target_commitish: String,
    pub name: Option<String>,
    pub body: Option<String>,
    pub draft: bool,
    pub prerelease: bool,
    pub author: String,
    pub created_at: String,
    pub published_at: Option<String>,
    pub assets: Vec<Asset>,
}

/// A file attached to a release.
pub struct Asset {
    pub id: u64,
    pub name: String,
    pub label: Option<String>,
    pub content_type: String,
    pub data: Vec<u8>,
    /// `sha256:` followed by the hex digest of `data`.
    pub digest: String,
    pub uploader: String,
    pub created_at: String,
}
