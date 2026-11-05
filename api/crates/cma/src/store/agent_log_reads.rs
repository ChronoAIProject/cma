//! Complete-snapshot traversal for Mongo agent log page, range, and prefix reads.

use layer2_core::ids::AgentId;
use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::AgentLogEntry;
use mongodb::bson::doc;
use mongodb::options::ReadConcern;

use super::agent_log::scan_complete_log_with_snapshot_session;
use super::agent_log_integrity::TransactionLogError;
use super::sandbox_mongo::{unsigned, MongoSandboxStore};

pub(super) async fn read_point(
    store: &MongoSandboxStore,
    agent_id: &AgentId,
    sequence: u64,
) -> Result<Option<AgentLogEntry>, SandboxStoreError> {
    if sequence == 0 {
        return Ok(None);
    }
    let mut session = store
        .client
        .start_session()
        .snapshot(true)
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
    let Some(aggregate) = store
        .agents
        .find_one(doc! {"_id":agent_id.as_str()})
        .read_concern(ReadConcern::snapshot())
        .session(&mut session)
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
    else {
        return Ok(None);
    };
    if super::agent_log_segments::read_format(&aggregate)?.is_some() {
        let head = super::agent_log_segments::read_head(store, &aggregate, &mut session, true)
            .await
            .map_err(transaction_error)?;
        let through = sequence.min(head.last_seq as u64);
        let after = if sequence > head.last_seq as u64 {
            through
        } else {
            through - 1
        };
        let mut result = None;
        super::agent_log_segments::visit_range(
            store,
            &head,
            super::agent_log_segments::LogRange {
                after,
                through,
                limit: 1,
            },
            &mut session,
            true,
            |entry| {
                result = Some(entry);
                Ok(())
            },
        )
        .await
        .map_err(transaction_error)?;
        return Ok(result);
    }
    store.agent_log.find_one(doc! {"agent_id":agent_id.as_str(),"seq": super::sandbox_mongo::signed(sequence, "Agent log sequence").map_err(SandboxStoreError::Corrupt)?})
        .read_concern(ReadConcern::snapshot()).session(&mut session).await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
        .as_ref().map(super::agent_log::read_entry).transpose()
}

pub(super) async fn scan_read_range<F>(
    store: &MongoSandboxStore,
    agent_id: &AgentId,
    after: u64,
    through: Option<u64>,
    limit: u16,
    mut visit: F,
) -> Result<(), SandboxStoreError>
where
    F: FnMut(AgentLogEntry) -> Result<(), SandboxStoreError> + Send,
{
    let mut session = store
        .client
        .start_session()
        .snapshot(true)
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
    let aggregate = store
        .agents
        .find_one(doc! {"_id":agent_id.as_str()})
        .read_concern(ReadConcern::snapshot())
        .session(&mut session)
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
    let Some(aggregate) = aggregate else {
        return if through.is_some_and(|end| end > after) {
            Err(SandboxStoreError::SequenceConflict)
        } else {
            Ok(())
        };
    };
    let last_seq = aggregate
        .get_i64("last_seq")
        .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))
        .and_then(|n| unsigned(n, "Agent log head").map_err(SandboxStoreError::Corrupt))?;
    let after = if through.is_none() {
        after.min(last_seq)
    } else {
        after
    };
    let through = through.unwrap_or(last_seq);
    let expected = (through - after).min(u64::from(limit));
    if super::agent_log_segments::read_format(&aggregate)?.is_some() {
        let head = super::agent_log_segments::read_head(store, &aggregate, &mut session, true)
            .await
            .map_err(transaction_error)?;
        return super::agent_log_segments::visit_range(
            store,
            &head,
            super::agent_log_segments::LogRange {
                after,
                through,
                limit,
            },
            &mut session,
            true,
            visit,
        )
        .await
        .map_err(transaction_error);
    }
    let mut count = 0;
    scan_complete_log_with_snapshot_session(store, agent_id, last_seq, &mut session, |entry| {
        if entry.seq > after && entry.seq <= through && count < expected {
            visit(entry)?;
            count += 1;
        }
        Ok(())
    })
    .await
    .map_err(transaction_error)?;
    if count != expected {
        return Err(SandboxStoreError::SequenceConflict);
    }
    Ok(())
}

fn transaction_error(error: TransactionLogError) -> SandboxStoreError {
    match error {
        TransactionLogError::Domain(error) => error,
        TransactionLogError::Driver(error) => SandboxStoreError::Unavailable(error.to_string()),
    }
}
