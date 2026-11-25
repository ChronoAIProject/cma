//! One-time suspended recovery settlement and predecessor exclusion.
use crate::store::transaction_terminal::GuardedTerminal as _;
use layer2_core::{
    sandbox::{
        apply_suspended_reopen, store::SandboxRuntimeLossRequest, RetainedReopenReplay,
        RetainedSuspendedReopenReceipt, RetainedSuspendedReopenRequest, Sandbox, SandboxEvent,
        SandboxInterruptionCause, SandboxLifecycleOperation, SandboxReviveCause, SandboxStatus,
    },
    seconds_from_rfc3339,
};
use mongodb::bson::Document;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::*;
use crate::store::{
    sandbox_events::{append_with_receipt, reconcile_event_with_snapshot_session},
    sandbox_mongo::{record_bson, signed, DOCUMENT_SCHEMA_VERSION},
    sandbox_reconciliation::settle_reopen_agents,
    sandboxes::{read_sandbox, sandbox_replacement_fields},
};

pub(super) const SUSPENDED_FIELD: &str = "retained_suspended_reopen_receipt_v2";

#[cfg(test)]
#[path = "suspended_tests.rs"]
mod tests;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Exclusion {
    kind: String,
    version: u32,
    receipt_sha256: String,
}

fn corrupt() -> SandboxStoreError {
    SandboxStoreError::Corrupt("suspended retained opening receipt is invalid".into())
}

pub(super) fn require_personal(
    policy: Option<&Document>,
    parent: &Sandbox,
) -> Result<(), SandboxStoreError> {
    if let Some(policy) = policy {
        let personal = crate::store::resource_access::read_inventory_policy(policy)
            .and_then(|policy| policy.personal_owner_of(parent))
            .map_err(|_| {
                SandboxStoreError::Corrupt("invalid retained recovery ownership".into())
            })?;
        if !personal {
            return Err(SandboxStoreError::NotFound);
        }
    }
    Ok(())
}

fn receipt_digest(receipt: &RetainedSuspendedReopenReceipt) -> Result<String, SandboxStoreError> {
    let bytes = serde_json::to_vec(receipt).map_err(|_| corrupt())?;
    let mut hash = Sha256::new();
    hash.update(b"cma.retained-suspended-reopen.receipt.v2\0");
    hash.update(bytes);
    Ok(format!("{:x}", hash.finalize()))
}

pub(super) fn pair(
    row: &Document,
) -> Result<Option<RetainedSuspendedReopenReceipt>, SandboxStoreError> {
    let Some(value) = row.get(SUSPENDED_FIELD) else {
        // A lone exclusion is unreadable as v1 and must also fail closed here.
        transaction::read_receipt(row)?;
        return Ok(None);
    };
    let receipt: RetainedSuspendedReopenReceipt =
        mongodb::bson::deserialize_from_bson(value.clone()).map_err(|_| corrupt())?;
    let marker: Exclusion =
        mongodb::bson::deserialize_from_bson(row.get(FIELD).cloned().ok_or_else(corrupt)?)
            .map_err(|_| corrupt())?;
    if marker.kind != "suspended_native_reopen"
        || marker.version != 2
        || marker.receipt_sha256 != receipt_digest(&receipt)?
    {
        return Err(corrupt());
    }
    let parent = read_sandbox(row)?;
    let last_seq =
        u64::try_from(row.get_i64("last_seq").map_err(|_| corrupt())?).map_err(|_| corrupt())?;
    validate_receipt(&receipt, &parent, last_seq)?;
    Ok(Some(receipt))
}

