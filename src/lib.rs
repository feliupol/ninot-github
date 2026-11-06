#![recursion_limit = "256"]

//! In-memory fake of the GitHub REST API and git hosting for end-to-end tests.
//!
//! The public surface:
//!
//! - [`Server`] starts a fake, seeds users, organizations and tokens, and inspects it;
//!   [`server::Options`] configures it, with [`rate_limit::Limits`] and [`rate_limit::Quota`]
//!   for its rate limits.
//! - [`faults`] holds the fault injection types: [`faults::Rule`], [`faults::Target`],
//!   [`faults::Fault`] and the [`faults::Applied`] log.
//! - [`Delivery`] and [`DeliveryHeaders`] describe the webhook deliveries
//!   [`Server::deliveries`] returns.
//! - [`OPERATIONS`], [`ops`], [`Operation`], [`OperationInfo`] and [`Method`] describe the
//!   spec's operations, and [`IMPLEMENTED`] lists those the fake serves.

pub(crate) mod api;
pub mod faults;
mod fixture_git;
pub mod fixtures;
pub(crate) mod graphql;
pub(crate) mod handlers;
pub(crate) mod operation;
pub mod rate_limit;
pub(crate) mod render;
pub(crate) mod router;
pub mod server;
pub mod state;
pub(crate) mod time;

/// REST request and response types generated from the pinned GitHub API spec.
#[allow(clippy::all, dead_code, missing_docs)]
pub(crate) mod types {
    use serde::Deserialize;

    /// Deserializes a present non-null optional request field.
    ///
    /// Generated request models use this when omission differs from a value. Missing fields
    /// default to `None`; explicit `null` is rejected by `T`.
    pub fn deserialize_optional<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
    where
        D: serde::Deserializer<'de>,
        T: Deserialize<'de>,
    {
        T::deserialize(deserializer).map(Some)
    }

    /// Deserializes a present nullable optional request field.
    ///
    /// Generated request models use the outer option for omission and the inner option for an
    /// explicit `null`.
    pub fn deserialize_nullable_optional<'de, D, T>(
        deserializer: D,
    ) -> Result<Option<Option<T>>, D::Error>
    where
        D: serde::Deserializer<'de>,
        T: Deserialize<'de>,
    {
        Option::<T>::deserialize(deserializer).map(Some)
    }

    include!(concat!(env!("OUT_DIR"), "/types.rs"));
}
pub(crate) mod validate;
/// Webhook payload types generated from the pinned octokit/webhooks schemas.
#[allow(clippy::all, missing_docs)]
pub(crate) mod webhook_types {
    include!(concat!(env!("OUT_DIR"), "/webhook_types.rs"));
}
pub(crate) mod webhooks;

pub use graphql::fields as graphql_fields;
pub use handlers::IMPLEMENTED;
pub use operation::{Method, OPERATIONS, Operation, OperationInfo, ops};
pub use server::{Commit, Issue, Project, PullRequest, Repository, Review, Server, WorkflowRun};
pub use state::{Delivery, State};
pub use webhooks::DeliveryHeaders;
