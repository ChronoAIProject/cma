//! Durable authority required by browser and bearer authentication.

#[cfg(any(test, feature = "test-support"))]
use std::time::SystemTime;

#[cfg(any(test, feature = "test-support"))]
use layer2_core::{StoreError, SubjectId};

pub(super) use crate::next_auth_counter;
pub use crate::{
    ApiBearerCustody, AuthStore, BrowserLogin, DurableSubjectTokens, LoginAttempt, SecretDigest,
    SecretValue, SubjectAuthState, SubjectRefreshLease,
};

#[cfg(any(test, feature = "test-support"))]
mod memory;
#[cfg(any(test, feature = "test-support"))]
pub use memory::MemoryAuthStore;

#[cfg(test)]
#[path = "persistence_tests.rs"]
mod tests;

#[cfg(any(test, feature = "test-support"))]
pub mod test_observer;
