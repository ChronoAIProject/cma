//! Provider-neutral sponsored credential grants and spend enforcement.
//!
//! A sponsorship policy never contains provider credential material. The
//! control plane persists that material through [`SponsorshipStore`] and gives
//! provider adapters a short-lived [`ProviderCredentialSecret`] only while
//! validating, rotating, revoking, or issuing an expiring grant. The session
//! receives only [`InertGrantCapability`], which is an identifier to present
//! alongside its ordinary authenticated session authority, never a bearer.

mod lifecycle;
mod model;
mod ports;
mod usage;
mod validation;

pub use lifecycle::*;
pub use model::*;
pub use ports::*;
