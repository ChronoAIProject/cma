use crate::bridge::{
    invoke::{BridgeInvokeEffect, BridgeInvokeProof},
    partition::{NormalizedPartitionSource, PartitionAdmissionDecision, PartitionSourcePin},
    BridgeCustodyPin, BridgeRecord, BridgeValidation,
};

use super::{
    AllocationError, AllocationIdentity, AllocationPlacement, AllocationProfilePlan,
    AllocationRecord,
};

/// Checked partition admission, with no allocation or effect authority.
#[derive(Clone)]
pub struct AllocationAdmission {
    pub(super) bridge: BridgeRecord,
    pub(super) source: PartitionSourcePin,
    pub(super) identity: AllocationIdentity,
}

impl AllocationAdmission {
    pub fn new(
        bridge: BridgeRecord,
        source: &NormalizedPartitionSource,
    ) -> Result<Self, AllocationError> {
        let pin = bridge
            .admission_pin()
            .map_err(|_| AllocationError::AdmissionDenied)?;
        if &pin != source.source_pin().work_pin() || &bridge.owner != source.owner() {
            return Err(AllocationError::IdentityMismatch);
        }
        Self::from_accepted(bridge, source)
    }

    /// An already authenticated inbox retains its original configuration.
    /// Later generations may drain that work or explicitly suspend it.
    pub fn from_accepted(
        bridge: BridgeRecord,
        source: &NormalizedPartitionSource,
    ) -> Result<Self, AllocationError> {
        let pin = source.source_pin().work_pin();
        require_accepted_generation(&bridge, pin)?;
        if &bridge.owner != source.owner() || bridge.id != pin.bridge_id {
            return Err(AllocationError::IdentityMismatch);
        }
        let config = &bridge
            .pinned(pin)
            .map_err(|_| AllocationError::Corrupt)?
            .config;
        let PartitionAdmissionDecision::Admitted { key, .. } = source
            .admit(config)
            .map_err(|_| AllocationError::AdmissionDenied)?
        else {
            return Err(AllocationError::AdmissionDenied);
        };
        Ok(Self {
            identity: AllocationIdentity {
                bridge_id: bridge.id.clone(),
                key,
                generation: 1,
            },
            bridge,
            source: source.source_pin().clone(),
        })
    }
    pub fn identity(&self) -> &AllocationIdentity {
        &self.identity
    }
    pub fn after_reset(mut self, previous: &AllocationRecord) -> Result<Self, AllocationError> {
        previous.validate()?;
        if previous.identity.bridge_id != self.identity.bridge_id
            || previous.identity.key != self.identity.key
        {
            return Err(AllocationError::IdentityMismatch);
        }
        self.identity.generation = previous.identity.generation + 1;
        self.identity.id()?;
        Ok(self)
    }
}

/// A fixed-target invoke proof and new-resource creation proof cannot be
/// substituted. Neither variant is durable or convertible to CallerContext.
#[derive(Clone)]
pub enum AllocationAuthority {
    Existing(Box<BridgeInvokeProof>),
    Provisioning(Box<AllocationProvisioningProof>),
}

#[derive(Clone)]
pub struct AllocationProvisioningProof {
    bridge: BridgeRecord,
    allocation_id: String,
    plan: AllocationProfilePlan,
    connection: BridgeValidation,
}

impl AllocationProvisioningProof {
    pub(super) fn sandbox_policy(&self) -> (u16, &[crate::ServiceId]) {
        (
            self.plan.owner_live_sandbox_limit,
            &self.plan.baseline_services,
        )
    }

    pub fn new(
        bridge: BridgeRecord,
        identity: &AllocationIdentity,
        plan: AllocationProfilePlan,
        connection: BridgeValidation,
    ) -> Result<Self, AllocationError> {
        if bridge != plan.bridge || bridge.id != identity.bridge_id {
            return Err(AllocationError::IdentityMismatch);
        }
        let proof = Self {
            bridge,
            allocation_id: identity.id()?,
            plan,
            connection,
        };
        proof.require_current()?;
        Ok(proof)
    }
    fn require_current(&self) -> Result<(), AllocationError> {
        if tokio::time::Instant::now() >= self.plan.deadline {
            return Err(AllocationError::AuthorityUnavailable);
        }
        let config = &self
            .bridge
            .pinned(&self.plan.pin)
            .map_err(|_| AllocationError::Corrupt)?
            .config;
        self.plan.profiles.validate(config)?;
        require_accepted_generation(&self.bridge, &self.plan.pin)?;
        self.connection
            .require(&self.bridge.owner, config)
            .map_err(|_| AllocationError::AuthorityUnavailable)
    }
}

