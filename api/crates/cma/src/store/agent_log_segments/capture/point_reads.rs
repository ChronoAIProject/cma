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
    read_budgeted(store, captured, sequences, session, snapshot, None).await
}

pub(super) async fn read_budgeted(
    store: &MongoSandboxStore,
    captured: &CapturedLog,
    sequences: &[u64],
    session: &mut ClientSession,
    snapshot: bool,
    mut budget: Option<&mut preview_budget::PreviewBudget>,
) -> Result<Vec<AgentLogEntry>, TransactionLogError> {
    // A transaction can see its own writes. Reject an aggregate/head changed
    // since capture even while the exclusive transaction stamp still matches.
    let row = if let Some(budget) = budget.as_deref_mut() {
        budget
            .document(
                &store.agents,
                doc! {"_id":captured.agent.id.as_str()},
                None,
                session,
            )
            .await?
            .ok_or_else(corrupt)?
    } else {
        let query = store
            .agents
            .find_one(doc! {"_id":captured.agent.id.as_str()});
        let query = if snapshot {
            query.read_concern(ReadConcern::snapshot())
        } else {
            query
        };
        query
            .session(&mut *session)
            .await
            .map_err(TransactionLogError::Driver)?
            .ok_or_else(corrupt)?
    };
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
            let head = if let Some(budget) = budget.as_deref_mut() {
                reads::preview_head(store, &row, session, budget).await?
            } else {
                super::super::read_head(store, &row, session, snapshot).await?
            };
            if head != captured.active_head {
                return Err(SandboxStoreError::SequenceConflict.into());
            }
        }
    }
    let source = store.agent_log.clone_with_type::<RawDocumentBuf>();
    let mut read = budget
        .is_none()
        .then(|| crate::store::mongo_read_telemetry::Read::find(source.name()));
    let signed = sequences.iter().map(|seq| *seq as i64).collect::<Vec<_>>();
    let query = source
        .find(doc! {"agent_id":captured.agent.id.as_str(),"seq":{"$in":signed.clone()}})
        .sort(doc! {"seq":1})
        .limit(33)
        .batch_size(32);
    let query = if snapshot {
        query.read_concern(ReadConcern::snapshot())
    } else {
        query
    };
    let mut cursor = if budget.is_none() {
        Some(
            query
                .session(&mut *session)
                .await
                .map_err(TransactionLogError::Driver)?,
        )
    } else {
        None
    };
    let mut preview_rows = preview_budget::PreviewRows::new(
        store.agent_log.clone(),
        doc! {"agent_id":captured.agent.id.as_str(),"seq":{"$in":signed}},
        33,
    );
    let mut entries = Vec::with_capacity(sequences.len());
    let mut bytes = 0_usize;
    let mut seal: Option<super::super::Seal> = None;
    while let Some(raw) = if let Some(budget) = budget.as_deref_mut() {
        preview_rows.next(session, budget).await?
    } else {
        read.as_mut()
            .ok_or_else(corrupt)?
            .next_in_session(cursor.as_mut().ok_or_else(corrupt)?, session)
            .await
            .map_err(TransactionLogError::Driver)?
    } {
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
                seal = Some(if let Some(budget) = budget.as_deref_mut() {
                    reads::preview_containing(store, &captured.active_head, seq, session, budget)
                        .await?
                } else {
                    reads::containing(store, &captured.active_head, seq, session, snapshot).await?
                });
            }
            let seal = seal.as_ref().ok_or_else(corrupt)?;
            digest::check_leaf(&seal.leaf_digests, seq - seal.start_seq, &leaf)?;
        }
        entries.push(entry);
    }
    if entries.len() != sequences.len() {
        return Err(SandboxStoreError::SequenceConflict.into());
    }
    if let Some(budget) = budget {
        budget.check()?;
    }
    if let Some(read) = read.as_mut() {
        read.complete();
    }
    Ok(entries)
}
