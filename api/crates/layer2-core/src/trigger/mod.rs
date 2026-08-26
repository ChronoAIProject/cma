//! Durable repository-trigger resources and contribution workflow.
//!
//! This module owns the provider-neutral contract. Repository hosts, identity
//! brokers, HTTP routes, databases, runtimes, and public protocols implement
//! its ports in their own layers.

mod capability;
mod contribution;
mod launch;
mod revision;
mod service;
mod store;
mod template;
mod workspace;

pub use capability::*;
pub use contribution::*;
pub use launch::*;
pub use revision::*;
pub use service::*;
pub use store::*;
pub use workspace::*;

#[cfg(test)]
mod tests;
