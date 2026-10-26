//! Durable ordinary-routing admission, deliberately disconnected from ingress.
//!
//! Immutable encrypted chunks precede one create-only root manifest. Only the
//! root commits admission. An interrupted chunk upload is never acknowledged as
//! accepted and needs the original request to retry; it cannot launch a turn.
//! Recovery restores the winning snapshot, not current routing configuration.
//! All records share a fixed retention boundary of at most seven days. Stored
//! event-reference expiry remains unchanged and is checked separately by any
//! future dispatcher. Delivery isolation and fenced scheduling are still absent.
mod candidate;
mod codec;
mod recovery;
mod store;
mod types;

pub use candidate::Candidate;
pub use recovery::{RecoveryIssue, RecoveryPage};
pub use store::PlanStore;
pub use types::{AdmissionError, AdmissionState, AdmittedPlan, EventIdentity, StoredTarget};

#[cfg(test)]
mod security_tests;
#[cfg(test)]
mod test_server;
#[cfg(test)]
mod tests;
