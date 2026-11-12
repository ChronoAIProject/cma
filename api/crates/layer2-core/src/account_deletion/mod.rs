//! Ordered, durable erasure of a personal account.
mod memory;
mod model;
mod service;
mod standing;
mod types;
pub use memory::MemoryAccountDeletionStore;
pub use model::AccountDeletion;
pub use service::{AccountDeletionService, AccountDeletionStore, AccountErasureEffects};
pub use standing::{AccountStanding, AvailableAccounts};
pub use types::*;
#[cfg(test)]
mod model_tests;
#[cfg(test)]
mod service_tests;
