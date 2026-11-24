use layer2_core::sandbox::{
    apply_retained_reopen, RetainedReopenAccepted, RetainedReopenPath, RetainedReopenReceipt,
    RetainedReopenRequest, Sandbox, SandboxEvent, SandboxStatus, SandboxStoreError,
};
use mongodb::{
    bson::{doc, Document, Timestamp},
    options::ReadConcern,
    ClientSession,
};
use sha2::{Digest, Sha256};

use super::{domain, history, unavailable, ApplyError, MongoSandboxStore, FIELD};
use crate::store::{
    agent_log_integrity::IntegrityFailurePolicy,
    agent_log_segments::transaction_scope::TransactionScope,
    sandbox_events::{append_with_receipt, reconcile_event_with_snapshot_session},
    sandbox_mongo::{record_bson, signed, DOCUMENT_SCHEMA_VERSION},
    sandboxes::{read_sandbox, sandbox_replacement_fields},
};

pub(super) enum Applied {
    Replay(RetainedReopenAccepted),
    Commit {
        receipt: Box<RetainedReopenReceipt>,
        event: SandboxEvent,
        repeat: bool,
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
    validate_receipt(&receipt, &parent)?;
    Ok(Some(receipt))
}

pub(super) fn validate_receipt(
    receipt: &RetainedReopenReceipt,
    parent: &Sandbox,
) -> Result<(), SandboxStoreError> {
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
    Ok(())
}

pub(super) async fn apply_scoped(
    store: &MongoSandboxStore,
    request: &RetainedReopenRequest,
    mut scope: TransactionScope<'_>,
) -> Result<Applied, ApplyError> {
    let id = &request.snapshot.retained.sandbox.id;
    let row = scope
        .find_one(&store.sandboxes, doc! {"_id":id.as_str()})
        .await
        .map_err(ApplyError::Driver)?
        .ok_or(ApplyError::Domain(SandboxStoreError::NotFound))?;
    let expected = &request.snapshot.retained.sandbox;
    let parent = read_sandbox(&row).map_err(ApplyError::Domain)?;
    if parent.owner != expected.owner {
        return Err(ApplyError::Domain(SandboxStoreError::NotFound));
    }
    let previous = scope
        .retained_reopen_history(store, &row, &request.key_sha256)
        .await?;
    if let Some(receipt) = previous.replay {
        if receipt.expected_revision != expected.revision {
            return Err(ApplyError::Domain(SandboxStoreError::Conflict));
        }
        return Ok(Applied::Replay(receipt.accepted));
    }
    let repeat = previous.latest_revision.is_some();
    if previous.latest_revision.is_some_and(|latest| {
        !request.repeat_authorized
            || parent.status != SandboxStatus::Failed
            || parent.revision <= latest
    }) || parent.revision != expected.revision
    {
        return Err(ApplyError::Domain(SandboxStoreError::Conflict));
    }
    let (current, last_seq) = scope
        .retained_reopen_view(
            store,
            id,
            RetainedReopenPath::Idle,
            IntegrityFailurePolicy::Quarantine,
        )
        .await?;
    let session = scope.release();
    let (next, receipt, event) =
        apply_retained_reopen(&current, request).map_err(ApplyError::Domain)?;
    for agent in &current.retained.agents {
        crate::store::agent_capacity_recovery::source::fence_agent(
            store,
            agent,
            "sandbox_recovery_fence",
            session,
        )
        .await?;
    }
    fence_membership(store, &current, session).await?;
    let mut fields = sandbox_replacement_fields(&next).map_err(ApplyError::Domain)?;
    let mut filter = doc! {
        "_id": expected.id.as_str(), "owner": expected.owner.as_str(),
        "document_schema_version": DOCUMENT_SCHEMA_VERSION, "revision": number(expected.revision)?,
        "last_seq": number(last_seq)?,
        "channel_reuse_cleanup_execution": { "$exists": false },
        "channel_retirement_finalization": { "$exists": false },
    };
    let event_receipt = if repeat {
        let event_seq = last_seq
            .checked_add(1)
            .ok_or(ApplyError::Domain(SandboxStoreError::SequenceConflict))?;
        Some(history::Receipt::Idle(receipt.clone()).document(event_seq)?)
    } else {
        fields.insert(
            FIELD,
            record_bson(&receipt).map_err(|_| domain("recovery receipt cannot be encoded"))?,
        );
        filter.insert(FIELD, doc! { "$exists": false });
        None
    };
    let written = store
        .sandboxes
        .update_one(
            crate::store::work_schema::query(filter),
            crate::store::work_lease_update::replacement(fields),
        )
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?;
    require_one(written.matched_count)?;
    crate::store::sandbox_usage::save::record_saved(store, session, &next, expected.revision)
        .await?;
    let event = append_with_receipt(
        store,
        session,
        &expected.id,
        request.requested_at.clone(),
        event,
        event_receipt,
    )
    .await?;
    Ok(Applied::Commit {
        receipt: Box::new(receipt),
        event,
        repeat,
    })
}

pub(super) async fn fence_membership(
    store: &MongoSandboxStore,
    current: &layer2_core::sandbox::RetainedReopenSnapshot,
    session: &mut ClientSession,
) -> Result<(), ApplyError> {
    let id = &current.retained.sandbox.id;
    for workspace in &current.retained.workspaces {
        let written = store.workspaces.update_one(crate::store::work_schema::query(doc! {
            "_id": workspace.id.as_str(), "sandbox_id": id.as_str(),
            "document_schema_version": DOCUMENT_SCHEMA_VERSION, "revision": number(workspace.revision)?,
        }), doc! { "$inc": { "sandbox_recovery_fence": 1_i64 } })
            .session(&mut *session).await.map_err(ApplyError::Driver)?;
        require_one(written.matched_count)?;
    }
    for tree in &current.worktrees {
        let written = store.worktrees.update_one(doc! {
            "_id": tree.id.as_str(), "sandbox_id": id.as_str(),
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
    Ok(())
}

pub(super) async fn reconcile(
    store: &MongoSandboxStore,
    receipt: &RetainedReopenReceipt,
    event: &SandboxEvent,
    time: Timestamp,
    repeat: bool,
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
    let receipt_matches = if repeat {
        history::matches_event_receipt(
            store,
            event,
            history::Receipt::Idle(receipt.clone()),
            &mut session,
        )
        .await?
    } else {
        read_receipt(&row)?.as_ref() == Some(receipt)
    };
    if !receipt_matches
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
