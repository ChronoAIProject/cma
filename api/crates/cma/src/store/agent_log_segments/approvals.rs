use super::{
    consumers::{points, semantic},
    corrupt,
    projection::{self, EvidenceRecord},
};
use crate::store::{agent_log_integrity::TransactionLogError, sandbox_mongo::MongoSandboxStore};
use layer2_core::sandbox::{store::SandboxStoreError, Agent, AgentLogEntry, AgentLogKind};
use mongodb::{bson::doc, options::ReadConcern, ClientSession};

/// None is the legacy fallback; Some certifies an indexed segmented snapshot.
pub(in crate::store) async fn exact(
    store: &MongoSandboxStore,
    agent: &Agent,
    key: &str,
    session: &mut ClientSession,
    snapshot: bool,
) -> Result<Option<(Option<AgentLogEntry>, bool)>, TransactionLogError> {
    let Some((head, state)) = semantic(store, agent, session, snapshot).await? else {
        return Ok(None);
    };
    let row = projection::evidence(store, &head, "approval", key, session).await?;
    let sequence = match row {
        Some(EvidenceRecord::Approval {
            approval_id,
            request_sequence,
            epoch,
            ..
        }) if approval_id == key && epoch == state.approval_epoch => request_sequence,
        Some(EvidenceRecord::Approval { .. }) | None => None,
        _ => return Err(corrupt().into()),
    };
    let entry = if let Some(sequence) = sequence {
        let entry = points(store, &head, &[sequence], session, snapshot)
            .await?
            .pop()
            .ok_or_else(corrupt)?;
        if !matches!(&entry.kind, AgentLogKind::ApprovalRequested { approval_id, .. } if approval_id == key)
        {
            return Err(corrupt().into());
        }
        Some(entry)
    } else {
        None
    };
    let find = store.segment_logs.evidence.find_one(doc! {
        "agent_id":&head.agent_id,"incarnation":&head.incarnation,"evidence_generation":&head.evidence_generation,"kind":"approval","entity_key":{"$ne":key},
        "record.epoch":i64::try_from(state.approval_epoch).map_err(|_|corrupt())?,
        "record.request_sequence":{"$gt":0_i64},
    });
    let find = if snapshot {
        find.read_concern(ReadConcern::snapshot())
    } else {
        find
    };
    let other = find
        .session(session)
        .await
        .map_err(TransactionLogError::Driver)?;
    let other = other.map(|row| {
        let key = row.get_str("entity_key").map_err(|_|corrupt())?.to_owned();
        let record = projection::read_evidence(&head, "approval", &key, row)?;
        if !matches!(record, EvidenceRecord::Approval { approval_id, request_sequence:Some(sequence), epoch, .. }
            if approval_id == key && epoch == state.approval_epoch && sequence > 0 && sequence <= head.last_seq as u64) {
            return Err(corrupt());
        }
        Ok(())
    }).transpose()?.is_some();
    Ok(Some((entry, other)))
}

pub(in crate::store) fn store_error(error: TransactionLogError) -> SandboxStoreError {
    match error {
        TransactionLogError::Domain(error) => error,
        TransactionLogError::Driver(error) => SandboxStoreError::Unavailable(error.to_string()),
    }
}
