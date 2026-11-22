use super::worker::*;
use super::{worker_guard as guard, worker_records as records};

pub(super) async fn stage(
    store: &MongoErasureJournal,
    request: &JournalRequest,
    claim: Option<&JournalClaim>,
    token: &str,
    session: &mut ClientSession,
) -> Attempt<(JournalOutcome, bool)> {
    let (old, document, at) = guard::parent(store, &request.binding().deletion_id, session).await?;
    let stored = store
        .checkpoints
        .find_one(doc! {"_id":old.id.as_str()})
        .session(&mut *session)
        .await?
        .map(records::checkpoint)
        .transpose()?;
    if let Some(receipt) = history(store, request, stored.as_ref(), &old, session).await? {
        return Ok((
            JournalOutcome::Recorded(Box::new(JournalRecorded {
                receipt,
                claim: None,
            })),
            false,
        ));
    }
    let enrolled = matches!(request.command(), JournalCommand::Enroll);
    let previous = match (&stored, enrolled) {
        (None, true) if old.step == request.binding().step => None,
        (Some(row), false)
            if row.checkpoint.binding == *request.binding()
                && row.accepted.matches(&old)
                && row.checkpoint.parent_revision == old.revision =>
        {
            Some(&row.checkpoint)
        }
        _ => return Err(JournalError::BindingMismatch.into()),
    };
    let mut next = old.clone();
    let mut events = Vec::new();
    let claim_at = match request.command() {
        JournalCommand::Enroll => {
            next.revision = next.revision.checked_add(1).ok_or(JournalError::Capacity)?;
            None
        }
        JournalCommand::Claim {
            holder,
            lease_seconds,
            ..
        } => {
            if !old.is_due(&at) {
                return Ok((JournalOutcome::Idle, false));
            }
            next.claim(holder, &at, Duration::from_secs(*lease_seconds))
                .map_err(|_| JournalError::StaleClaim)?;
            next.work.lease.as_mut().unwrap().token = token.into();
            events.push(next.current_event(&at, ErasureEventKind::Started));
            Some(at.as_str())
        }
        command => {
            let claim = claim.ok_or(JournalError::StaleClaim)?;
            guard::claim(
                stored.as_ref().ok_or(JournalError::Incompatible)?,
                &old,
                claim,
            )?;
            match command {
                JournalCommand::Prepare { .. } | JournalCommand::Observe { .. } => {
                    next.revision = next.revision.checked_add(1).ok_or(JournalError::Capacity)?;
                }
                JournalCommand::Renew { lease_seconds } => next
                    .renew(
                        claim.lease().fence,
                        &at,
                        Duration::from_secs(*lease_seconds),
                    )
                    .map_err(|_| JournalError::StaleClaim)?,
                JournalCommand::Defer { delay_seconds } => {
                    let at =
                        layer2_core::seconds_from_rfc3339(&at).ok_or(JournalError::Incompatible)?;
                    let until = layer2_core::rfc3339_at(
                        std::time::UNIX_EPOCH
                            + Duration::from_secs(
                                at.checked_add(*delay_seconds)
                                    .ok_or(JournalError::Capacity)?,
                            ),
                    );
                    next.defer(claim.lease().fence, &until)
                        .map_err(|_| JournalError::StaleClaim)?;
                }
                JournalCommand::Block { reason } => {
                    let reason = match reason {
                        JournalBlock::AuthorityUnavailable => {
                            ErasureBlock::OwnerAuthorityUnavailable
                        }
                        JournalBlock::CapacityExceeded => ErasureBlock::CapacityExceeded {
                            class: "erasure_journal".into(),
                            limit: MAX_JOURNAL_BYTES as u64,
                        },
                    };
                    events.push(
                        next.block(claim.lease().fence, &at, reason)
                            .map_err(|_| JournalError::StaleClaim)?,
                    );
                }
                _ => return Err(JournalError::Invalid.into()),
            }
            None
        }
    };
    if let JournalCommand::Observe { plan, .. } = request.command() {
        if previous.and_then(|p| p.latest_plan.as_deref()) != Some(plan) {
            return Err(JournalError::BindingMismatch.into());
        }
        let planned = store
            .operations
            .find_one(doc! {"_id":plan})
            .session(&mut *session)
            .await?
            .ok_or(JournalError::Incompatible)?;
        let planned = records::operation(planned)?;
        if planned.deletion_id != old.id
            || planned.request.binding().contract != request.binding().contract
            || !matches!(planned.request.command(), JournalCommand::Prepare { .. })
            || planned.receipt.checkpoint.binding.inventory_generation
                != request.binding().inventory_generation
        {
            return Err(JournalError::BindingMismatch.into());
        }
    }
    let kind = records::kind(request);
    let sequence = request
        .binding()
        .sequence
        .checked_add(u64::from(kind == Kind::Operation))
        .ok_or(JournalError::Capacity)?;
    let generation = request
        .binding()
        .inventory_generation
        .checked_add(u64::from(matches!(
            request.command(),
            JournalCommand::Prepare { .. }
        )))
        .ok_or(JournalError::Capacity)?;
    let mut checkpoint = JournalCheckpoint {
        binding: JournalBinding {
            sequence,
            inventory_generation: generation,
            ..request.binding().clone()
        },
        parent_revision: next.revision,
        last_operation: request.key().into(),
        latest_plan: previous.and_then(|p| p.latest_plan.clone()),
        last_observation: previous.and_then(|p| p.last_observation.clone()),
        digest: request.key()[4..]
            .to_owned()
            .try_into()
            .map_err(|_| JournalError::Invalid)?,
    };
    match request.command() {
        JournalCommand::Prepare { .. } => {
            checkpoint.latest_plan = Some(request.key().into());
            checkpoint.last_observation = None;
        }
        JournalCommand::Observe { .. } => checkpoint.last_observation = Some(request.key().into()),
        _ => {}
    }
    let mut receipt = JournalReceipt {
        operation: request.key().into(),
        checkpoint: checkpoint.clone(),
        parent_fence: next.work.lease.as_ref().map(|l| l.fence),
        lease_expires_at: next.work.lease.as_ref().map(|l| l.expires_at.clone()),
    };
    receipt.checkpoint.digest = records::receipt_digest(previous.map(|v| &v.digest), &receipt);
    checkpoint = receipt.checkpoint.clone();
    let row = CheckpointRow {
        id: old.id.clone(),
        schema: JOURNAL_VERSION,
        kind: "checkpoint".into(),
        accepted: Accepted::from_parent(&old),
        checkpoint: checkpoint.clone(),
    };
    let operation = OperationRow {
        id: request.key().into(),
        schema: JOURNAL_VERSION,
        kind,
        deletion_id: old.id.clone(),
        journal_sequence: (kind == Kind::Operation).then_some(sequence),
        previous_digest: previous.map(|v| v.digest.clone()),
        request: request.clone(),
        receipt: receipt.clone(),
    };
    let encoded = records::encode(&row)?;
    let operation = records::encode(&operation)?;
    if enrolled {
        store
            .checkpoints
            .insert_one(encoded)
            .session(&mut *session)
            .await?;
    } else {
        let changed = store.checkpoints.replace_one(doc! {"_id":old.id.as_str(), "checkpoint.parent_revision":guard::signed(old.revision)?,"checkpoint.binding.sequence":guard::signed(request.binding().sequence)?},encoded).session(&mut *session).await?;
        if changed.matched_count != 1 {
            return Err(JournalError::Conflict.into());
        }
    }
    store
        .operations
        .insert_one(operation)
        .session(&mut *session)
        .await?;
    if !events.is_empty() {
        let rows = events
            .iter()
            .map(|v| super::event_document(v, None).map_err(|_| JournalError::Invalid))
            .collect::<Result<Vec<_>, _>>()?;
        crate::store::durability::majority_collection::<Document>(&store.database, EVENTS)
            .insert_many(rows)
            .session(&mut *session)
            .await?;
    }
    guard::publish(store, &old, &document, &next, claim_at, enrolled, session).await?;
    let claim = if next.work.lease.is_some() && !enrolled {
        Some(JournalClaim::from_record(checkpoint, &next)?)
    } else {
        None
    };
    Ok((
        JournalOutcome::Recorded(Box::new(JournalRecorded { receipt, claim })),
        true,
    ))
}

