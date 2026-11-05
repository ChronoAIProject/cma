use futures_util::TryStreamExt;
use layer2_core::sandbox::{
    store::{PendingAgentApprovalPage, SandboxStoreError},
    Agent, AgentLogEntry, AgentLogKind,
};
use mongodb::{bson::doc, options::ReadConcern, ClientSession};

use super::{
    consumers::{semantic, visit_points},
    corrupt,
    projection::{self, EvidenceRecord},
};
use crate::store::{agent_log_integrity::TransactionLogError, sandbox_mongo::MongoSandboxStore};

pub(in crate::store) async fn page(
    store: &MongoSandboxStore,
    agent: &Agent,
    after: u64,
    limit: u16,
    session: &mut ClientSession,
) -> Result<Option<PendingAgentApprovalPage>, TransactionLogError> {
    if !(1..=200).contains(&limit) || after > agent.conversation.last_seq {
        return Err(SandboxStoreError::InvalidPage.into());
    }
    let Some((head, state)) = semantic(store, agent, session, true).await? else {
        return Ok(None);
    };
    let mut rows = store
        .segment_logs
        .evidence
        .find(doc! {
            "agent_id":&head.agent_id,"incarnation":&head.incarnation,"evidence_generation":&head.evidence_generation,"kind":"approval",
            "record.epoch":i64::try_from(state.approval_epoch).map_err(|_|corrupt())?,
            "record.request_sequence":{"$gt":i64::try_from(after).map_err(|_|corrupt())?},
        })
        .sort(doc! {"record.request_sequence":1})
        .limit(i64::from(limit) + 1)
        .batch_size(1)
        .read_concern(ReadConcern::snapshot())
        .session(&mut *session)
        .await
        .map_err(TransactionLogError::Driver)?;
    let mut requested = Vec::new();
    while let Some(row) = rows
        .stream(&mut *session)
        .try_next()
        .await
        .map_err(TransactionLogError::Driver)?
    {
        let key = row.get_str("entity_key").map_err(|_| corrupt())?.to_owned();
        let EvidenceRecord::Approval {
            approval_id,
            request_sequence: Some(sequence),
            epoch,
            ..
        } = projection::read_evidence(&head, "approval", &key, row)?
        else {
            return Err(corrupt().into());
        };
        if key != approval_id
            || epoch != state.approval_epoch
            || sequence <= after
            || sequence > head.last_seq as u64
            || requested
                .last()
                .is_some_and(|(previous, _)| *previous >= sequence)
        {
            return Err(corrupt().into());
        }
        requested.push((sequence, key));
    }
    let more = requested.len() > usize::from(limit);
    requested.truncate(usize::from(limit));
    let sequences = requested.iter().map(|(seq, _)| *seq).collect::<Vec<_>>();
    let mut result = PendingAgentApprovalPage {
        items: Vec::new(),
        next_after: None,
    };
    let mut bytes = 0;
    let mut full = false;
    visit_points(store, &head, &sequences, session, true, &mut |entry| {
        let expected = requested.binary_search_by_key(&entry.seq, |(seq, _)| *seq)
            .ok().map(|index| requested[index].1.as_str()).ok_or_else(corrupt)?;
        if !matches!(&entry.kind, AgentLogKind::ApprovalRequested { approval_id, .. } if approval_id == expected) {
            return Err(corrupt());
        }
        if full { return Ok(()); }
        let size = serde_json::to_vec(&entry).map_err(|_|corrupt())?.len();
        if bytes + size > 2 * 1024 * 1024 {
            if result.items.is_empty() {
                return Err(SandboxStoreError::Unavailable("approval exceeds page byte budget".into()));
            }
            full = true;
        } else {
            bytes += size;
            result.items.push(entry);
        }
        Ok(())
    }).await?;
    if full || more {
        result.next_after = result.items.last().map(|e| e.seq);
    }
    Ok(Some(result))
}

/// Preserve the compatibility response only when it fits the documented page
/// budget. Callers must explicitly request pages for a larger outstanding set.
pub(in crate::store) fn complete(
    page: PendingAgentApprovalPage,
) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
    if page.next_after.is_some() {
        return Err(SandboxStoreError::Unavailable(
            "pending approvals require pagination".into(),
        ));
    }
    Ok(page.items)
}
