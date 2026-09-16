//! Provider proof and dispatch journal are joined inside the retirement fence.
mod dispatch;
mod home_dispatch;
mod teardown;
use super::super::MutationError;
use crate::sandbox::channel_retirement::{
    model::ChannelRetirementSnapshot,
    provider_finality::{digest, ProviderDispatchOrigin, ProviderFinalityReceipt},
};
use crate::store::{durability::majority_collection, sandbox_mongo::record_bson};
pub(crate) use dispatch::begin_origin;
use layer2_core::bridge::allocation::AllocationError;
use layer3_adapter::RuntimeProvisionReceipt;
use mongodb::{
    bson::{doc, Document},
    ClientSession, Database,
};
use serde::{Deserialize, Serialize};
pub(crate) use teardown::{write_home_delete, write_runtime_finality};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeDispatch {
    operation_id: String,
    request_sha256: String,
    generation: u64,
    receipt: Option<RuntimeProvisionReceipt>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct HomeDispatch {
    operation_id: String,
    request: layer3_adapter::HomeVolumeRequest,
    receipt: Option<layer3_adapter::HomeProvisionReceipt>,
    backing: Option<layer3_adapter::HomeBackingIdentity>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DispatchJournal {
    origin: ProviderDispatchOrigin,
    runtime: Vec<RuntimeDispatch>,
    #[serde(default)]
    home: Vec<HomeDispatch>,
}

impl crate::store::MongoBridgeAllocationStore {
    /// The driver already fenced this retiring snapshot, and revalidates it
    /// again immediately before the provider effect. Dispatch admission is
    /// forbidden once the allocation leaves provisioning.
    pub(crate) async fn channel_provider_runtime(
        &self,
        snapshot: &ChannelRetirementSnapshot,
    ) -> Result<Option<ReadyRuntimeJournal>, AllocationError> {
        let database = self.bridges.client.database(&self.records.namespace().db);
        let mut session = self
            .bridges
            .client
            .start_session()
            .await
            .map_err(|_| AllocationError::Unavailable)?;
        crate::store::sandbox_mongo::start_transaction(&mut session)
            .await
            .map_err(|_| AllocationError::Unavailable)?;
        let result = read_ready_runtime(&database, snapshot, &mut session).await;
        let _ = session.abort_transaction().await;
        result.map_err(|error| match error {
            MutationError::Core(error) => error,
            MutationError::Mongo(_) => AllocationError::Unavailable,
        })
    }
}

pub(crate) struct ReadyRuntimeJournal {
    pub origin: ProviderDispatchOrigin,
    pub original: RuntimeProvisionReceipt,
    pub runtime_dispatch_set_sha256: String,
    pub home_dispatch_set_sha256: String,
    pub home: Option<layer3_adapter::HomeBackingIdentity>,
    pub runtime_finality: Option<layer3_adapter::RuntimeFinalityReceipt>,
    pub home_delete: Option<layer3_adapter::HomeDeleteReceipt>,
}

async fn read_journal(
    database: &Database,
    id: &str,
    session: &mut ClientSession,
) -> Result<Option<DispatchJournal>, MutationError> {
    let collection =
        majority_collection::<Document>(database, "channel_provider_dispatch_journals");
    let Some(document) = collection
        .find_one(doc! {"_id":id})
        .session(session)
        .await?
    else {
        return Ok(None);
    };
    let journal = mongodb::bson::deserialize_from_bson(
        document
            .get("record")
            .cloned()
            .ok_or(AllocationError::Corrupt)?,
    )
    .map_err(|_| AllocationError::Corrupt)?;
    Ok(Some(journal))
}

pub(crate) async fn read_ready_runtime(
    database: &Database,
    snapshot: &ChannelRetirementSnapshot,
    session: &mut ClientSession,
) -> Result<Option<ReadyRuntimeJournal>, MutationError> {
    let Some(journal) = read_journal(database, &snapshot.allocation.id, session).await? else {
        return Ok(None);
    };
    journal.origin.require(snapshot)?;
    if journal.runtime.len() != 1
        || snapshot.sandbox.lifecycle.runtime_generation_high_water != 1
        || snapshot.sandbox.lifecycle.runtime_provision.is_some()
    {
        return Ok(None);
    }
    let (home, home_dispatch_set_sha256) = match &journal.origin.original_home_policy {
        None if journal.home.is_empty() => (
            None,
            digest(&(
                "no-home-dispatches",
                &journal.origin.original_settings_sha256,
            ))?,
        ),
        Some(policy) if journal.home.len() == 1 => {
            let row = &journal.home[0];
            let (Some(receipt), Some(backing)) = (&row.receipt, &row.backing) else {
                return Ok(None);
            };
            receipt.validate().map_err(|_| AllocationError::Corrupt)?;
            backing.validate().map_err(|_| AllocationError::Corrupt)?;
            let attachment = snapshot
                .sandbox
                .lifecycle
                .home
                .as_ref()
                .and_then(|home| home.attachment())
                .ok_or(AllocationError::IdentityMismatch)?;
            let provenance = snapshot
                .home_provenance
                .as_ref()
                .ok_or(AllocationError::IdentityMismatch)?;
            if receipt.operation_id() != row.operation_id
                || receipt.home().request != row.request
                || &row.request.policy != policy
                || receipt.home() != &attachment
                || backing.reference.home != attachment
                || backing.reference.provider_scope != provenance.scope
                || provenance.first_receipt.as_ref() != Some(&attachment.receipt)
            {
                return Err(AllocationError::IdentityMismatch.into());
            }
            (Some(backing.clone()), digest(&journal.home)?)
        }
        _ => return Ok(None),
    };
    let dispatch = &journal.runtime[0];
    let Some(original) = &dispatch.receipt else {
        return Ok(None);
    };
    original.validate().map_err(|_| AllocationError::Corrupt)?;
    let binding = snapshot
        .state
        .as_ref()
        .and_then(|state| state.runtime.as_ref())
        .ok_or(AllocationError::IdentityMismatch)?;
    if dispatch.generation != 1
        || dispatch.operation_id != original.operation_id()
        || dispatch.request_sha256 != original.request_sha256()
        || original.handle().0 != binding.runtime_id
        || binding.provision_generation != Some(1)
    {
        return Err(AllocationError::IdentityMismatch.into());
    }
    let mut ready = ReadyRuntimeJournal {
        runtime_dispatch_set_sha256: digest(&journal.runtime)?,
        home_dispatch_set_sha256,
        home,
        runtime_finality: None,
        home_delete: None,
        origin: journal.origin,
        original: original.clone(),
    };
    teardown::read_effects(database, snapshot, &mut ready, session).await?;
    Ok(Some(ready))
}

pub(crate) async fn read_verified(
    database: &Database,
    snapshot: &ChannelRetirementSnapshot,
    session: &mut ClientSession,
) -> Result<Option<ProviderFinalityReceipt>, MutationError> {
    let collection =
        majority_collection::<Document>(database, "channel_provider_finality_receipts");
    let Some(document) = collection
        .find_one(doc! {"_id":&snapshot.allocation.id})
        .session(&mut *session)
        .await?
    else {
        return Ok(None);
    };
    let receipt: ProviderFinalityReceipt = mongodb::bson::deserialize_from_bson(
        document
            .get("record")
            .cloned()
            .ok_or(AllocationError::Corrupt)?,
    )
    .map_err(|_| AllocationError::Corrupt)?;
    receipt.require(snapshot)?;
    let ready = read_ready_runtime(database, snapshot, session)
        .await?
        .ok_or(AllocationError::IdentityMismatch)?;
    require_journal(&receipt, &ready)?;
    if document.get_str("sha256").ok() != Some(receipt.digest()?.as_str()) {
        return Err(AllocationError::Corrupt.into());
    }
    Ok(Some(receipt))
}

pub(crate) async fn write_verified(
    database: &Database,
    snapshot: &ChannelRetirementSnapshot,
    receipt: &ProviderFinalityReceipt,
    session: &mut ClientSession,
) -> Result<(), MutationError> {
    receipt.require(snapshot)?;
    let ready = read_ready_runtime(database, snapshot, session)
        .await?
        .ok_or(AllocationError::AuthorityUnavailable)?;
    require_journal(receipt, &ready)?;
    if let Some(previous) = read_verified(database, snapshot, session).await? {
        return if previous == *receipt {
            Ok(())
        } else {
            Err(AllocationError::IdentityMismatch.into())
        };
    }
    majority_collection::<Document>(database, "channel_provider_finality_receipts")
        .insert_one(
            doc! {"_id":&snapshot.allocation.id,"record":record_bson(receipt)
            .map_err(|_| AllocationError::Corrupt)?,"sha256":receipt.digest()?},
        )
        .session(session)
        .await?;
    Ok(())
}

fn require_journal(
    receipt: &ProviderFinalityReceipt,
    ready: &ReadyRuntimeJournal,
) -> Result<(), AllocationError> {
    if receipt.origin != ready.origin
        || receipt.runtime.original() != &ready.original
        || receipt.runtime_dispatch_set_sha256 != ready.runtime_dispatch_set_sha256
        || receipt.home_dispatch_set_sha256 != ready.home_dispatch_set_sha256
        || ready
            .runtime_finality
            .as_ref()
            .is_some_and(|proof| proof != &receipt.runtime)
    {
        return Err(AllocationError::IdentityMismatch);
    }
    match (&receipt.home, &ready.home, &ready.home_delete) {
        (crate::sandbox::channel_retirement::provider_finality::ProviderHomeFinality::NotConfigured { .. }, None, None) => (),
        (crate::sandbox::channel_retirement::provider_finality::ProviderHomeFinality::Removed { receipt }, Some(original), Some(delete))
            if receipt.original() == original && &receipt.delete_receipt() == delete && ready.runtime_finality.is_some() => (),
        _ => return Err(AllocationError::IdentityMismatch),
    }
    Ok(())
}
