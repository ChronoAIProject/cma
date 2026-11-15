//! One physical legacy unit: fold once and retain raw leaf hashes, never text.
use super::*;
use crate::store::{agent_log::read_entry, agent_log_integrity::read::legacy_proof_for_context};
use layer2_core::sandbox::{AgentLogKind, BoundedAgentLogFold};
use mongodb::bson::RawDocumentBuf;
use std::future::IntoFuture;

use super::super::{corrupt, digest, MAX_BYTES, MAX_ENTRIES, MIGRATION, READER};

pub(super) async fn read(
    store: &MongoSandboxStore,
    agent: &Agent,
    row: &Document,
    session: &mut ClientSession,
    context: SourceContext,
    snapshot: bool,
) -> Result<CapturedLog, TransactionLogError> {
    legacy_proof_for_context(store, agent, row, session, snapshot).await?;
    if agent.conversation.last_seq > MAX_ENTRIES as u64 {
        return Err(SandboxStoreError::AgentLogCapacityExceeded.into());
    }
    let mut scan = crate::store::agent_log_telemetry::Scan::new(
        crate::store::agent_log_telemetry::ScanCaller::Other,
    );
    let incarnation = digest::incarnation(agent);
    let source = store.agent_log.clone_with_type::<RawDocumentBuf>();
    let mut read = crate::store::mongo_read_telemetry::Read::find(source.name());
    let query = source
        .find(doc! {"agent_id":agent.id.as_str()})
        .sort(doc! {"seq":1})
        .limit(MAX_ENTRIES + 1)
        .batch_size(64);
    let query = if snapshot {
        query.read_concern(ReadConcern::snapshot())
    } else {
        query
    };
    let mut cursor = scan
        .observe(query.session(&mut *session).into_future(), false)
        .await
        .map_err(TransactionLogError::Driver)?;
    let mut fold = BoundedAgentLogFold::default();
    let mut leaves = Vec::new();
    let mut count = 0_i64;
    let mut bytes = 0_i64;
    let mut latest_entry = None;
    let mut max_opened_generation = 0;
    let mut max_retired_generation = 0;
    while let Some(raw) = scan
        .observe(read.next_in_session(&mut cursor, session), true)
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
