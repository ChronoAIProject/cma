use layer2_core::sandbox::{
    apply_retained_reopen, RetainedReopenAccepted, RetainedReopenReceipt, RetainedReopenRequest,
    SandboxEvent, SandboxStoreError,
};
use mongodb::{
    bson::{doc, Document, Timestamp},
    options::ReadConcern,
    ClientSession,
};
use sha2::{Digest, Sha256};

use super::{domain, load, unavailable, ApplyError, MongoSandboxStore, FIELD};
use crate::store::{
    sandbox_events::{append_in_transaction, reconcile_event_with_snapshot_session},
    sandbox_mongo::{record_bson, signed, DOCUMENT_SCHEMA_VERSION},
    sandboxes::{read_sandbox, sandbox_replacement_fields},
};

pub(super) enum Applied {
    Replay(RetainedReopenAccepted),
    Commit {
        receipt: Box<RetainedReopenReceipt>,
        event: SandboxEvent,
    },
}

pub(super) fn read_receipt(
    row: &Document,
) -> Result<Option<RetainedReopenReceipt>, SandboxStoreError> {
    let parent = read_sandbox(row)?;
    let Some(value) = row.get(FIELD) else {
        return Ok(None);
    };
    let receipt: RetainedReopenReceipt = mongodb::bson::deserialize_from_bson(value.clone())
        .map_err(|_| SandboxStoreError::Corrupt("retained reopening receipt is invalid".into()))?;
    if receipt.version != 1
        || receipt.sandbox_id != parent.id
        || receipt.owner != parent.owner
        || receipt.expected_revision.checked_add(1) != Some(receipt.accepted.revision)
        || receipt.accepted.revision > parent.revision
        || [
            &receipt.key_sha256,
            &receipt.snapshot_sha256,
            &receipt.result_sha256,
        ]
        .iter()
        .any(|hash| {
            hash.len() != 64
                || !hash
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
        || layer2_core::seconds_from_rfc3339(&receipt.accepted.accepted_at).is_none()
    {
        return Err(SandboxStoreError::Corrupt(
            "retained reopening receipt authority is invalid".into(),
        ));
    }
    Ok(Some(receipt))
}

pub(super) async fn apply(
    store: &MongoSandboxStore,
    request: &RetainedReopenRequest,
    session: &mut ClientSession,
) -> Result<Applied, ApplyError> {
    let expected = &request.snapshot.retained.sandbox;
    let row = store
        .sandboxes
        .find_one(doc! { "_id": expected.id.as_str() })
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?
        .ok_or(ApplyError::Domain(SandboxStoreError::NotFound))?;
    if read_sandbox(&row).map_err(ApplyError::Domain)?.owner != expected.owner {
        return Err(ApplyError::Domain(SandboxStoreError::NotFound));
    }
    if let Some(receipt) = read_receipt(&row).map_err(ApplyError::Domain)? {
        if receipt.key_sha256 != request.key_sha256
            || receipt.expected_revision != expected.revision
        {
            return Err(ApplyError::Domain(SandboxStoreError::Conflict));
        }
        return Ok(Applied::Replay(receipt.accepted));
    }
    let (current, last_seq) = load::snapshot(store, &expected.id, session).await?;
    let (next, receipt, event) =
        apply_retained_reopen(&current, request).map_err(ApplyError::Domain)?;
    for agent in &current.retained.agents {
        let written = store.agents.update_one(doc! {
            "_id": agent.id.as_str(), "sandbox_id": expected.id.as_str(),
            "document_schema_version": DOCUMENT_SCHEMA_VERSION,
            "revision": number(agent.revision)?, "last_seq": number(agent.conversation.last_seq)?,
        }, doc! { "$inc": { "sandbox_recovery_fence": 1_i64 } })
            .session(&mut *session).await.map_err(ApplyError::Driver)?;
        require_one(written.matched_count)?;
    }
    for workspace in &current.retained.workspaces {
        let written = store.workspaces.update_one(doc! {
            "_id": workspace.id.as_str(), "sandbox_id": expected.id.as_str(),
            "document_schema_version": DOCUMENT_SCHEMA_VERSION, "revision": number(workspace.revision)?,
        }, doc! { "$inc": { "sandbox_recovery_fence": 1_i64 } })
            .session(&mut *session).await.map_err(ApplyError::Driver)?;
        require_one(written.matched_count)?;
    }
    for tree in &current.worktrees {
        let written = store.worktrees.update_one(doc! {
            "_id": tree.id.as_str(), "sandbox_id": expected.id.as_str(),
            "document_schema_version": DOCUMENT_SCHEMA_VERSION, "revision": number(tree.revision)?,
        }, doc! { "$inc": { "sandbox_recovery_fence": 1_i64 } })
            .session(&mut *session).await.map_err(ApplyError::Driver)?;
        require_one(written.matched_count)?;
    }
    for (id, lane) in &current.command_lanes {
        if lane.last_order == 0 {
            continue;
        }
        let written = store
            .agent_command_orders
            .update_one(
                doc! {
                    "_id": id.as_str(), "last_order": number(lane.last_order)?,
                },
                doc! { "$inc": { "sandbox_recovery_fence": 1_i64 } },
            )
            .session(&mut *session)
            .await
            .map_err(ApplyError::Driver)?;
        require_one(written.matched_count)?;
    }
    let mut fields = sandbox_replacement_fields(&next).map_err(ApplyError::Domain)?;
    fields.insert(
        FIELD,
        record_bson(&receipt).map_err(|_| domain("recovery receipt cannot be encoded"))?,
    );
    let written = store.sandboxes.update_one(doc! {
        "_id": expected.id.as_str(), "owner": expected.owner.as_str(),
        "document_schema_version": DOCUMENT_SCHEMA_VERSION, "revision": number(expected.revision)?,
        "last_seq": number(last_seq)?, FIELD: { "$exists": false },
        "channel_reuse_cleanup_execution": { "$exists": false },
        "channel_retirement_finalization": { "$exists": false },
    }, doc! { "$set": fields }).session(&mut *session).await.map_err(ApplyError::Driver)?;
    require_one(written.matched_count)?;
    crate::store::sandbox_usage::save::record_saved(store, session, &next, expected.revision)
        .await?;
    let event = append_in_transaction(
        store,
        session,
        &expected.id,
        request.requested_at.clone(),
        event,
    )
    .await?;
    Ok(Applied::Commit {
        receipt: Box::new(receipt),
        event,
    })
}

pub(super) async fn reconcile(
    store: &MongoSandboxStore,
    receipt: &RetainedReopenReceipt,
    event: &SandboxEvent,
    time: Timestamp,
) -> Result<(), SandboxStoreError> {
    let mut session = store
        .client
        .start_session()
        .snapshot(true)
        .snapshot_time(time)
        .await
        .map_err(unavailable)?;
    let row = store
        .sandboxes
        .find_one(doc! { "_id": receipt.sandbox_id.as_str() })
        .read_concern(ReadConcern::snapshot())
        .session(&mut session)
        .await
        .map_err(unavailable)?
        .ok_or_else(|| unavailable("recovery receipt is unresolved"))?;
    let parent = read_sandbox(&row)?;
    let bytes = serde_json::to_vec(&parent).map_err(unavailable)?;
    if read_receipt(&row)?.as_ref() != Some(receipt)
        || format!("{:x}", Sha256::digest(bytes)) != receipt.result_sha256
        || reconcile_event_with_snapshot_session(store, event, &mut session)
            .await?
            .is_none()
    {
        return Err(unavailable("recovery receipt or event is unresolved"));
    }
    Ok(())
}

fn number(value: u64) -> Result<i64, ApplyError> {
    signed(value, "retained reopening fence")
        .map_err(|error| ApplyError::Domain(SandboxStoreError::Corrupt(error)))
}
fn require_one(count: u64) -> Result<(), ApplyError> {
    if count == 1 {
        Ok(())
    } else {
        Err(ApplyError::Domain(SandboxStoreError::Conflict))
    }
}
