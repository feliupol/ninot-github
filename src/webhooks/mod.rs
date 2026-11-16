//! Webhook deliveries: queueing, signing and sending in [`delivery`], one payload builder
//! per event in [`events`].

pub mod delivery;
pub mod events;

pub(crate) use delivery::record_unbuilt;
pub use delivery::{DeliveryHeaders, TargetPolicy, enqueue, spawn_sender};
pub(crate) use events::ping::ping_payload;
pub use events::push::enqueue_push;
