use crate::{
    auth::keys::bridge::mint_intent::BridgeSandboxMintIntent,
    bridge::{provisioning::revival::ChannelRevivalAuthority, BridgeControlPlaneLease},
    sandbox::{
        channel_lifecycle_credentials::model::{ChannelLifecycleSnapshot, CredentialPhase},
        channel_provisioning::state::{ChannelProvisioningPhase, ChannelProvisioningSnapshot},
        channel_renewal::model::{ChannelRenewalSnapshot, RenewalPhase},
    },
    store::{bridge_allocations::MutationError, MongoBridgeAllocationStore},
};
use layer2_core::{
    bridge::allocation::{AllocationAuthority, AllocationError, AllocationRecord},
    sandbox::Sandbox,
};
use mongodb::ClientSession;

pub(crate) enum ChannelMintAttempt<'a> {
    Provisioning {
        authority: &'a AllocationAuthority,
        human: &'a BridgeControlPlaneLease,
        snapshot: &'a ChannelProvisioningSnapshot,
    },
    Renewal {
        authority: &'a AllocationAuthority,
        human: &'a BridgeControlPlaneLease,
        snapshot: &'a ChannelRenewalSnapshot,
    },
    Revival {
        permit: &'a ChannelRevivalAuthority,
        snapshot: &'a ChannelLifecycleSnapshot,
    },
}
impl ChannelMintAttempt<'_> {
    pub(crate) fn human(&self) -> &BridgeControlPlaneLease {
        match self {
            Self::Provisioning { human, .. } | Self::Renewal { human, .. } => human,
            Self::Revival { permit, .. } => permit.human(),
        }
    }
    pub(crate) fn allocation(&self) -> &AllocationRecord {
        match self {
            Self::Provisioning { snapshot, .. } => &snapshot.allocation,
            Self::Renewal { snapshot, .. } => &snapshot.allocation,
            Self::Revival { snapshot, .. } => &snapshot.allocation,
        }
    }
    pub(crate) fn sandbox(&self) -> &Sandbox {
        match self {
            Self::Provisioning { snapshot, .. } => &snapshot.sandbox,
            Self::Renewal { snapshot, .. } => &snapshot.sandbox,
            Self::Revival { snapshot, .. } => &snapshot.sandbox,
        }
    }
    pub(crate) fn claim_fence(&self) -> Result<u64, AllocationError> {
        match self {
            Self::Provisioning { snapshot, .. } => Ok(snapshot
                .allocation
                .lease
                .as_ref()
                .ok_or(AllocationError::LeaseUnavailable)?
                .fence),
            Self::Renewal { snapshot, .. } => Ok(snapshot
                .renewal
                .as_ref()
                .ok_or(AllocationError::Corrupt)?
                .claim
                .fence),
            Self::Revival { snapshot, .. } => Ok(snapshot
                .sandbox
                .work
                .lease
                .as_ref()
                .ok_or(AllocationError::LeaseUnavailable)?
                .fence),
        }
    }
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::Provisioning { .. } => "initial",
            Self::Renewal { .. } => "renewal",
            Self::Revival { .. } => "revival",
        }
    }
    pub(crate) fn generation(&self) -> Result<u64, AllocationError> {
        match self {
            Self::Provisioning { .. } => Ok(1),
            Self::Renewal { snapshot, .. } => Ok(snapshot
                .renewal
                .as_ref()
                .ok_or(AllocationError::Corrupt)?
                .generation),
            Self::Revival { snapshot, .. } => Ok(snapshot
                .current_attempt()
                .ok_or(AllocationError::Corrupt)?
                .generation),
        }
    }
    pub(crate) fn credential_id(&self) -> Result<&str, AllocationError> {
        match self {
            Self::Provisioning { snapshot, .. } => snapshot
                .state
                .mint_credential_id
                .as_deref()
                .ok_or(AllocationError::Corrupt),
            Self::Renewal { snapshot, .. } => Ok(&snapshot
                .renewal
                .as_ref()
                .ok_or(AllocationError::Corrupt)?
                .credential_id),
            Self::Revival { snapshot, .. } => Ok(&snapshot
                .current_attempt()
                .ok_or(AllocationError::Corrupt)?
                .credential_id),
        }
    }
    pub(crate) fn intent(&self) -> Result<&BridgeSandboxMintIntent, AllocationError> {
        let value = match self {
            Self::Provisioning { snapshot, .. } => &snapshot.state.mint_intent,
            Self::Renewal { snapshot, .. } => {
                &snapshot
                    .renewal
                    .as_ref()
                    .ok_or(AllocationError::Corrupt)?
                    .mint_intent
            }
            Self::Revival { snapshot, .. } => {
                &snapshot
                    .current_attempt()
                    .ok_or(AllocationError::Corrupt)?
                    .mint_intent
            }
        };
        value.as_ref().ok_or(AllocationError::AuthorityUnavailable)
    }
    pub(crate) fn require_current(&self) -> Result<(), AllocationError> {
        let deployment = match self {
            Self::Provisioning {
                authority,
                snapshot,
                ..
            } => {
                snapshot.validate()?;
                authority.require_record(&snapshot.allocation)?;
                if snapshot.state.phase != ChannelProvisioningPhase::MintStarted {
                    return Err(AllocationError::InvalidTransition);
                }
                let claim = snapshot
                    .allocation
                    .lease
                    .as_ref()
                    .ok_or(AllocationError::LeaseUnavailable)?;
                if claim.expires_at_ms <= mongodb::bson::DateTime::now().timestamp_millis() {
                    return Err(AllocationError::LeaseUnavailable);
                }
                &snapshot.state.settings.deployment_id
            }
            Self::Renewal {
                authority,
                snapshot,
                ..
            } => {
                snapshot.validate()?;
                authority.require_record(&snapshot.allocation)?;
                let state = snapshot.renewal.as_ref().ok_or(AllocationError::Corrupt)?;
                if state.phase != RenewalPhase::MintStarted {
                    return Err(AllocationError::InvalidTransition);
                }
                if state.claim.expires_at_ms <= mongodb::bson::DateTime::now().timestamp_millis() {
                    return Err(AllocationError::LeaseUnavailable);
                }
                &snapshot.provisioning.settings.deployment_id
            }
            Self::Revival { permit, snapshot } => {
                snapshot.validate()?;
                permit.require_current()?;
                if snapshot
                    .current_attempt()
                    .is_none_or(|state| state.phase != CredentialPhase::MintStarted)
                {
                    return Err(AllocationError::InvalidTransition);
                }
                &snapshot.provisioning.settings.deployment_id
            }
        };
        self.intent()?
            .require_binding(self.human(), deployment, self.credential_id()?)
            .map_err(|_| AllocationError::AuthorityUnavailable)
    }
    pub(crate) async fn fence(
        &self,
        store: &MongoBridgeAllocationStore,
        session: &mut ClientSession,
    ) -> Result<(), MutationError> {
        self.require_current()?;
        match self {
            Self::Provisioning {
                authority,
                human,
                snapshot,
            } => {
                store
                    .fence_channel_mint_provisioning(authority, human, snapshot, session)
                    .await?;
            }
            Self::Renewal {
                authority,
                human,
                snapshot,
            } => {
                store
                    .fence_channel_mint_renewal(authority, human, snapshot, session)
                    .await?;
            }
            Self::Revival { permit, snapshot } => {
                if permit.sandbox() != &snapshot.sandbox
                    || permit.allocation() != &snapshot.allocation
                {
                    return Err(AllocationError::Conflict.into());
                }
                store.fence_channel_revival(permit, session).await?;
            }
        }
        Ok(())
    }
}
