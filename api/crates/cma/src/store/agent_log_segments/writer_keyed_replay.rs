//! Exact single-entry replay after process restart or later head advancement.
use super::*;
use layer2_core::{sandbox::AgentLogKind, AgentId};

pub(in crate::store) async fn replay(
    store: &MongoSandboxStore,
    id: &AgentId,
    operation: &PublicationIdentity,
    kind: &AgentLogKind,
) -> Result<Option<AgentLogEntry>, SandboxStoreError> {
    operation.validate()?;
    let mut snapshot = store
        .client
        .start_session()
        .snapshot(true)
        .await
        .map_err(driver)?;
    let row = store
        .agents
        .find_one(doc! {"_id":id.as_str()})
        .read_concern(ReadConcern::snapshot())
        .session(&mut snapshot)
        .await
        .map_err(driver)?
        .ok_or(SandboxStoreError::NotFound)?;
    let current = read_agent(&row)?;
    let incarnation = digest::incarnation(&current);
    let receipt = collection(store, PUBLICATIONS)
        .find_one(doc! {
            "_id":publication::identity(id.as_str(), &incarnation, &operation.key)
        })
        .read_concern(ReadConcern::snapshot())
        .session(&mut snapshot)
        .await
        .map_err(driver)?;
    let Some(receipt) = receipt else {
        return Ok(None);
    };
    let receipt = Publication::read(receipt)?;
    receipt.matches_request(&current, operation)?;
    if receipt.first_seq != receipt.last_seq || receipt.last_seq - receipt.expected_seq != 1 {
        return Err(SandboxStoreError::Conflict);
    }
    let head = read_head(store, &row, &mut snapshot, true)
        .await
        .map_err(error)?;
    verify_head(store, &receipt, &current, &head, &mut snapshot).await?;
    let mut found = None;
    visit_range(
        store,
        &head,
        LogRange {
            after: receipt.expected_seq as u64,
            through: receipt.last_seq as u64,
            limit: 1,
        },
        &mut snapshot,
        true,
        |entry| {
            if &entry.kind != kind
                || entry.appended_at != operation.appended_at
                || publication::result_digest(&entry)? != receipt.result_digest
            {
                return Err(SandboxStoreError::Conflict);
            }
            found = Some(entry);
            Ok(())
        },
    )
    .await
    .map_err(error)?;
    found.map(Some).ok_or_else(corrupt)
}
