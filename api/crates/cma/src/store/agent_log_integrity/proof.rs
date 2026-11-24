//! Bounded trusted-prefix checks and fail-closed invalidation.
use super::*;
use crate::store::transaction_terminal::GuardedTerminal as _;

pub(super) async fn validate_summary(
    store: &MongoSandboxStore,
    agent: &Agent,
    row: &Document,
    caller: ScanCaller,
    session: &mut mongodb::ClientSession,
    snapshot: bool,
) -> Result<Option<LogSummary>, TransactionLogError> {
    let result = read_only_summary(store, agent, row, session, snapshot).await;
    if !row.contains_key(QUARANTINE_FIELD)
        && matches!(
            &result,
            Err(TransactionLogError::Domain(SandboxStoreError::Corrupt(_)))
        )
    {
        // Preserve the known corruption classification even when its structural
        // audit succeeds; callers own any transaction abort and quarantine.
        let _ = scan::scan_corruption(
            caller,
            &store.agent_log,
            &agent.id,
            agent.conversation.last_seq,
            session,
            crate::store::agent_log_integrity::ScanOptions::new(snapshot, store.scan_cooperation),
        )
        .await;
    }
    result
}

/// Shared proof only: no audit, metadata write, or transaction lifecycle effects.
pub(super) async fn read_only_summary(
    store: &MongoSandboxStore,
    agent: &Agent,
    row: &Document,
    session: &mut mongodb::ClientSession,
    snapshot: bool,
) -> Result<Option<LogSummary>, TransactionLogError> {
    if row.contains_key(QUARANTINE_FIELD) {
        return Err(TransactionLogError::Domain(SandboxStoreError::Corrupt(
            "Agent log integrity is quarantined".into(),
        )));
    }
    let parsed = parse_integrity(row)?;
    let Some(integrity) = parsed else {
        if agent.conversation.last_seq == 0
            && point_read(
                store,
                doc! { "agent_id": agent.id.as_str() },
                true,
                session,
                snapshot,
            )
            .await?
            .is_some()
        {
            return Err(SandboxStoreError::Corrupt(
                "legacy zero-head Agent has retained log rows".into(),
            )
            .into());
        }
        return Ok(None);
    };
    if integrity.head_seq > agent.conversation.last_seq
        || integrity.bound_agent_revision > agent.revision
    {
        return Err(SandboxStoreError::Corrupt(
            "Agent log integrity is ahead of the Agent head or revision".into(),
        )
        .into());
    }
    let stale = integrity.head_seq < agent.conversation.last_seq
        || integrity.bound_agent_revision != agent.revision;
    if integrity.history_identity != history_identity(agent)
        || integrity.entry_count != integrity.head_seq
        || integrity.entry_count > PHYSICAL_AGENT_LOG_ENTRIES
        || integrity.encoded_bytes > PHYSICAL_AGENT_LOG_BYTES
        || (integrity.head_seq == 0
            && (integrity.encoded_bytes != 0 || integrity.head_hash != [0_u8; HASH_BYTES]))
        || integrity.semantic.through_seq != integrity.head_seq
        || (!stale && integrity.semantic.valid && !integrity.semantic.matches_agent(agent))
    {
        return Err(
            SandboxStoreError::Corrupt("Agent log integrity metadata is corrupt".into()).into(),
        );
    }
    if integrity.head_seq > 0 {
        let raw = point_read(
            store,
            // Match the same owner of the log row as the sequence scan before
            // returning a trusted tail's payload to the integrity validator.
            doc! { "_id": log_id(&agent.id, integrity.head_seq), "agent_id": agent.id.as_str() },
            false,
            session,
            snapshot,
        )
        .await?;
        let Some(raw) = raw else {
            return Err(SandboxStoreError::Corrupt(
                "Agent log integrity tail row is missing".into(),
            )
            .into());
        };
        let entry = Document::try_from(&raw)
            .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))
            .and_then(|document| read_entry(&document));
        let entry = entry?;
        if entry.agent_id != agent.id
            || entry.seq != integrity.head_seq
            || hash_raw(raw.as_bytes()) != integrity.head_hash
        {
            return Err(
                SandboxStoreError::Corrupt("Agent log integrity tail proof failed".into()).into(),
            );
        }
    }
    if stale {
        return Ok(None);
    }
    let mut extra_filter = doc! { "agent_id": agent.id.as_str() };
    if integrity.head_seq > 0 {
        extra_filter.insert("seq", doc! { "$gt": integrity.head_seq as i64 });
    }
    let extra = point_read(store, extra_filter, true, session, snapshot).await?;
    if extra.is_some() {
        return Err(SandboxStoreError::Corrupt(
            "Agent log has rows beyond its trusted head".into(),
        )
        .into());
    }
    if !integrity.semantic.valid {
        return Ok(None);
    }
    Ok(Some(LogSummary {
        entry_count: integrity.entry_count,
        encoded_bytes: integrity.encoded_bytes,
        head_hash: integrity.head_hash,
        semantic: Some(integrity.semantic),
    }))
}