pub(super) fn validate_receipt(
    receipt: &RetainedSuspendedReopenReceipt,
    parent: &Sandbox,
    last_seq: u64,
) -> Result<(), SandboxStoreError> {
    let accepted = &receipt.accepted;
    if receipt.version != 2
        || receipt.sandbox_id != parent.id
        || receipt.owner != parent.owner
        || receipt.expected_revision == 0
        || receipt.expected_revision.checked_add(1) != Some(accepted.revision)
        || accepted.revision > parent.revision
        || receipt.event_seq == 0
        || receipt.event_seq > last_seq
        || receipt.authority_generation == 0
        || receipt.authority_generation >= parent.lifecycle.next_authority
        || accepted.operation_id
            != format!(
                "sandbox-lifecycle-v1:{}:{}",
                parent.id, receipt.authority_generation
            )
        || seconds_from_rfc3339(&accepted.accepted_at).is_none()
        || [
            receipt.key_sha256.as_str(),
            receipt.snapshot_sha256.as_str(),
            receipt.settled_sha256.as_str(),
        ]
        .iter()
        .any(|hash| {
            hash.len() != 64
                || !hash
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
        || parent
            .lifecycle
            .operation
            .as_ref()
            .is_some_and(|operation| {
                let authority = operation.authority();
                authority.generation == receipt.authority_generation
                    && (!matches!(
                        operation,
                        SandboxLifecycleOperation::Revive {
                            cause: SandboxReviveCause::RuntimeLost,
                            ..
                        }
                    ) || authority.operation_id != accepted.operation_id
                        || authority.requested_at != accepted.accepted_at)
            })
    {
        return Err(corrupt());
    }
    Ok(())
}

pub(in crate::store) async fn replay(
    store: &MongoSandboxStore,
    id: &SandboxId,
) -> Result<Option<RetainedReopenReplay>, SandboxStoreError> {
    let mut session = store
        .client
        .start_session()
        .snapshot(true)
        .await
        .map_err(unavailable)?;
    let row = store
        .sandboxes
        .find_one(doc! {"_id":id.as_str()})
        .read_concern(ReadConcern::snapshot())
        .session(&mut session)
        .await
        .map_err(unavailable)?
        .ok_or(SandboxStoreError::NotFound)?;
    if row.contains_key(SUSPENDED_FIELD) {
        let policy = store
            .resource_policies
            .find_one(doc! {"_id":id.as_str()})
            .read_concern(ReadConcern::snapshot())
            .session(&mut session)
            .await
            .map_err(unavailable)?;
        require_personal(policy.as_ref(), &read_sandbox(&row)?)?;
        return Ok(pair(&row)?.map(Into::into));
    }
    Ok(transaction::read_receipt(&row)?.map(Into::into))
}

enum Applied {
    Replay(RetainedReopenAccepted),
    Commit(Box<RetainedSuspendedReopenReceipt>, SandboxEvent, bool),
}

pub(in crate::store) async fn admit_suspended(
    store: &MongoSandboxStore,
    request: RetainedSuspendedReopenRequest,
) -> Result<RetainedReopenAccepted, SandboxStoreError> {
    request.proof.validate(&request.snapshot)?;
    for _ in 0..TRANSACTION_ATTEMPTS {
        let mut session = store.client.start_session().await.map_err(unavailable)?;
        let scope = match TransactionScope::start(&mut session).await {
            Ok(scope) => scope,
            Err(error) if is_transient(&error) => continue,
            Err(error) => return Err(unavailable(error)),
        };
        let (receipt, event, repeat) = match apply(store, &request, scope).await {
            Ok(Applied::Commit(receipt, event, repeat)) => (receipt, event, repeat),
            Ok(Applied::Replay(accepted)) => {
                let _ = session.abort_guarded().await;
                return Ok(accepted);
            }
            Err(ApplyError::Driver(error)) if is_transient(&error) => {
                let _ = session.abort_guarded().await;
                continue;
            }
            Err(error) => {
                let _ = session.abort_guarded().await;
                return Err(error.domain());
            }
        };
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => return Ok(receipt.accepted),
            CommitOutcome::Retry(_) => continue,
            CommitOutcome::Unknown(error) => {
                let time = session.operation_time().ok_or_else(|| unavailable(error))?;
                let store = store.clone();
                return wait_for_reconciliation(
                    "suspended retained reopening commit",
                    async move {
                        reconcile(&store, &receipt, &event, time, repeat).await?;
                        Ok(receipt.accepted)
                    },
                )
                .await
                .map_err(unavailable)?;
            }
            CommitOutcome::Failed(error) => return Err(unavailable(error)),
        }
    }
    Err(unavailable(
        "suspended retained reopening transaction retries exhausted",
    ))
}

async fn apply(
    store: &MongoSandboxStore,
    request: &RetainedSuspendedReopenRequest,
    mut scope: TransactionScope<'_>,
) -> Result<Applied, ApplyError> {
    let parent = &request.snapshot.retained.sandbox;
    let row = scope
        .find_one(&store.sandboxes, doc! {"_id":parent.id.as_str()})
        .await
        .map_err(ApplyError::Driver)?
        .ok_or(ApplyError::Domain(SandboxStoreError::NotFound))?;
    let durable = read_sandbox(&row).map_err(ApplyError::Domain)?;
    if durable.owner != parent.owner {
        return Err(ApplyError::Domain(SandboxStoreError::NotFound));
    }
    let previous = scope
        .retained_reopen_history(store, &row, &request.key_sha256)
        .await?;
    if let Some(receipt) = previous.replay {
        if receipt.expected_revision != parent.revision {
            return Err(ApplyError::Domain(SandboxStoreError::Conflict));
        }
        return Ok(Applied::Replay(receipt.accepted));
    }
    let repeat = previous.latest_revision.is_some();
    if previous.latest_revision.is_some_and(|latest| {
        !request.repeat_authorized
            || durable.status != SandboxStatus::Failed
            || durable.revision <= latest
    }) || durable.revision != parent.revision
    {
        return Err(ApplyError::Domain(SandboxStoreError::Conflict));
    }
    let (current, last_seq) = scope
        .retained_reopen_view(
            store,
            &parent.id,
            RetainedReopenPath::Suspended,
            IntegrityFailurePolicy::Quarantine,
        )
        .await?;
    let (settlement, receipt) =
        apply_suspended_reopen(&current, request).map_err(ApplyError::Domain)?;
    let loss = SandboxRuntimeLossRequest {
        sandbox_id: parent.id.clone(),
        expected_revision: parent.revision,
        expected_runtime: parent.runtime.clone(),
        retain_runtime: true,
        interruption_cause: SandboxInterruptionCause::RuntimeUnavailable,
        observed_at: request.requested_at.clone(),
        // A suspended reopen is not a native retirement.
        native_retirement_finality: false,
    };
    let session = settle_reopen_agents(
        store,
        &current.retained.agents,
        &settlement.agents,
        &loss,
        scope,
    )
    .await?;
    transaction::fence_membership(store, &current, session).await?;
    let mut next = sandbox_replacement_fields(&settlement.sandbox).map_err(ApplyError::Domain)?;
    let number = |value| {
        signed(value, "suspended reopening fence")
            .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))
    };
    let mut filter = doc! {
        "_id": parent.id.as_str(), "owner": parent.owner.as_str(),
        "document_schema_version": DOCUMENT_SCHEMA_VERSION, "revision": number(parent.revision)?,
        "last_seq": number(last_seq)?,
        "channel_reuse_cleanup_execution": { "$exists": false },
        "channel_retirement_finalization": { "$exists": false },
    };
    let event_receipt = if repeat {
        Some(history::Receipt::Suspended(receipt.clone()).document(receipt.event_seq)?)
    } else {
        let marker = Exclusion {
            kind: "suspended_native_reopen".into(),
            version: 2,
            receipt_sha256: receipt_digest(&receipt).map_err(ApplyError::Domain)?,
        };
        next.insert(
            FIELD,
            record_bson(&marker)
                .map_err(|_| domain("suspended reopening marker cannot be encoded"))?,
        );
        next.insert(
            SUSPENDED_FIELD,
            record_bson(&receipt)
                .map_err(|_| domain("suspended reopening receipt cannot be encoded"))?,
        );
        filter.insert(FIELD, doc! { "$exists": false });
        filter.insert(SUSPENDED_FIELD, doc! { "$exists": false });
        None
    };
    let written = store
        .sandboxes
        .update_one(
            crate::store::work_schema::query(filter),
            crate::store::work_lease_update::replacement(next),
        )
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?;
    if written.matched_count != 1 {
        return Err(ApplyError::Domain(SandboxStoreError::Conflict));
    }
    crate::store::sandbox_usage::save::record_saved(
        store,
        session,
        &settlement.sandbox,
        parent.revision,
    )
    .await?;
    let [kind] = settlement
        .events
        .try_into()
        .map_err(|_| domain("suspended reopening event is invalid"))?;
    let event = append_with_receipt(
        store,
        session,
        &parent.id,
        request.requested_at.clone(),
        kind,
        event_receipt,
    )
    .await?;
    if event.seq != receipt.event_seq {
        return Err(ApplyError::Domain(SandboxStoreError::SequenceConflict));
    }
    Ok(Applied::Commit(Box::new(receipt), event, repeat))
}

async fn reconcile(
    store: &MongoSandboxStore,
    receipt: &RetainedSuspendedReopenReceipt,
    event: &SandboxEvent,
    time: mongodb::bson::Timestamp,
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
        .find_one(doc! {"_id":receipt.sandbox_id.as_str()})
        .read_concern(ReadConcern::snapshot())
        .session(&mut session)
        .await
        .map_err(unavailable)?
        .ok_or_else(|| unavailable("suspended reopening receipt is unresolved"))?;
    let receipt_matches = if repeat {
        history::matches_event_receipt(
            store,
            event,
            history::Receipt::Suspended(receipt.clone()),
            &mut session,
        )
        .await?
    } else {
        pair(&row)?.as_ref() == Some(receipt)
    };
    if !receipt_matches
        || reconcile_event_with_snapshot_session(store, event, &mut session)
            .await?
            .is_none()
    {
        return Err(unavailable(
            "suspended reopening receipt pair or event is unresolved",
        ));
    }
    Ok(())
}
