use layer2_core::bridge::allocation::{
    AllocationError, ChannelSandboxAdmission, ChannelSandboxReservation,
};
use layer2_core::sandbox::{SandboxEvent, SandboxEventKind};
use mongodb::{
    bson::{doc, Bson},
    ClientSession,
};

use crate::store::{
    resource_access::{policy_document, read_inventory_policy},
    sandbox_events::{event_document, read_event},
    sandbox_mongo::{record_bson, DOCUMENT_SCHEMA_VERSION},
    sandboxes::{read_sandbox, sandbox_document},
};

use super::{
    super::{
        records::{decode_head, decode_record},
        MongoBridgeAllocationStore, MutationError,
    },
    Resources,
};

pub(super) async fn apply(
    store: &MongoBridgeAllocationStore,
    resources: &Resources,
    request: &ChannelSandboxReservation,
    session: &mut ClientSession,
) -> Result<ChannelSandboxAdmission, MutationError> {
    request.require_current()?;
    store.fence(request.authority(), session).await?;
    let expected = request.previous();
    let head_id = expected.identity.head_id()?;
    let head = store
        .heads
        .find_one(doc! {"_id":&head_id})
        .session(&mut *session)
        .await?
        .ok_or(AllocationError::Conflict)?;
    if decode_head(&head, &expected.identity)? != expected.id {
        return Err(AllocationError::Conflict.into());
    }
    let stored = store
        .records
        .find_one(doc! {"_id":&expected.id})
        .session(&mut *session)
        .await?
        .ok_or(AllocationError::Conflict)?;
    let current = decode_record(&stored)?;
    if &current != expected && &current != request.allocation() {
        return Err(AllocationError::Conflict.into());
    }
    request.authority().require_record(&current)?;
    let changed = store
        .heads
        .update_one(
            doc! {"_id":&head_id,"allocation_id":&expected.id},
            doc! {"$inc":{"sandbox_admission_fence":1_i64}},
        )
        .session(&mut *session)
        .await?;
    if changed.matched_count != 1 {
        return Err(AllocationError::Conflict.into());
    }
    // This is the same owner serialization document as ordinary admission.
    resources.admissions.update_one(doc! {"_id":request.sandbox().owner.as_str()}, doc! {"$inc":{"fence":1_i64},"$setOnInsert":{"document_schema_version":DOCUMENT_SCHEMA_VERSION}}).upsert(true).session(&mut *session).await?;
    let sidecar = record_bson(request.sidecar()).map_err(|_| AllocationError::Corrupt)?;
    let existing = resources
        .sandboxes
        .find_one(doc! {"_id":request.sandbox().id.as_str()})
        .session(&mut *session)
        .await?;
    let created = if let Some(document) = existing {
        let sandbox = read_sandbox(&document).map_err(|_| AllocationError::Corrupt)?;
        if &sandbox != request.sandbox()
            || document.get("channel_creation") != Some(&sidecar)
            || stored.get("sandbox_admission") != Some(&sidecar)
            || !current.effect_started
        {
            return Err(AllocationError::Conflict.into());
        }
        request.sidecar().require_held(&current, &sandbox)?;
        let policy = resources
            .policies
            .find_one(doc! {"_id":sandbox.id.as_str()})
            .session(&mut *session)
            .await?
            .ok_or(AllocationError::Corrupt)?;
        if read_inventory_policy(&policy).map_err(|_| AllocationError::Corrupt)?
            != *request.policy()
        {
            return Err(AllocationError::Conflict.into());
        }
        let event = resources
            .events
            .find_one(doc! {"sandbox_id":sandbox.id.as_str(),"seq":1_i64})
            .session(&mut *session)
            .await?
            .ok_or(AllocationError::Corrupt)?;
        if read_event(&event).map_err(|_| AllocationError::Corrupt)? != creation_event(request) {
            return Err(AllocationError::Corrupt.into());
        }
        resources
            .sandboxes
            .update_one(
                doc! {"_id":sandbox.id.as_str(),"revision":1_i64},
                doc! {"$inc":{"channel_admission_fence":1_i64}},
            )
            .session(&mut *session)
            .await?;
        resources
            .policies
            .update_one(
                doc! {"_id":sandbox.id.as_str(),"revision":1_i64},
                doc! {"$inc":{"channel_admission_fence":1_i64}},
            )
            .session(&mut *session)
            .await?;
        false
    } else {
        if request.requires_existing()
            || current.effect_started
            || stored.contains_key("sandbox_admission")
        {
            return Err(AllocationError::Corrupt.into());
        }
        let live = resources
            .sandboxes
            .count_documents(
                doc! {"owner":request.sandbox().owner.as_str(),"retired_at":Bson::Null},
            )
            .session(&mut *session)
            .await?;
        if live >= u64::from(request.fresh_channel_live_limit()) {
            return Err(AllocationError::Capacity.into());
        }
        let mut document =
            sandbox_document(request.sandbox(), 1).map_err(|_| AllocationError::Corrupt)?;
        document.insert("channel_creation", sidecar.clone());
        resources
            .sandboxes
            .insert_one(document)
            .session(&mut *session)
            .await?;
        resources
            .policies
            .insert_one(policy_document(request.policy()).map_err(|_| AllocationError::Corrupt)?)
            .session(&mut *session)
            .await?;
        resources
            .events
            .insert_one(
                event_document(&creation_event(request)).map_err(|_| AllocationError::Corrupt)?,
            )
            .session(&mut *session)
            .await?;
        true
    };
    if &current != request.allocation() {
        store
            .replace(&current, request.allocation(), session)
            .await?;
    }
    let changed = store
        .records
        .update_one(
            doc! {"_id":&expected.id,"revision":request.allocation().revision as i64},
            doc! {"$set":{"sandbox_admission":sidecar},"$inc":{"sandbox_admission_fence":1_i64}},
        )
        .session(&mut *session)
        .await?;
    if changed.matched_count != 1 {
        return Err(AllocationError::Conflict.into());
    }
    request.require_current()?;
    Ok(ChannelSandboxAdmission {
        sandbox: request.sandbox().clone(),
        allocation: request.allocation().clone(),
        sidecar: request.sidecar().clone(),
        created,
    })
}

fn creation_event(request: &ChannelSandboxReservation) -> SandboxEvent {
    SandboxEvent {
        sandbox_id: request.sandbox().id.clone(),
        seq: 1,
        appended_at: request.sandbox().created_at.clone(),
        kind: SandboxEventKind::Created,
    }
}