/// Persist invalidation outside the failed transaction. Fence the immutable
/// incarnation and monotonic observed prefix, not its mutable revision: a
/// concurrent append/revision-only write cannot repair a damaged prefix and
/// must not erase the audit's finding. Explicit recovery must revalidate after
/// this marker is written before clearing it.
pub(in crate::store) async fn quarantine_row(
    store: &MongoSandboxStore,
    row: &Document,
) -> Result<(), TransactionLogError> {
    let observed = crate::store::agents::read_agent(row).map_err(TransactionLogError::Domain)?;
    let head = signed(observed.conversation.last_seq, "quarantined log head")
        .map_err(SandboxStoreError::Corrupt)?;
    let filter = doc! {
        "_id": observed.id.as_str(),
        "record.creation_key": &observed.creation_key,
        "record.creation_fingerprint": &observed.creation_fingerprint,
        "record.created_at": &observed.created_at,
        "record.workspace_id": observed.workspace_id.as_str(),
        "last_seq": { "$gte": head },
    };
    let result = store
        .agents
        .update_one(
            filter,
            doc! { "$set": {
                QUARANTINE_FIELD: {
                    "format": INTEGRITY_FORMAT,
                    "reason": "integrity_validation_failed",
                    "observed_head": head,
                }
            }},
        )
        .write_concern(mongodb::options::WriteConcern::majority())
        .await
        .map_err(TransactionLogError::Driver)?;
    if result.matched_count != 1 {
        return Err(SandboxStoreError::Corrupt(
            "Agent history changed before quarantine could be persisted".into(),
        )
        .into());
    }
    fleet::telemetry::record(fleet::telemetry::Event::QuarantineWritten);
    Ok(())
}

pub(in crate::store) async fn trusted_summary_at_head(
    store: &MongoSandboxStore,
    agent: &Agent,
    row: &Document,
    session: &mut mongodb::ClientSession,
) -> Result<Option<LogSummary>, TransactionLogError> {
    // Settlement charges the current tail, so refuse a stale anchor before
    // the integrity check can read its earlier log row.
    if !parse_integrity(row)?.is_some_and(|integrity| {
        integrity.head_seq == agent.conversation.last_seq
            && integrity.bound_agent_revision == agent.revision
    }) {
        return Ok(None);
    }
    read_only_summary(store, agent, row, session, false).await
}

pub(in crate::store) async fn trusted_summary(
    store: &MongoSandboxStore,
    agent: &Agent,
    row: &Document,
    caller: ScanCaller,
    session: &mut mongodb::ClientSession,
    snapshot: bool,
) -> Result<Option<LogSummary>, TransactionLogError> {
    let result = validate_summary(store, agent, row, caller, session, snapshot).await;
    if matches!(
        &result,
        Err(TransactionLogError::Domain(SandboxStoreError::Corrupt(_)))
    ) {
        let _ = session.abort_guarded().await;
        quarantine_row(store, row).await?;
    }
    if store.bounded_log_enabled(&agent.id) {
        use super::fleet::telemetry::{record, Event};
        if row.contains_key(QUARANTINE_FIELD) {
            record(Event::Quarantined);
        }
        match &result {
            Ok(Some(_)) => record(Event::WriteScanAvoided),
            Ok(None) => record(Event::Fallback),
            Err(_) => {}
        }
        result
    } else {
        result.map(|_| None)
    }
}

pub(super) async fn point_read(
    store: &MongoSandboxStore,
    filter: Document,
    id_only: bool,
    session: &mut mongodb::ClientSession,
    snapshot: bool,
) -> Result<Option<RawDocumentBuf>, TransactionLogError> {
    let mut observation = crate::store::mongo_read_telemetry::Read::find(store.agent_log.name());
    let collection = store.agent_log.clone_with_type::<RawDocumentBuf>();
    let find = collection.find_one(filter);
    let find = if id_only {
        find.projection(doc! { "_id": 1 })
    } else {
        find
    };
    let find = if snapshot {
        find.read_concern(mongodb::options::ReadConcern::snapshot())
    } else {
        find
    };
    let raw = find
        .session(session)
        .await
        .map_err(TransactionLogError::Driver)?;
    if let Some(raw) = &raw {
        observation.document(raw.as_bytes().len());
    }
    observation.complete();
    Ok(raw)
}
