//! Bounded trusted-prefix checks and fail-closed invalidation.
use super::*;

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
    let parsed = parse_summary_metadata(row)?;
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
    let stale = validate_metadata(agent, &integrity)?;
    if integrity.head_seq > 0 {
        let raw = point_read(
            store,
            doc! { "_id": log_id(&agent.id, integrity.head_seq) },
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
        validate_tail(
            &entry,
            &agent.id,
            integrity.head_seq,
            &integrity.head_hash,
            &raw,
        )?;
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

fn parse_summary_metadata(row: &Document) -> Result<Option<LogIntegrity>, SandboxStoreError> {
    if row.contains_key(QUARANTINE_FIELD) {
        return Err(SandboxStoreError::Corrupt(
            "Agent log integrity is quarantined".into(),
        ));
    }
    parse_integrity(row)
}

pub(super) fn validate_metadata(
    agent: &Agent,
    integrity: &LogIntegrity,
) -> Result<bool, SandboxStoreError> {
    if integrity.head_seq > agent.conversation.last_seq
        || integrity.bound_agent_revision > agent.revision
    {
        return Err(SandboxStoreError::Corrupt(
            "Agent log integrity is ahead of the Agent head or revision".into(),
        ));
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
        return Err(SandboxStoreError::Corrupt(
            "Agent log integrity metadata is corrupt".into(),
        ));
    }
    Ok(stale)
}

fn validate_tail(
    entry: &AgentLogEntry,
    agent_id: &AgentId,
    sequence: u64,
    head_hash: &[u8; HASH_BYTES],
    raw: &mongodb::bson::RawDocument,
) -> Result<(), SandboxStoreError> {
    if entry.agent_id != *agent_id
        || entry.seq != sequence
        || hash_raw(raw.as_bytes()) != *head_hash
    {
        return Err(SandboxStoreError::Corrupt(
            "Agent log integrity tail proof failed".into(),
        ));
    }
    Ok(())
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
        let _ = session.abort_transaction().await;
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
    point_read_raw(store, filter, id_only, session, snapshot)
        .await
        .map_err(TransactionLogError::Driver)
}

async fn point_read_raw(
    store: &MongoSandboxStore,
    filter: Document,
    id_only: bool,
    session: &mut mongodb::ClientSession,
    snapshot: bool,
) -> Result<Option<RawDocumentBuf>, mongodb::error::Error> {
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
    let raw = find.session(session).await?;
    if let Some(raw) = &raw {
        observation.document(raw.as_bytes().len());
    }
    observation.complete();
    Ok(raw)
}

use crate::store::ingress_reads::{
    error::{self, ReadFailure},
    row as ingress_row,
};
use layer2_core::sandbox::ingress_ownership::{
    AdmittedError, AdmittedOperation, AllocationPlan, IngressClass, IngressOwnershipPolicy,
    SharedOwned,
};

type PointRead<'a> =
    AdmittedOperation<'a, Result<Option<SharedOwned<RawDocumentBuf>>, ReadFailure>>;

impl MongoSandboxStore {
    /// Preserve the original point read, including id-only probes and telemetry.
    /// Query/driver work requires the caller's separately qualified read plan.
    /// The raw owner is not a decoded tail or a trusted-prefix proof.
    pub(in crate::store) fn integrity_point_read_admitted<'a, I>(
        &'a self,
        session: &'a mut mongodb::ClientSession,
        snapshot: bool,
        id_only: bool,
        policy: &'a IngressOwnershipPolicy,
        read_plan: AllocationPlan,
        initialize: I,
    ) -> Result<PointRead<'a>, AdmittedError>
    where
        I: FnOnce() -> Document + Send + 'a,
    {
        AdmittedOperation::from_owned(
            policy,
            IngressClass::Transformation,
            AllocationPlan::checked("integrity point read operation", &[])?,
            move |scope| async move {
                let result = ingress_row::load_raw_with_error(policy, read_plan, || {
                    point_read_raw(self, initialize(), id_only, session, snapshot)
                })
                .await;
                error::finish(result, &scope)
            },
        )
    }
}

#[path = "proof_ingress.rs"]
pub(super) mod ingress;

use crate::store::ingress_reads::{
    layout::{BsonLayout, LayoutError},
    row::{ConversionPlan, RowSchema},
};
use layer2_core::sandbox::ingress_ownership::sizing::shared_owner_bytes;

pub(super) struct SummarySchema;

impl RowSchema for SummarySchema {
    type Value = Option<LogIntegrity>;
    const INVALID_RAW: &'static str = "invalid raw integrity metadata";

    fn plan(layout: BsonLayout) -> Result<ConversionPlan, LayoutError> {
        // The checkpoint bounds each queued-message element and its strings.
        let retained = layout.domain_bytes(
            size_of::<SemanticCheckpoint>(),
            shared_owner_bytes::<Option<LogIntegrity>>(),
        )?;
        let temporary = layout
            .bson_bytes()?
            .checked_mul(3)
            .and_then(|bytes| retained.checked_mul(2)?.checked_add(bytes))
            .and_then(|bytes| layout.wire_bytes.checked_mul(8)?.checked_add(bytes))
            .and_then(|bytes| bytes.checked_add(4096))
            .ok_or(LayoutError::Structure)?;
        Ok(ConversionPlan {
            temporary_bytes: temporary,
            retained_bytes: retained,
        })
    }

    fn decode(row: Document) -> Result<Self::Value, SandboxStoreError> {
        parse_summary_metadata(&row)
    }
}

pub(super) use ingress::{copy_summary, summary_validation_plan};

#[cfg(test)]
pub(super) mod summary_tests {
    use super::*;
    use layer2_core::sandbox::ingress_ownership::{ClassBounds, IngressBudget};

    pub(in crate::store::agent_log_integrity) fn policy() -> (IngressBudget, IngressOwnershipPolicy)
    {
        let root = IngressBudget::new(128 * 1024 * 1024);
        let plan = |n| AllocationPlan::checked("summary fixture", &[n]).unwrap();
        let policy = IngressOwnershipPolicy::new(
            root.clone(),
            ClassBounds::new(
                root.capacity_bytes(),
                plan(1024 * 1024),
                plan(96 * 1024 * 1024),
            )
            .unwrap(),
        )
        .unwrap();
        (root, policy)
    }

    pub(in crate::store::agent_log_integrity) fn metadata(
        agent: &Agent,
        hash: [u8; HASH_BYTES],
    ) -> LogIntegrity {
        let mut checkpoint =
            mongodb::bson::serialize_to_document(&SemanticCheckpoint::empty()).unwrap();
        checkpoint.insert("through_seq", agent.conversation.last_seq as i64);
        checkpoint.insert("creation_seen", true);
        LogIntegrity {
            head_seq: agent.conversation.last_seq,
            entry_count: agent.conversation.last_seq,
            encoded_bytes: 100 * agent.conversation.last_seq as usize,
            head_hash: hash,
            bound_agent_revision: agent.revision,
            history_identity: history_identity(agent),
            semantic: super::super::metadata::parse_semantic(&checkpoint).unwrap(),
        }
    }

    pub(in crate::store::agent_log_integrity) fn read<'a>(
        store: &'a MongoSandboxStore,
        agent: &'a Agent,
        raw: &'a mongodb::bson::RawDocument,
        session: &'a mut mongodb::ClientSession,
        snapshot: bool,
        policy: &'a IngressOwnershipPolicy,
    ) -> Result<super::super::read::SummaryRead<'a>, AdmittedError> {
        super::super::read::read_only_summary_admitted(
            store,
            agent,
            raw,
            session,
            snapshot,
            policy,
            AllocationPlan::checked("summary source", &[50 * 1024 * 1024])?,
        )
    }

    #[test]
    fn metadata_and_summary_keep_independent_data_and_original_errors() {
        let (root, policy) = policy();
        let plan = |n| AllocationPlan::checked("summary fixture", &[n]).unwrap();
        let idle = root.available_bytes();
        let mut semantic =
            mongodb::bson::serialize_to_document(&SemanticCheckpoint::empty()).unwrap();
        semantic.insert("valid", false);
        semantic.insert("conversation_id", "c".repeat(32768));
        semantic.insert(
            "queued_owner_messages",
            (0..128)
                .map(|n| {
                    doc! {
                        "sequence":n+1, "content_sha256":"a".repeat(64), "nonempty":true,
                    }
                })
                .collect::<Vec<_>>(),
        );
        let metadata = LogIntegrity {
            head_seq: 0,
            entry_count: 0,
            encoded_bytes: 0,
            head_hash: [0; HASH_BYTES],
            bound_agent_revision: 1,
            history_identity: [0; HASH_BYTES],
            semantic: super::super::metadata::parse_semantic(&semantic).unwrap(),
        };
        let row = doc! {INTEGRITY_FIELD:metadata.to_document().unwrap()};
        let raw = RawDocumentBuf::try_from(&row).unwrap();
        let parsed = ingress_row::decode::<SummarySchema>(&raw, &policy).unwrap();
        let copy = copy_summary(&parsed, &policy).unwrap();
        assert_eq!(copy.semantic.as_ref(), Some(&metadata.semantic));
        assert!(copy.belongs_to(&policy));
        let both = root.available_bytes();
        drop(parsed);
        assert!(root.available_bytes() > both);
        assert!(root.available_bytes() < idle);
        drop(copy);
        assert_eq!(root.available_bytes(), idle);
        let mut bad = row.clone();
        bad.get_document_mut(INTEGRITY_FIELD)
            .unwrap()
            .insert("head_seq", -1_i64);
        let expected = parse_summary_metadata(&bad).unwrap_err();
        let raw = RawDocumentBuf::try_from(&bad).unwrap();
        let pressure = policy
            .admit(IngressClass::Data, plan(policy.bounds().data_bytes()))
            .unwrap();
        assert!(matches!(
            ingress_row::decode::<SummarySchema>(&raw, &policy),
            Err(ingress_row::RowError::Refused(LayoutError::Capacity))
        ));
        drop(pressure);
        let ingress_row::RowError::Domain(error) =
            ingress_row::decode::<SummarySchema>(&raw, &policy).unwrap_err()
        else {
            panic!("original metadata error required")
        };
        assert_eq!(*error, expected);
        drop(error);
        assert_eq!(root.available_bytes(), idle);
    }
}
