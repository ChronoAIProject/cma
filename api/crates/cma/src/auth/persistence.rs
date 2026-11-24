//! Durable authority required by browser and bearer authentication.

#[cfg(test)]
use std::time::SystemTime;

#[cfg(test)]
use layer2_core::{StoreError, SubjectId};

pub(super) use cma_am_auth::next_auth_counter;
pub use cma_am_auth::{
    ApiBearerCustody, AuthStore, BrowserLogin, DurableSubjectTokens, LoginAttempt, SecretDigest,
    SecretValue, SubjectAuthState, SubjectRefreshLease,
};

#[cfg(test)]
mod memory;
#[cfg(test)]
pub use memory::MemoryAuthStore;

#[cfg(test)]
#[path = "persistence_tests.rs"]
mod tests;

#[cfg(test)]
pub(crate) mod test_observer;
