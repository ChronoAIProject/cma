use super::worker::*;

pub(super) async fn parent(
    store: &MongoErasureJournal,
    id: &AccountDeletionId,
    session: &mut ClientSession,
) -> Attempt<(AccountDeletion, Document, String)> {
    let result = store
        .database
        .run_command(doc! {
            "aggregate":RECORDS, "pipeline":[{"$match":{"_id":id.as_str()}},
            {"$project":{"record":"$$ROOT","at":"$$NOW","_id":0}}],
            "cursor":{"batchSize":2},"maxTimeMS":5000,
        })
        .session(session)
        .await?;
    let cursor = result
        .get_document("cursor")
        .map_err(|_| JournalError::Incompatible)?;
    if cursor.get_i64("id").unwrap_or(-1) != 0 {
        return Err(JournalError::Incompatible.into());
    }
    let rows = cursor
        .get_array("firstBatch")
        .map_err(|_| JournalError::Incompatible)?;
    if rows.len() != 1 {
        return Err(JournalError::Conflict.into());
    }
    let row = rows[0].as_document().ok_or(JournalError::Incompatible)?;
    let at = row
        .get_datetime("at")
        .map_err(|_| JournalError::Incompatible)?;
    let document = row
        .get_document("record")
        .map_err(|_| JournalError::Incompatible)?
        .clone();
    let parent =
        super::records::read_record(document.clone()).map_err(|_| JournalError::Incompatible)?;
    if !parent.is_active() || parent.revision == 0 || parent.work.next_fence == 0 {
        return Err(JournalError::Incompatible.into());
    }
    super::records::date(&parent.requested_at).map_err(|_| JournalError::Incompatible)?;
    super::records::date(&parent.deadline_at).map_err(|_| JournalError::Incompatible)?;
    super::records::date(&parent.work.ready_at).map_err(|_| JournalError::Incompatible)?;
    if let Some(lease) = &parent.work.lease {
        super::records::date(&lease.expires_at).map_err(|_| JournalError::Incompatible)?;
    }
    Ok((
        parent,
        document,
        layer2_core::rfc3339_at(at.to_system_time()),
    ))
}

pub(super) fn claim(
    row: &CheckpointRow,
    parent: &AccountDeletion,
    handle: &JournalClaim,
) -> Result<(), JournalError> {
    if parent.status != AccountDeletionStatus::Running
        || row.checkpoint != *handle.checkpoint()
        || row.checkpoint.parent_revision != parent.revision
        || parent.work.lease.as_ref() != Some(handle.lease())
        || parent.work.next_fence != handle.next_fence()
        || parent.step != row.checkpoint.binding.step
        || !row.accepted.matches(parent)
    {
        return Err(JournalError::StaleClaim);
    }
    Ok(())
}

pub(super) async fn publish(
    store: &MongoErasureJournal,
    old: &AccountDeletion,
    document: &Document,
    next: &AccountDeletion,
    claim_at: Option<&str>,
    enrollment: bool,
    session: &mut ClientSession,
) -> Attempt<()> {
    super::completion_guard::require_pending(next, &[]).map_err(|_| JournalError::Incompatible)?;
    super::completion_guard::preserve_acceptance(old, next)
        .map_err(|_| JournalError::Incompatible)?;
    if next.revision != old.revision.checked_add(1).ok_or(JournalError::Capacity)?
        || next.step != old.step
    {
        return Err(JournalError::Invalid.into());
    }
    let mut filter = doc! {"_id":old.id.as_str(),"revision":signed(old.revision)?,
    "work.next_fence":signed(old.work.next_fence)?, "subject_hash":&old.subject_hash,
    "requested_at":&old.requested_at,"deadline_at":&old.deadline_at,
    "step":mongodb::bson::serialize_to_bson(&old.step).map_err(|_| JournalError::Invalid)?};
    match &old.work.lease {
        Some(lease) => {
            filter.insert("work.lease.token", &lease.token);
            filter.insert("work.lease.fence", signed(lease.fence)?);
        }
        None => {
            filter.insert("work.lease", mongodb::bson::Bson::Null);
        }
    }
    if !enrollment {
        super::lease_admission::guard(&mut filter, old, next, claim_at)
            .map_err(|_| JournalError::StaleClaim)?;
    }
    let keys = super::records::request_keys(document).map_err(|_| JournalError::Incompatible)?;
    let replacement =
        super::records::record_document(next, &keys).map_err(|_| JournalError::Incompatible)?;
    #[cfg(test)]
    store
        .pause_at(super::commit_pause::Point::BeforeGuard)
        .await;
    let changed = store
        .parents()
        .replace_one(filter, replacement)
        .session(session)
        .await?;
    if changed.matched_count != 1 {
        return Err(JournalError::StaleClaim.into());
    }
    Ok(())
}

pub(super) fn signed(value: u64) -> Result<i64, JournalError> {
    i64::try_from(value).map_err(|_| JournalError::Capacity)
}
