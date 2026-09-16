use super::*;
use layer3_adapter::{HomeDeleteReceipt, RuntimeFinalityReceipt};

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Effects {
    runtime: Option<RuntimeFinalityReceipt>,
    home_delete: Option<HomeDeleteReceipt>,
}

fn require_runtime(
    snapshot: &ChannelRetirementSnapshot,
    ready: &ReadyRuntimeJournal,
    receipt: &RuntimeFinalityReceipt,
) -> Result<(), AllocationError> {
    receipt
        .validate()
        .map_err(|_| AllocationError::IdentityMismatch)?;
    if receipt.original() != &ready.original
        || receipt.delete_operation_id()
            != crate::sandbox::channel_retirement::provider_finality::delete_operation_id(snapshot)
    {
        return Err(AllocationError::IdentityMismatch);
    }
    Ok(())
}
fn require_home(
    snapshot: &ChannelRetirementSnapshot,
    ready: &ReadyRuntimeJournal,
    receipt: &HomeDeleteReceipt,
) -> Result<(), AllocationError> {
    receipt
        .validate()
        .map_err(|_| AllocationError::IdentityMismatch)?;
    if ready.runtime_finality.is_none()
        || ready.home.as_ref() != Some(receipt.original())
        || receipt.operation_id()
            != crate::sandbox::channel_retirement::provider_finality::home_delete_operation_id(
                snapshot,
            )
    {
        return Err(AllocationError::IdentityMismatch);
    }
    Ok(())
}

pub(super) async fn read_effects(
    database: &Database,
    snapshot: &ChannelRetirementSnapshot,
    ready: &mut ReadyRuntimeJournal,
    session: &mut ClientSession,
) -> Result<(), MutationError> {
    let Some(document) =
        majority_collection::<Document>(database, "channel_provider_teardown_receipts")
            .find_one(doc! {"_id": &snapshot.allocation.id})
            .session(session)
            .await?
    else {
        return Ok(());
    };
    let effects: Effects = mongodb::bson::deserialize_from_bson(
        document
            .get("record")
            .cloned()
            .ok_or(AllocationError::Corrupt)?,
    )
    .map_err(|_| AllocationError::Corrupt)?;
    if let Some(receipt) = effects.runtime {
        require_runtime(snapshot, ready, &receipt)?;
        ready.runtime_finality = Some(receipt);
    }
    if let Some(receipt) = effects.home_delete {
        require_home(snapshot, ready, &receipt)?;
        ready.home_delete = Some(receipt);
    }
    Ok(())
}

pub(crate) async fn write_runtime_finality(
    database: &Database,
    snapshot: &ChannelRetirementSnapshot,
    receipt: &RuntimeFinalityReceipt,
    session: &mut ClientSession,
) -> Result<(), MutationError> {
    let mut ready = read_ready_runtime(database, snapshot, session)
        .await?
        .ok_or(AllocationError::AuthorityUnavailable)?;
    require_runtime(snapshot, &ready, receipt)?;
    if ready
        .runtime_finality
        .as_ref()
        .is_some_and(|previous| previous != receipt)
    {
        return Err(AllocationError::IdentityMismatch.into());
    }
    ready.runtime_finality = Some(receipt.clone());
    write(database, snapshot, ready, session).await
}

pub(crate) async fn write_home_delete(
    database: &Database,
    snapshot: &ChannelRetirementSnapshot,
    receipt: &HomeDeleteReceipt,
    session: &mut ClientSession,
) -> Result<(), MutationError> {
    let mut ready = read_ready_runtime(database, snapshot, session)
        .await?
        .ok_or(AllocationError::AuthorityUnavailable)?;
    require_home(snapshot, &ready, receipt)?;
    if ready
        .home_delete
        .as_ref()
        .is_some_and(|previous| previous != receipt)
    {
        return Err(AllocationError::IdentityMismatch.into());
    }
    ready.home_delete = Some(receipt.clone());
    write(database, snapshot, ready, session).await
}

async fn write(
    database: &Database,
    snapshot: &ChannelRetirementSnapshot,
    ready: ReadyRuntimeJournal,
    session: &mut ClientSession,
) -> Result<(), MutationError> {
    let effects = Effects {
        runtime: ready.runtime_finality,
        home_delete: ready.home_delete,
    };
    majority_collection::<Document>(database, "channel_provider_teardown_receipts")
        .update_one(
            doc! {"_id": &snapshot.allocation.id},
            doc! {"$set": {"record": record_bson(&effects).map_err(|_| AllocationError::Corrupt)?}},
        )
        .upsert(true)
        .session(session)
        .await?;
    Ok(())
}
