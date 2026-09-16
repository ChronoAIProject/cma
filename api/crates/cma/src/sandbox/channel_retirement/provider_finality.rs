//! Exact provider receipts; missing original evidence remains a retention hold.
use super::model::ChannelRetirementSnapshot;
use crate::sandbox::channel_provisioning::state::ChannelProvisioningSnapshot;
use layer2_core::{bridge::allocation::AllocationError, SandboxId};
use layer3_adapter::{HomeFinalityReceipt, HomeVolumePolicy, RuntimeFinalityReceipt};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProviderDispatchOrigin {
    pub schema_version: u16,
    pub allocation_id: String,
    pub sandbox_id: SandboxId,
    pub sandbox_created_at: String,
    pub allocation_operation_id: String,
    pub creation_sha256: String,
    pub original_settings_sha256: String,
    pub original_home_policy: Option<HomeVolumePolicy>,
}

impl ProviderDispatchOrigin {
    pub fn capture(snapshot: &ChannelProvisioningSnapshot) -> Result<Self, AllocationError> {
        snapshot.validate()?;
        Ok(Self {
            schema_version: 1,
            allocation_id: snapshot.allocation.id.clone(),
            sandbox_id: snapshot.sandbox.id.clone(),
            sandbox_created_at: snapshot.sandbox.created_at.clone(),
            allocation_operation_id: snapshot.allocation.operation_id.clone(),
            creation_sha256: digest(&snapshot.creation)?,
            original_settings_sha256: digest(&snapshot.state.settings)?,
            original_home_policy: snapshot.state.settings.home_policy.clone(),
        })
    }

    pub fn require(&self, snapshot: &ChannelRetirementSnapshot) -> Result<(), AllocationError> {
        if self.schema_version != 1
            || self.allocation_id != snapshot.allocation.id
            || self.sandbox_id != snapshot.sandbox.id
            || self.sandbox_created_at != snapshot.sandbox.created_at
            || self.allocation_operation_id != snapshot.allocation.operation_id
            || self.creation_sha256 != digest(&snapshot.creation)?
            || self.original_settings_sha256 != digest(&snapshot.provisioning.settings)?
            || self.original_home_policy != snapshot.provisioning.settings.home_policy
        {
            return Err(AllocationError::IdentityMismatch);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum ProviderHomeFinality {
    NotConfigured { original_settings_sha256: String },
    Removed { receipt: Box<HomeFinalityReceipt> },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProviderFinalityReceipt {
    pub schema_version: u16,
    pub origin: ProviderDispatchOrigin,
    pub runtime_dispatch_set_sha256: String,
    pub home_dispatch_set_sha256: String,
    pub runtime: RuntimeFinalityReceipt,
    pub home: ProviderHomeFinality,
}

impl ProviderFinalityReceipt {
    pub fn require(&self, snapshot: &ChannelRetirementSnapshot) -> Result<(), AllocationError> {
        self.origin.require(snapshot)?;
        self.runtime
            .validate()
            .map_err(|_| AllocationError::IdentityMismatch)?;
        let state = snapshot
            .state
            .as_ref()
            .ok_or(AllocationError::IdentityMismatch)?;
        let runtime = state
            .runtime
            .as_ref()
            .ok_or(AllocationError::IdentityMismatch)?;
        let request = layer3_adapter::RuntimeProvisionRequest::new(
            1,
            crate::sandbox::runtime_mapping::sandbox_spec(
                &snapshot.sandbox,
                &snapshot.provisioning.settings.deployment_id,
            ),
        );
        if self.schema_version != 1
            || !is_digest(&self.runtime_dispatch_set_sha256)
            || !is_digest(&self.home_dispatch_set_sha256)
            || runtime.provision_generation != Some(1)
            || snapshot.sandbox.lifecycle.runtime_generation_high_water != 1
            || snapshot.sandbox.lifecycle.runtime_provision.is_some()
            || self.runtime.original().handle().0 != runtime.runtime_id
            || self.runtime.original().request_sha256()
                != layer3_adapter::provider_request_digest(&request)
                    .map_err(|_| AllocationError::Corrupt)?
            || self.runtime.delete_operation_id() != delete_operation_id(snapshot)
        {
            return Err(AllocationError::IdentityMismatch);
        }
        match &self.home {
            ProviderHomeFinality::NotConfigured {
                original_settings_sha256,
            } if self.origin.original_home_policy.is_none()
                && snapshot.provisioning.settings.home_policy.is_none()
                && snapshot.sandbox.lifecycle.home.is_none()
                && snapshot.home_provenance.is_none()
                && snapshot.home_prepare_evidence.is_none()
                && original_settings_sha256 == &self.origin.original_settings_sha256 =>
            {
                Ok(())
            }
            // Configured home integration requires an original backing pin and
            // complete home dispatch journal; no current observation supplies it.
            _ => Err(AllocationError::AuthorityUnavailable),
        }
    }

    pub fn digest(&self) -> Result<String, AllocationError> {
        digest(self)
    }
}

pub(crate) fn delete_operation_id(snapshot: &ChannelRetirementSnapshot) -> String {
    format!(
        "{}:provider-runtime-delete:v1",
        snapshot.allocation.operation_id
    )
}
pub(crate) fn provisioning_request(
    snapshot: &ChannelProvisioningSnapshot,
) -> layer3_adapter::RuntimeProvisionRequest {
    layer3_adapter::RuntimeProvisionRequest::new(
        1,
        crate::sandbox::runtime_mapping::sandbox_spec(
            &snapshot.sandbox,
            &snapshot.state.settings.deployment_id,
        ),
    )
}
pub(crate) fn digest(value: &impl Serialize) -> Result<String, AllocationError> {
    layer2_core::profiles::content_digest(&("cma.channel.provider-finality.v1", value))
        .map_err(|_| AllocationError::Corrupt)
}
fn is_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|v| v.is_ascii_digit() || (b'a'..=b'f').contains(&v))
}
