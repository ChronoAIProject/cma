//! Bridge control-plane definitions. Transport, renewable custody and runtime
//! dispatch adapters must establish their own live authority before effects.

mod authority;
mod config;
pub mod ingress;
mod management;
mod memory;
mod model;
mod service;
mod store;
mod validation;

pub use config::*;
pub use management::BridgeManagementLease;
pub use memory::MemoryBridgeStore;
pub use model::*;
pub use service::{BridgeMutation, BridgePage, BridgeService};
pub use store::*;
pub use validation::*;

#[cfg(test)]
mod tests;