impl AllocationAuthority {
    pub fn config(&self) -> Result<&crate::bridge::BridgeConfig, AllocationError> {
        match self {
            Self::Existing(proof) => Ok(&proof.grant().config),
            Self::Provisioning(proof) => Ok(&proof
                .bridge
                .pinned(&proof.plan.pin)
                .map_err(|_| AllocationError::IdentityMismatch)?
                .config),
        }
    }
    pub fn bridge(&self) -> &BridgeRecord {
        match self {
            Self::Existing(proof) => proof.bridge(),
            Self::Provisioning(proof) => &proof.bridge,
        }
    }
    pub fn custody_pin(&self) -> &BridgeCustodyPin {
        match self {
            Self::Existing(proof) => proof.custody_pin(),
            Self::Provisioning(proof) => proof.connection.custody_pin(),
        }
    }
    pub fn require_current(&self) -> Result<(), AllocationError> {
        match self {
            Self::Existing(proof) => proof
                .require_current()
                .map_err(|_| AllocationError::AuthorityUnavailable),
            Self::Provisioning(proof) => proof.require_current(),
        }
    }
    pub fn require_snapshot(&self, bridge: &BridgeRecord) -> Result<(), AllocationError> {
        self.require_current()?;
        if self.bridge() != bridge {
            return Err(AllocationError::Conflict);
        }
        Ok(())
    }
    pub fn require_record(&self, record: &AllocationRecord) -> Result<(), AllocationError> {
        self.require_current()?;
        if record.owner != self.bridge().owner
            || record.pin.bridge_id != self.bridge().id
            || self
                .bridge()
                .pinned(&record.pin)
                .map_err(|_| AllocationError::IdentityMismatch)?
                .config
                != record.config
        {
            return Err(AllocationError::IdentityMismatch);
        }
        match (self, &record.placement) {
            (Self::Existing(proof), AllocationPlacement::ExistingAgent { target, grant_id })
                if proof.grant().pin == record.pin
                    && &proof.grant().target.ids == target
                    && proof.grant().id.as_str() == grant_id
                    && proof.effect()
                        == &BridgeInvokeEffect::BindExistingTarget {
                            allocation_id: record.id.clone(),
                        } =>
            {
                Ok(())
            }
            (Self::Provisioning(proof), AllocationPlacement::NewSandbox { profiles, .. })
                if proof.allocation_id == record.id
                    && profiles.as_ref() == &proof.plan.profiles
                    && record.pin == proof.plan.pin =>
            {
                Ok(())
            }
            _ => Err(AllocationError::IdentityMismatch),
        }
    }
    pub(super) fn placement(&self, id: &str) -> Result<AllocationPlacement, AllocationError> {
        self.require_current()?;
        match self {
            Self::Existing(proof) => Ok(AllocationPlacement::ExistingAgent {
                target: proof.grant().target.ids.clone(),
                grant_id: proof.grant().id.as_str().to_owned(),
            }),
            Self::Provisioning(proof) if proof.allocation_id == id => {
                Ok(AllocationPlacement::NewSandbox {
                    target: super::model::reserved_target(id)?,
                    profiles: Box::new(proof.plan.profiles.clone()),
                })
            }
            _ => Err(AllocationError::IdentityMismatch),
        }
    }
}

pub(super) fn require_accepted_generation(
    bridge: &BridgeRecord,
    pin: &crate::bridge::BridgeWorkPin,
) -> Result<(), AllocationError> {
    use crate::bridge::{AcceptedWorkPolicy, BridgeState};
    bridge
        .pinned(pin)
        .map_err(|_| AllocationError::IdentityMismatch)?;
    if bridge.emergency.execution_stopped
        || matches!(
            bridge.state,
            BridgeState::Disabled {
                accepted_work: AcceptedWorkPolicy::SuspendOriginal
            } | BridgeState::Retired {
                accepted_work: AcceptedWorkPolicy::SuspendOriginal
            }
        )
        || bridge.generations.iter().any(|generation| {
            generation.config_revision > pin.config_revision
                && generation.previous_work == Some(AcceptedWorkPolicy::SuspendOriginal)
        })
    {
        return Err(AllocationError::AuthorityUnavailable);
    }
    Ok(())
}
