//! One physical legacy unit: fold once and retain raw leaf hashes, never text.
use super::*;
use crate::store::{agent_log::read_entry, agent_log_integrity::read::legacy_proof};
use layer2_core::sandbox::{AgentLogKind, BoundedAgentLogFold};
use mongodb::bson::RawDocumentBuf;

use super::super::{corrupt, digest, MAX_BYTES, MAX_ENTRIES, MIGRATION, READER};

pub(super) async fn legacy(
    store: &MongoSandboxStore,
    agent: &Agent,
    row: &Document,
    session: &mut ClientSession,
    context: SnapshotContext,
) -> Result<CapturedLog, TransactionLogError> {
    legacy_proof(store, agent, row, session).await?;
    if agent.conversation.last_seq > MAX_ENTRIES as u64 {
        return Err(SandboxStoreError::AgentLogCapacityExceeded.into());
    }
    let mut scan = crate::store::agent_log_telemetry::Scan::new(
        crate::store::agent_log_telemetry::ScanCaller::Other,
    );
    let incarnation = digest::incarnation(agent);
    let source = store.agent_log.clone_with_type::<RawDocumentBuf>();
    let mut read = crate::store::mongo_read_telemetry::Read::find(source.name());
    let mut cursor = source
        .find(doc! {"agent_id":agent.id.as_str()})
        .sort(doc! {"seq":1})
        .limit(MAX_ENTRIES + 1)
        .batch_size(64)
        .read_concern(ReadConcern::snapshot())
        .session(&mut *session)
        .await
        .map_err(TransactionLogError::Driver)?;
    let mut fold = BoundedAgentLogFold::default();
    let mut leaves = Vec::new();
    let mut count = 0_i64;
    let mut bytes = 0_i64;
    let mut latest_entry = None;
    let mut max_opened_generation = 0;
    let mut max_retired_generation = 0;
    while let Some(raw) = read
        .next_in_session(&mut cursor, session)
        .await
        .map_err(TransactionLogError::Driver)?
    {
        scan.document(raw.as_bytes().len());
        let entry = read_entry(&Document::try_from(&raw).map_err(|_| corrupt())?)?;
        count += 1;
        if count > MAX_ENTRIES
            || entry.agent_id != agent.id
            || entry.seq != count as u64
            || entry.seq > agent.conversation.last_seq
        {
            return Err(SandboxStoreError::SequenceConflict.into());
        }
        let length = i64::try_from(raw.as_bytes().len()).map_err(|_| corrupt())?;
        bytes = bytes
            .checked_add(length)
            .filter(|n| *n <= MAX_BYTES)
            .ok_or(SandboxStoreError::AgentLogCapacityExceeded)?;
        if length > 16 * 1024 * 1024 {
            return Err(SandboxStoreError::AgentLogCapacityExceeded.into());
        }
        match &entry.kind {
            AgentLogKind::ConversationOpened {
                runtime_generation, ..
            } => {
                max_opened_generation = max_opened_generation.max(*runtime_generation);
            }
            AgentLogKind::RuntimeGenerationRetired {
                runtime_generation, ..
            } => {
                max_retired_generation = max_retired_generation.max(*runtime_generation);
            }
            _ => {}
        }
        // The shared fold validates original creation lineage, semantic order,
        // native identity and the exact aggregate, including queued input hashes.
        fold.observe(agent, &entry)
            .map_err(SandboxStoreError::Corrupt)?;
        leaves.extend_from_slice(
            &digest::leaf(agent.id.as_str(), &incarnation, count, raw.as_bytes()).bytes,
        );
        latest_entry = Some(entry);
    }
    if count as u64 != agent.conversation.last_seq {
        return Err(SandboxStoreError::SequenceConflict.into());
    }
    let projection = fold.finish(agent).map_err(SandboxStoreError::Corrupt)?;
    let head = Head {
        agent_id: agent.id.to_string(),
        format_version: 1,
        incarnation: incarnation.clone(),
        revision: i64::try_from(agent.revision).map_err(|_| corrupt())?,
        active_ordinal: 0,
        active_start_seq: 1,
        last_seq: count,
        active_count: count,
        active_bytes: bytes,
        previous_seal: None,
        lifetime_count: count,
        lifetime_bytes: bytes,
        projection_format: 1,
        evidence_generation: "captured-legacy".into(),
        projection_through: count,
        migration_id: MIGRATION.into(),
        required_reader: READER.into(),
    };
    head.validate()?;
    read.complete();
    scan.complete();
    Ok(CapturedLog {
        agent: agent.clone(),
        format: CapturedFormat::Legacy,
        incarnation,
        latest_entry,
        projection,
        max_opened_generation,
        max_retired_generation,
        active_head: head,
        active_leaf_digests: digest::binary(leaves),
        context,
    })
}

pub(super) async fn points(
    store: &MongoSandboxStore,
    captured: &CapturedLog,
    sequences: &[u64],
    session: &mut ClientSession,
) -> Result<Vec<AgentLogEntry>, TransactionLogError> {
    if captured.format != CapturedFormat::Legacy {
        return Err(pending());
    }
    let source = store.agent_log.clone_with_type::<RawDocumentBuf>();
    let mut read = crate::store::mongo_read_telemetry::Read::find(source.name());
    let signed = sequences.iter().map(|seq| *seq as i64).collect::<Vec<_>>();
    let mut cursor = source
        .find(doc! {"agent_id":captured.agent.id.as_str(),"seq":{"$in":signed}})
        .sort(doc! {"seq":1})
        .limit(33)
        .batch_size(32)
        .read_concern(ReadConcern::snapshot())
        .session(&mut *session)
        .await
        .map_err(TransactionLogError::Driver)?;
    let mut entries = Vec::with_capacity(sequences.len());
    let mut bytes = 0_usize;
    while let Some(raw) = read
        .next_in_session(&mut cursor, session)
        .await
        .map_err(TransactionLogError::Driver)?
    {
        bytes = bytes
            .checked_add(raw.as_bytes().len())
            .filter(|n| *n <= 4 * 1024 * 1024)
            .ok_or_else(|| {
                SandboxStoreError::Unavailable(
                    "captured Agent points exceed the source byte budget".into(),
                )
            })?;
        let entry = read_entry(&Document::try_from(&raw).map_err(|_| corrupt())?)?;
        if entry.agent_id != captured.agent.id || sequences.get(entries.len()) != Some(&entry.seq) {
            return Err(SandboxStoreError::SequenceConflict.into());
        }
        let leaf = digest::leaf(
            captured.agent.id.as_str(),
            &captured.incarnation,
            entry.seq as i64,
            raw.as_bytes(),
        );
        digest::check_leaf(&captured.active_leaf_digests, entry.seq as i64 - 1, &leaf)?;
        entries.push(entry);
    }
    if entries.len() != sequences.len() {
        return Err(SandboxStoreError::SequenceConflict.into());
    }
    read.complete();
    Ok(entries)
}
