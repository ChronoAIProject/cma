//! Separate, explicit consent for a sender to control only its own Channel
//! source. Neither an old invoke grant nor Channel creation implies consent.
mod access;
mod model;
mod service;
mod store;

pub use access::{SourceControlAccess, SourceControlWrite};
pub use model::*;
pub use service::BridgeSourceControlService;
pub use store::BridgeSourceControlStore;
