use mongodb::{
    bson::{doc, Document},
    options::ReturnDocument,
    ClientSession,
};

use super::super::{store::unavailable, validation, *};
use super::{active_writer, model::*, records, selected};

pub(super) async fn execute(
    store: &AuthorityStore,
    command: &CommandRecord,
    session: &mut ClientSession,
) -> Result<NativePollExecution> {
    if let Some(receipt) = records::receipt(store, command, session).await? {
        return Ok(NativePollExecution::Replay(receipt));
    }
    let head = store.head_in(&command.writer.deployment, session).await?;
    active_writer(&head, &command.writer)?;
    if selected(&head, command.lane)? != command.selected_target {
        return Err(AuthorityError::Conflict);
    }
    let previous = records::tip(store, &command.writer, command.lane, session).await?;
    let sequence = previous
        .as_ref()
        .map_or(Ok(1), |r| validation::increment(r.sequence()))?;
    if sequence != command.sequence
        || previous.as_ref().map(|r| r.sha256()) != command.previous_receipt_sha256.as_deref()
    {
        return Err(AuthorityError::Conflict);
    }

    let mut next = head.clone();
    next.admission_sequence = validation::increment(head.admission_sequence)?;
    // Even Empty writes this same head, serializing with freeze. Epoch, control
    // revision and active_effects are unchanged; no logical permit is settled.
    store.write_head(&head, &next, false, session).await?;
    let target = store.native_targets.find_one_and_update(
        selector(command),
        vec![doc! {"$set": {"lease": {"$literal": lease(command)},
            "lease_until": {"$dateAdd": {"startDate": "$$NOW", "unit": "second", "amount": 60_i64}}}}],
    ).sort(doc! {"reconcile_after": 1, "_id": 1})
        .return_document(ReturnDocument::After).session(&mut *session)
        .await.map_err(unavailable)?;
    let outcome = match target {
        Some(target) => {
            // Reject malformed persisted fields before committing the lease.
            validate_target(command, &target)?;
            NativePollOutcome::Claimed(Box::new(target))
        }
        None => NativePollOutcome::Empty,
    };
    let mut receipt = ReceiptRecord {
        key: command.key.clone(),
        kind: "native_target_poll_receipt".into(),
        schema_version: 1,
        command: command.clone(),
        outcome,
        admission_sequence: next.admission_sequence,
        control_revision: head.control_revision,
        receipt_sha256: String::new(),
    };
    receipt.receipt_sha256 = receipt.digest()?;
    receipt.validate()?;
    let receipt = NativePollReceipt(receipt);
    records::append(store, previous.as_ref(), &receipt, session).await?;
    Ok(NativePollExecution::Fresh(receipt))
}

/// Entirely fixed query. Non-null leases are never reclaimed on elapsed time.
/// Server time is a scheduling condition, never predecessor-effect finality.
fn selector(command: &CommandRecord) -> Document {
    let mut filter = doc! {
        "plan.deployment": &command.writer.deployment,
        "document_schema_version": {"$in": [3_i64, 4_i64]},
        "retired": {"$ne": true},
        "lease": {"$type": "null"},
        "lease_until": {"$type": "date"},
        "reconcile_after": {"$type": "date"},
        "$expr": {"$and": [
            {"$lte": ["$lease_until", "$$NOW"]},
            {"$lte": ["$reconcile_after", "$$NOW"]},
        ]},
    };
    if let Some(target) = &command.selected_target {
        filter.insert("_id", &target.target_id);
        filter.insert("revision", target.target_revision);
        filter.insert("retiring", doc! {"$ne": true});
        filter.insert("machine", command.lane == NativePollLane::StartupMachine);
        filter.insert("binding.uid", &target.pool_uid);
        filter.insert("binding.template_uid", &target.template_uid);
    }
    filter
}
