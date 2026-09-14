//! Resource policy separates retained ownership, authenticated authority and
//! actual attribution. Evidence is supplied by trusted adapters, never by DTOs
//! or historical command receipts. Legacy aggregates remain personally owned.

mod authority;
mod context;
mod identity;
mod policy;
mod share;

pub use authority::{OrganizationAuthority, OrganizationChoice};
pub use context::{
    CallerContext, CredentialContext, MembershipError, OrganizationMembership, OrganizationRole,
    MAX_MEMBERSHIP_AGE,
};
pub use identity::{ResourceActor, ResourceOwner};
pub use policy::{AccessDecision, AccessError, AccessPurpose, ResourceCapability, ResourceScope};
pub use share::{ResourceSelector, ResourceShare, ShareMode, SharingIsolation};

#[cfg(test)]
mod tests;
