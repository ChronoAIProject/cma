//! Ordered, durable erasure of a personal account.
mod effect;
mod memory;
mod model;
mod service;
mod standing;
mod types;
pub mod verification;
pub mod worker;
#[cfg(test)]
mod worker_tests;
mod worker_types;
mod worker_validation;
pub use effect::{ErasureDigest, ErasureEffectCheckpoint};
pub use memory::MemoryAccountDeletionStore;
pub use model::AccountDeletion;
pub use service::{AccountDeletionService, AccountDeletionStore, AccountErasureEffects};
pub use standing::{AccountStanding, AvailableAccounts};
pub use types::*;
#[cfg(test)]
mod lease_tests;
#[cfg(test)]
mod model_tests;
#[cfg(test)]
mod service_tests;
