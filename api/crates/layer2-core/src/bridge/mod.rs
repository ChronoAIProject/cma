//! Bridge control-plane definitions. Transport, renewable custody and runtime
//! dispatch adapters must establish their own live authority before effects.

pub mod allocation;
mod authority;
mod capabilities;
pub mod channel_control;
pub mod cleanup;
mod config;
pub mod ingress;
pub mod invoke;
mod management;
mod memory;
mod model;
pub mod outbox;
pub mod partition;
mod service;
pub mod setup;
mod store;
mod validation;

pub use capabilities::*;
pub use config::*;
pub use management::BridgeManagementLease;
pub use memory::MemoryBridgeStore;
pub use model::*;
pub use service::{BridgeMutation, BridgePage, BridgeService};
pub use store::*;
pub use validation::*;

#[cfg(test)]
mod tests;
