//! Explicit authority for a bridge to use a retained Agent. Management consent,
//! immutable grant history and live effect proof are separate boundaries.
mod enable;
mod model;
pub use enable::BridgeInvokeEnableProof;
mod mutation;
mod proof;
mod readiness;
pub use readiness::{BridgeInvokeConsentStatus, BridgeInvokeReadiness};
mod service;
mod store;
mod target;

pub use model::*;
pub use mutation::*;
pub use proof::*;
pub use service::*;
pub use store::*;
pub use target::BridgeInvokeTarget;

#[cfg(test)]
mod tests;
