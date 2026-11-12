//! Personal durable documents, independent of the environment that produced them.
//! Metadata and access change atomically; immutable source bytes live behind a
//! separate bounded object repository. No caller selects an object location.
mod mutation;
mod quota;
mod runtime;
mod service;
mod types;
mod validation;
pub use quota::*;
pub use service::{DocumentService, DOCUMENT_SOURCE_WRITE_SLOTS};
pub use types::*;
#[cfg(any(test, feature = "test-support"))]
pub mod memory;
#[cfg(test)]
mod tests;

#[cfg(test)]
mod runtime_tests;

#[cfg(test)]
mod revocation_tests;

#[cfg(test)]
mod revocation_race_tests;

#[cfg(test)]
mod rollout_tests;

#[cfg(test)]
mod quota_tests;
