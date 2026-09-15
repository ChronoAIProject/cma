//! First real resource commit in a Channel allocation. This only admits a held
//! Sandbox; it grants no provider, credential, Workspace or Agent effect.
mod sidecar;
pub use sidecar::*;

use crate::resource_access::{OrganizationOwnershipEvidence, ResourceOwner, ResourcePolicyRecord};
use crate::sandbox::{
    Sandbox, SandboxCredentialState, SandboxLimits, SandboxOrigin, SandboxSignInState,
    SandboxStatus, SandboxWorkState, SandboxWorkStep,
};

use super::{
    AllocationAuthority, AllocationError, AllocationPlacement, AllocationRecord, AllocationState,
};

#[derive(Clone)]
pub struct ChannelSandboxReservation {
    authority: AllocationAuthority,
    previous: AllocationRecord,
    allocation: AllocationRecord,
    sandbox: Sandbox,
    policy: ResourcePolicyRecord,
    sidecar: ChannelSandboxSidecar,
    live_limit: u16,
}

impl ChannelSandboxReservation {
    pub fn prepare(
        authority: AllocationAuthority,
        previous: AllocationRecord,
    ) -> Result<Self, AllocationError> {
        authority.require_record(&previous)?;
        previous.validate()?;
        let AllocationAuthority::Provisioning(proof) = &authority else {
            return Err(AllocationError::IdentityMismatch);
        };
        if previous.lease.is_some()
            || previous.receipt.is_some()
            || !matches!(
                previous.state,
                AllocationState::Reserved | AllocationState::NeedsConnection
            )
            || (previous.effect_started && previous.state != AllocationState::NeedsConnection)
        {
            return Err(AllocationError::InvalidTransition);
        }
        let (live_limit, baseline) = proof.sandbox_policy();
        if live_limit == 0 {
            return Err(AllocationError::Capacity);
        }
        let AllocationPlacement::NewSandbox { target, profiles } = &previous.placement else {
            return Err(AllocationError::IdentityMismatch);
        };
        let at = crate::rfc3339_at(
            std::time::UNIX_EPOCH + std::time::Duration::from_millis(previous.created_at_ms as u64),
        );
        let subject = match &previous.owner {
            ResourceOwner::Person { subject } | ResourceOwner::Organization { subject } => {
                subject.clone()
            }
        };
        let content = &profiles.sandbox.content;
        let sandbox = Sandbox {
            id: target.sandbox_id.clone(),
            owner: subject,
            name: format!("channel-{}", &target.sandbox_id.as_str()[4..]),
            // This legacy field is provisioning ancestry. The immutable sidecar
            // below records actual Channel creation, owner and source identity.
            origin: SandboxOrigin::Manual,
            profile: profiles.sandbox.clone(),
            status: SandboxStatus::Provisioning,
            failure: None,
            runtime: None,
            credential: SandboxCredentialState {
                reaches: content.credential_services(baseline),
                service_grants: Vec::new(),
                key_generation: 0,
                delivered: false,
                current: None,
                pending: None,
                revoke_debt: Vec::new(),
                sign_in: match content.model_access {
                    crate::profiles::ModelAccess::ChronoaiManaged => {
                        SandboxSignInState::NotRequired
                    }
                    crate::profiles::ModelAccess::OwnCredentials => SandboxSignInState::Required,
                },
            },
            // Existing releases decode this row and find no claimable work.
            work: SandboxWorkState::new(SandboxWorkStep::None, at.clone()),
            limits: SandboxLimits {
                max_workspaces: content.max_workspaces,
                max_agents_per_workspace: content.max_agents_per_workspace,
            },
            lifecycle: Default::default(),
            last_activity_at: at.clone(),
            created_at: at.clone(),
            updated_at: at.clone(),
            retired_at: None,
            purge_after: None,
            revision: 1,
        };
        sandbox.validate().map_err(|_| AllocationError::Corrupt)?;
        let policy = ResourcePolicyRecord {
            sandbox_id: sandbox.id.clone(),
            sandbox_created_at: at.clone(),
            owner: previous.owner.clone(),
            revision: 1,
            organization_evidence: match &previous.owner {
                ResourceOwner::Person { .. } => None,
                ResourceOwner::Organization { subject } => Some(OrganizationOwnershipEvidence {
                    issuer: previous.config.connector.issuer.clone(),
                    organization: subject.clone(),
                    admitted_by: profiles.approved_by.clone(),
                    verified_at: at,
                }),
            },
        };
        policy.validate().map_err(|_| AllocationError::Corrupt)?;
        let sidecar = ChannelSandboxSidecar::capture(&previous, &sandbox)?;
        let mut allocation = previous.clone();
        if !allocation.effect_started {
            allocation.effect_started = true;
            allocation.state = AllocationState::NeedsConnection;
            allocation.revision = allocation
                .revision
                .checked_add(1)
                .ok_or(AllocationError::Corrupt)?;
        }
        allocation.validate()?;
        Ok(Self {
            authority,
            previous,
            allocation,
            sandbox,
            policy,
            sidecar,
            live_limit,
        })
    }

    pub fn require_current(&self) -> Result<(), AllocationError> {
        self.authority.require_record(&self.previous)?;
        self.authority.require_record(&self.allocation)
    }
    pub fn authority(&self) -> &AllocationAuthority {
        &self.authority
    }
    pub fn previous(&self) -> &AllocationRecord {
        &self.previous
    }
    pub fn allocation(&self) -> &AllocationRecord {
        &self.allocation
    }
    pub fn sandbox(&self) -> &Sandbox {
        &self.sandbox
    }
    pub fn policy(&self) -> &ResourcePolicyRecord {
        &self.policy
    }
    pub fn sidecar(&self) -> &ChannelSandboxSidecar {
        &self.sidecar
    }
    pub fn live_limit(&self) -> u16 {
        self.live_limit
    }
    pub fn requires_existing(&self) -> bool {
        self.previous.effect_started
    }
}

#[derive(Clone)]
pub struct ChannelSandboxAdmission {
    pub sandbox: Sandbox,
    pub allocation: AllocationRecord,
    pub sidecar: ChannelSandboxSidecar,
    pub created: bool,
}

#[async_trait::async_trait]
pub trait ChannelSandboxAdmissionStore: Send + Sync {
    /// Current bridge/custody, allocation head, owner quota, Sandbox+Created
    /// event, explicit ownership and creation linkage commit in one transaction.
    async fn admit_channel_sandbox(
        &self,
        request: ChannelSandboxReservation,
    ) -> Result<ChannelSandboxAdmission, AllocationError>;
}
