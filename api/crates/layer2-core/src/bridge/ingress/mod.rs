//! Signed NyxID callback acceptance. A verified callback grants only permission
//! to persist an inbox entry; deferred work requires separate current authority.
mod capabilities;
mod jwt;
mod model;
mod payload;
mod redelivery;
mod service;
mod timestamp;

pub use capabilities::{AuthenticatedCallbackPayload, CallbackCapabilityExtractor};
pub use jwt::{CallbackKeySet, CallbackTokenPolicy};
pub use model::*;
pub use redelivery::{VerifiedPlatformRedelivery, VerifiedRedeliveryParent};
pub use service::{CallbackHeaders, CallbackVerifier};

#[cfg(test)]
mod tests;
