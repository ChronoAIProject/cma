//! Resource policy separates retained ownership, authenticated authority and
//! actual attribution. Evidence is supplied by trusted adapters, never by DTOs
//! or historical command receipts. Legacy aggregates remain personally owned.

mod audit;
mod authority;
mod binding;
mod context;
pub mod execution;
mod identity;
mod inventory;
mod inventory_memberships;
pub(crate) use inventory_memberships::InventoryMemberships;
mod lineage;
mod observation;
mod personal;
mod personal_inventory;
mod policy;
pub mod revocations;
mod sandbox_inventory;
mod service;
mod share;
mod share_links;
pub use share_links::{AgentShareAudience, AgentShareLink, CreateAgentShareLink};
mod share_command;
mod sharing_service;
mod standing;
mod store;

pub use audit::{
    ResourceAuthorityObservation, ResourceAuthorizationAudit, ResourceAuthorizationBasis,
    ResourceGrantObservation,
};
pub use authority::{OrganizationAuthority, OrganizationChoice};
pub use context::{
    CallerContext, CredentialContext, CredentialSource, MembershipError, OrganizationMembership,
    OrganizationRole, MAX_MEMBERSHIP_AGE,
};
pub use identity::{ResourceActor, ResourceOwner};
pub use lineage::ResourceLineage;
pub use policy::{AccessDecision, AccessError, AccessPurpose, ResourceCapability, ResourceScope};
pub use service::{AuthorizedResource, ResourceAccessError, ResourceAccessService};
pub use share::{ResourceSelector, ResourceShare, ShareMode, SharingIsolation};
pub use share_command::{
    plan_share_mutation, ResourceShareStore, ShareChange, ShareMutation, ShareMutationPlan,
    ShareMutationReceipt, MAX_ACTIVE_RESOURCE_SHARES,
};
pub use sharing_service::ShareCommand;
pub use store::{
    OrganizationOwnershipEvidence, PersonalAgentInventoryFence, ResourceAccessSnapshot,
    ResourceAccessStore, ResourceAccessStoreError, ResourcePolicyFence, ResourcePolicyRecord,
    ResourceTarget,
};

#[cfg(test)]
mod request_read_tests;
#[cfg(test)]
mod tests;
