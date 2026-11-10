//! Complete-snapshot traversal for Mongo agent log page, range, and prefix reads.

use crate::store::agent_log_telemetry::ScanCaller;
use layer2_core::ids::AgentId;
use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::AgentLogEntry;
use mongodb::bson::doc;
use mongodb::options::ReadConcern;

use super::agent_log::scan_complete_log_with_snapshot_session;
use super::agent_log_integrity::TransactionLogError;
use super::sandbox_mongo::{require_version, unsigned, MongoSandboxStore};

pub(super) async fn scan_read_log<F>(
    store: &MongoSandboxStore,
    agent_id: &AgentId,
    visit: F,
) -> Result<(), SandboxStoreError>
where
    F: FnMut(AgentLogEntry) -> Result<(), SandboxStoreError>,
{
    let mut session = store
        .client
        .start_session()
        .snapshot(true)
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
    let Some(aggregate) = store
        .agents
        .find_one(doc! { "_id": agent_id.as_str() })
        .projection(doc! { "document_schema_version": 1, "last_seq": 1 })
        .read_concern(ReadConcern::snapshot())
        .session(&mut session)
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
    else {
        return Ok(());
    };
    require_version(&aggregate).map_err(SandboxStoreError::Corrupt)?;
    let last_seq = aggregate
        .get_i64("last_seq")
        .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))
        .and_then(|value| {
            unsigned(value, "agent log sequence").map_err(SandboxStoreError::Corrupt)
        })?;
    scan_complete_log_with_snapshot_session(
        ScanCaller::PublicWindowPrefix,
        store,
        agent_id,
        last_seq,
        &mut session,
        visit,
    )
    .await
    .map_err(transaction_error)?;
    Ok(())
}

fn transaction_error(error: TransactionLogError) -> SandboxStoreError {
    match error {
        TransactionLogError::Domain(error) => error,
        TransactionLogError::Driver(error) => SandboxStoreError::Unavailable(error.to_string()),
    }
}
