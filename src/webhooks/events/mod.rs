//! Payload builders, one module per webhook event. Each builds the payload type generated from
//! its octokit/webhooks schema, and [`Ctx::enqueue`] serializes it.

pub mod ping;
pub mod push;

pub(crate) mod check_run;
pub(crate) mod check_suite;
pub(crate) mod create;
pub(crate) mod delete;
pub(crate) mod issue_changes;
pub(crate) mod issue_comment;
pub(crate) mod issues;
pub(crate) mod model;
pub(crate) mod pull_request;
pub(crate) mod pull_request_review;
pub(crate) mod pull_request_review_comment;
pub(crate) mod release;
pub(crate) mod status;
pub(crate) mod workflow_job;
pub(crate) mod workflow_run;

#[cfg(test)]
mod schema;

use std::path::PathBuf;

use serde::Serialize;

use crate::api::ApiRequest;
use crate::render;
use crate::state::{Permission, State};
use crate::types;
use crate::urls::RepoUrls;
use crate::webhook_types;
use crate::webhooks::delivery::record_unbuilt;
use crate::webhooks::enqueue;
pub(crate) use model::PayloadError;

/// Projects the REST repository into the webhook contract.
fn repository(
    repository: &types::FullRepository,
) -> Result<webhook_types::Repository, PayloadError> {
    repository.clone().try_into()
}

/// The repository, request and caller a delivery is built from.
pub(crate) struct Ctx<'a> {
    /// Mutable server state. The caller holds the lock.
    pub state: &'a mut State,
    /// The request that caused the state change, or a fixture request with no caller.
    pub request: &'a ApiRequest,
    /// Repository owner login.
    pub owner: &'a str,
    /// Repository name.
    pub name: &'a str,
}

impl<'a> Ctx<'a> {
    /// A delivery context for `owner/name`.
    pub(crate) fn new(
        state: &'a mut State,
        request: &'a ApiRequest,
        owner: &'a str,
        name: &'a str,
    ) -> Self {
        Self {
            state,
            request,
            owner,
            name,
        }
    }

    /// Whether any active hook of the repository wants `event`.
    pub(crate) fn wants(&self, event: &str) -> bool {
        self.state
            .account(self.owner)
            .and_then(|account| account.repo(self.name))
            .is_some_and(|repo| repo.hooks.iter().any(|hook| hook.wants(event)))
    }

    /// The webhook repository object, when the repository still exists.
    pub(crate) fn repository(&self) -> Option<Result<webhook_types::Repository, PayloadError>> {
        let account = self.state.account(self.owner)?;
        let repo = account.repo(self.name)?;
        let full = render::full_repository(self.request, account, repo, Permission::default());
        Some(repository(&full))
    }

    /// Locations of the repository, spelled as state stores its owner and name.
    pub(crate) fn urls(&self) -> RepoUrls {
        let account = self.state.account(self.owner);
        let owner = account.map_or(self.owner, |account| account.login.as_str());
        let name = account
            .and_then(|account| account.repo(self.name))
            .map_or(self.name, |repo| repo.name.as_str());
        self.request.urls().repo(owner, name)
    }

    /// The webhook user for `explicit`, or the caller, or the repository owner.
    pub(crate) fn sender(
        &self,
        explicit: Option<&types::SimpleUser>,
    ) -> Option<Result<webhook_types::User, PayloadError>> {
        if let Some(user) = explicit {
            return Some(model::user(user));
        }
        let login = self
            .request
            .caller
            .as_ref()
            .map_or(self.owner, |caller| caller.login.as_str());
        let account = self.state.account(login)?;
        Some(model::user(&render::simple_user(self.request, account)))
    }

    /// The webhook repository and sender (see [`Self::sender`]) of an `event` payload, or
    /// `None` when the repository or the sender has no account. A failure to build either is
    /// recorded on the delivery and returned.
    pub(crate) fn parties(
        &mut self,
        event: &str,
        explicit: Option<&types::SimpleUser>,
    ) -> Result<Option<(webhook_types::Repository, webhook_types::User)>, PayloadError> {
        let (Some(repository), Some(sender)) = (self.repository(), self.sender(explicit)) else {
            return Ok(None);
        };
        match repository.and_then(|repository| sender.map(|sender| (repository, sender))) {
            Ok(parties) => Ok(Some(parties)),
            Err(error) => self
                .enqueue_built(event, Err::<(), _>(error))
                .map(|()| None),
        }
    }

    /// The default branch and description stored for the repository.
    pub(crate) fn branch_and_description(&self) -> Option<(String, Option<String>)> {
        let repo = self.state.account(self.owner)?.repo(self.name)?;
        Some((repo.default_branch.clone(), repo.description.clone()))
    }

    /// The repository's git directory, when it has one.
    pub(crate) fn git_dir(&self) -> Option<PathBuf> {
        self.state
            .account(self.owner)
            .and_then(|account| account.repo(self.name))
            .and_then(|repo| repo.git_dir.clone())
    }

    /// Records a delivery of `payload` as `event` to every hook that wants it.
    pub(crate) fn enqueue(&mut self, event: &str, payload: &impl Serialize) {
        if self.wants(event) {
            enqueue(self.state, self.owner, self.name, event, payload, None);
        }
    }

    /// [`Self::enqueue`] for a payload that may have failed to build. A failure is recorded on
    /// the delivery with its error, not queued, and returned.
    pub(crate) fn enqueue_built(
        &mut self,
        event: &str,
        payload: Result<impl Serialize, PayloadError>,
    ) -> Result<(), PayloadError> {
        if !self.wants(event) {
            return Ok(());
        }
        match payload {
            Ok(payload) => {
                enqueue(self.state, self.owner, self.name, event, &payload, None);
                Ok(())
            }
            Err(error) => {
                record_unbuilt(self.state, self.owner, self.name, event, &error, None);
                Err(error)
            }
        }
    }
}