pub(super) async fn history(
    store: &MongoErasureJournal,
    request: &JournalRequest,
    checkpoint: Option<&CheckpointRow>,
    parent: &AccountDeletion,
    session: &mut ClientSession,
) -> Attempt<Option<JournalReceipt>> {
    if let Some(current) = checkpoint {
        let latest = store
            .operations
            .find_one(doc! {"_id":&current.checkpoint.last_operation})
            .session(&mut *session)
            .await?
            .ok_or(JournalError::Incompatible)?;
        let latest = records::operation(latest)?;
        let mut durable = latest.receipt.checkpoint;
        // Recovery fences only advance parent authority, never logical progress.
        if durable.parent_revision > current.checkpoint.parent_revision {
            return Err(JournalError::Incompatible.into());
        }
        durable.parent_revision = current.checkpoint.parent_revision;
        if durable != current.checkpoint
            || current.checkpoint.parent_revision != parent.revision
            || current.id != parent.id
            || !current.accepted.matches(parent)
            || current.checkpoint.binding.step != parent.step
        {
            return Err(JournalError::Incompatible.into());
        }
    }
    let Some(row) = store
        .operations
        .find_one(doc! {"_id":request.key()})
        .session(session)
        .await?
    else {
        return Ok(None);
    };
    let row = records::operation(row)?;
    if row.request != *request {
        return Err(JournalError::BindingMismatch.into());
    }
    let current = checkpoint.ok_or(JournalError::Incompatible)?;
    if !current.accepted.matches(parent)
        || current.checkpoint.binding.contract != request.binding().contract
        || current.checkpoint.binding.sequence < row.receipt.checkpoint.binding.sequence
        || current.checkpoint.binding.inventory_generation
            < row.receipt.checkpoint.binding.inventory_generation
        || parent.revision < row.receipt.checkpoint.parent_revision
    {
        return Err(JournalError::Incompatible.into());
    }
    Ok(Some(row.receipt))
}
