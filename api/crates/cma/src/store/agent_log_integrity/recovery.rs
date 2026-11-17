//! Exact head evidence for recovery that only fences an unchanged terminal Agent.
use super::*;

pub(in crate::store) async fn exact_recovery_head(
    store: &MongoSandboxStore,
    agent: &Agent,
    row: &Document,
    session: &mut mongodb::ClientSession,
) -> Result<bool, TransactionLogError> {
    // An ineligible bounded proof always returns to the legacy structural and
    // semantic audit; it must never manufacture an append summary.
    if row.contains_key(QUARANTINE_FIELD) {
        return Ok(false);
    }
    let head = agent.conversation.last_seq;
    if head > 0 {
        let Some(raw) = proof::point_read(
            store,
            doc! { "_id": log_id(&agent.id, head) },
            false,
            session,
            false,
        )
        .await?
        else {
            return Ok(false);
        };
        let Ok(document) = Document::try_from(&raw) else {
            return Ok(false);
        };
        let Ok(entry) = read_entry(&document) else {
            return Ok(false);
        };
        if entry.agent_id != agent.id
            || entry.seq != head
            || document.contains_key(QUARANTINE_FIELD)
        {
            return Ok(false);
        }
    }
    let mut filter = doc! { "agent_id": agent.id.as_str() };
    if head > 0 {
        let Ok(head) = signed(head, "runtime-loss log head") else {
            return Ok(false);
        };
        filter.insert("seq", doc! { "$gt": head });
    }
    Ok(proof::point_read(store, filter, true, session, false)
        .await?
        .is_none())
}
