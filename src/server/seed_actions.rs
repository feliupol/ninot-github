//! Seeding for Actions artifacts and caches.

use std::io::{self, Cursor, Write};

use super::*;

impl Repository {
    /// Attaches an Actions artifact named `name` to workflow run `run_id`.
    ///
    /// `bytes` are stored as a zip entry of that name so `gh run download` extracts them.
    pub fn create_artifact(&self, run_id: u64, name: &str, bytes: &[u8]) -> io::Result<u64> {
        self.ensure_running()?;
        let zip = zip_entry(name, bytes)?;
        let mut state = lock(&self.app.state);
        require_repository(&state, &self.owner, &self.name)?;
        let id = state.next_id();
        let now = crate::time::now_utc();
        let repo = state
            .account_mut(&self.owner)
            .and_then(|account| account.repo_mut(&self.name))
            .ok_or_else(|| not_found("repository does not exist"))?;
        if !repo.workflow_runs.iter().any(|run| run.id == run_id) {
            return Err(not_found(format!("workflow run {run_id} does not exist")));
        }
        repo.artifacts.push(crate::state::ArtifactRecord {
            id,
            name: name.to_owned(),
            run_id,
            size_in_bytes: i64::try_from(bytes.len()).unwrap_or(i64::MAX),
            expired: false,
            created_at: now,
            expires_at: Some(now + chrono::Duration::days(90)),
            updated_at: now,
            zip,
        });
        Ok(id)
    }

    /// Records an Actions cache named `key` on `git_ref` (for example `refs/heads/main`).
    pub fn create_actions_cache(
        &self,
        key: &str,
        git_ref: &str,
        size_in_bytes: i64,
    ) -> io::Result<u64> {
        self.ensure_running()?;
        let mut state = lock(&self.app.state);
        require_repository(&state, &self.owner, &self.name)?;
        let id = state.next_id();
        let now = crate::time::now_utc();
        let repo = state
            .account_mut(&self.owner)
            .and_then(|account| account.repo_mut(&self.name))
            .ok_or_else(|| not_found("repository does not exist"))?;
        repo.caches.push(crate::state::CacheRecord {
            id,
            key: key.to_owned(),
            git_ref: git_ref.to_owned(),
            version: "v1".to_owned(),
            size_in_bytes,
            created_at: now,
            last_accessed_at: now,
        });
        Ok(id)
    }
}

fn zip_entry(name: &str, bytes: &[u8]) -> io::Result<Vec<u8>> {
    let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    archive
        .start_file(name, options)
        .map_err(|error| io::Error::other(error.to_string()))?;
    archive.write_all(bytes)?;
    let cursor = archive
        .finish()
        .map_err(|error| io::Error::other(error.to_string()))?;
    Ok(cursor.into_inner())
}
