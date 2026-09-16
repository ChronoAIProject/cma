use super::{MutationError, Resources};
use crate::sandbox::channel_provisioning::state::{
    ChannelProvisioningSettings, ChannelProvisioningSnapshot, ChannelProvisioningState,
};
use crate::store::{resource_access::read_inventory_policy, sandboxes::read_sandbox};
use layer2_core::{
    bridge::allocation::{AllocationError, AllocationRecord, ChannelSandboxSidecar},
    resource_access::ResourcePolicyRecord,
};
use mongodb::{
    bson::{doc, Document},
    ClientSession,
};

pub(super) async fn snapshot(
    resources: &Resources,
    allocation: &AllocationRecord,
    current: &Document,
    settings: &ChannelProvisioningSettings,
    session: &mut ClientSession,
) -> Result<(ChannelProvisioningSnapshot, Document, ResourcePolicyRecord), MutationError> {
    let document = resources
        .sandboxes
        .find_one(doc! {"_id":allocation.placement.target().sandbox_id.as_str()})
        .session(&mut *session)
        .await?
        .ok_or(AllocationError::Corrupt)?;
    let sandbox = read_sandbox(&document).map_err(|_| AllocationError::Corrupt)?;
    let creation: ChannelSandboxSidecar = mongodb::bson::deserialize_from_bson(
        document
            .get("channel_creation")
            .cloned()
            .ok_or(AllocationError::Corrupt)?,
    )
    .map_err(|_| AllocationError::Corrupt)?;
    if current.get("sandbox_admission") != document.get("channel_creation") {
        return Err(AllocationError::Corrupt.into());
    }
    creation.require_creation(allocation, &sandbox)?;
    let policy = resources
        .policies
        .find_one(doc! {"_id":sandbox.id.as_str()})
        .session(&mut *session)
        .await?
        .ok_or(AllocationError::Corrupt)?;
    let policy = read_inventory_policy(&policy).map_err(|_| AllocationError::Corrupt)?;
    if policy.owner != allocation.owner || policy.sandbox_created_at != sandbox.created_at {
        return Err(AllocationError::IdentityMismatch.into());
    }
    let state = match document.get("channel_provisioning") {
        Some(value) => mongodb::bson::deserialize_from_bson(value.clone())
            .map_err(|_| AllocationError::Corrupt)?,
        None => {
            creation.require_held(allocation, &sandbox)?;
            ChannelProvisioningState::initial(allocation, settings.clone())
        }
    };
    let snapshot = ChannelProvisioningSnapshot {
        allocation: allocation.clone(),
        sandbox,
        creation,
        state,
        home_provenance: document
            .get("channel_home_provenance")
            .map(|value| {
                mongodb::bson::deserialize_from_bson(value.clone())
                    .map_err(|_| AllocationError::Corrupt)
            })
            .transpose()?,
        home_prepare_evidence: document
            .get("channel_home_prepare_evidence")
            .map(|value| {
                mongodb::bson::deserialize_from_bson(value.clone())
                    .map_err(|_| AllocationError::Corrupt)
            })
            .transpose()?,
    };
    snapshot.validate()?;
    Ok((snapshot, document, policy))
}
