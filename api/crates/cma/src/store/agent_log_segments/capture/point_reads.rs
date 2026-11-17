use super::super::{corrupt, digest, reads};
use super::*;
use crate::store::agent_log::read_entry;
use mongodb::bson::RawDocumentBuf;

pub(super) async fn read(
    store: &MongoSandboxStore,
    captured: &CapturedLog,
    sequences: &[u64],
    session: &mut ClientSession,
    snapshot: bool,
) -> Result<Vec<AgentLogEntry>, TransactionLogError> {
    // A transaction can see its own writes. Reject an aggregate/head changed
    // since capture even while the exclusive transaction stamp still matches.
    let query = store
        .agents
        .find_one(doc! {"_id":captured.agent.id.as_str()});
    let query = if snapshot {
        query.read_concern(ReadConcern::snapshot())
    } else {
        query
    };
    let row = query
        .session(&mut *session)
        .await
        .map_err(TransactionLogError::Driver)?
        .ok_or_else(corrupt)?;
    if crate::store::agents::read_agent(&row)? != captured.agent {
        return Err(SandboxStoreError::SequenceConflict.into());
    }
    match captured.format {
        CapturedFormat::Legacy => {
            if super::super::read_format(&row)?.is_some() {
                return Err(corrupt().into());
            }
            crate::store::agent_log_integrity::read::legacy_proof_for_context(
                store,
                &captured.agent,
                &row,
                session,
                snapshot,
            )
            .await?;
        }
        CapturedFormat::Segmented => {
            let head = super::super::read_head(store, &row, session, snapshot).await?;
            if head != captured.active_head {
                return Err(SandboxStoreError::SequenceConflict.into());
            }
        }
    }
    let source = store.agent_log.clone_with_type::<RawDocumentBuf>();
    let mut read = crate::store::mongo_read_telemetry::Read::find(source.name());
    let signed = sequences.iter().map(|seq| *seq as i64).collect::<Vec<_>>();
    let query = source
        .find(doc! {"agent_id":captured.agent.id.as_str(),"seq":{"$in":signed}})
        .sort(doc! {"seq":1})
        .limit(33)
        .batch_size(32);
    let query = if snapshot {
        query.read_concern(ReadConcern::snapshot())
    } else {
        query
    };
    let mut cursor = query
        .session(&mut *session)
        .await
        .map_err(TransactionLogError::Driver)?;
    let mut entries = Vec::with_capacity(sequences.len());
    let mut bytes = 0_usize;
    let mut seal: Option<super::super::Seal> = None;
    while let Some(raw) = read
        .next_in_session(&mut cursor, session)
        .await
        .map_err(TransactionLogError::Driver)?
    {
        bytes = bytes
            .checked_add(raw.as_bytes().len())
            .filter(|n| *n <= 4 * 1024 * 1024)
            .ok_or(SandboxStoreError::AgentLogCapacityExceeded)?;
        let entry = read_entry(&Document::try_from(&raw).map_err(|_| corrupt())?)?;
        if entry.agent_id != captured.agent.id || sequences.get(entries.len()) != Some(&entry.seq) {
            return Err(SandboxStoreError::SequenceConflict.into());
        }
        let seq = entry.seq as i64;
        let leaf = digest::leaf(
            captured.agent.id.as_str(),
            &captured.incarnation,
            seq,
            raw.as_bytes(),
        );
        if seq >= captured.active_head.active_start_seq {
            digest::check_leaf(
                &captured.active_leaf_digests,
                seq - captured.active_head.active_start_seq,
                &leaf,
            )?;
        } else {
            if seal
                .as_ref()
                .is_none_or(|s| seq < s.start_seq || seq > s.end_seq)
            {
                seal = Some(
                    reads::containing(store, &captured.active_head, seq, session, snapshot).await?,
                );
            }
            let seal = seal.as_ref().ok_or_else(corrupt)?;
            digest::check_leaf(&seal.leaf_digests, seq - seal.start_seq, &leaf)?;
        }
        entries.push(entry);
    }
    if entries.len() != sequences.len() {
        return Err(SandboxStoreError::SequenceConflict.into());
    }
    read.complete();
    Ok(entries)
}
