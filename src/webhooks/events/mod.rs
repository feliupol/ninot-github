//! Payload builders, one module per webhook event.

pub mod ping;
pub mod push;

use crate::types;
use crate::webhook_types;

/// Projects the REST repository into the webhook contract.
fn repository(repository: &types::FullRepository) -> webhook_types::Repository {
    repository.clone().into()
}
