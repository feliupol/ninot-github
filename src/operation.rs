//! Operations generated from the pinned GitHub OpenAPI description.

pub use ninot_common::operation::{Method, Operation, OperationInfo};

include!(concat!(env!("OUT_DIR"), "/operations.rs"));
