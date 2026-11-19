//! A cache identity includes separately published evidence and quarantine.
use crate::store::{
    agent_log_integrity::TransactionLogError, agent_log_segments as segments,
    sandbox_mongo::MongoSandboxStore,
};
use layer2_core::sandbox::{store::SandboxStoreError, Agent};
use mongodb::{bson::doc, options::ReadConcern};
use sha2::{Digest, Sha256};

pub(super) async fn read(
    store: &MongoSandboxStore,
    agent: &Agent,
) -> Result<Option<Vec<u8>>, SandboxStoreError> {
    let mut session = store
        .client
        .start_session()
        .snapshot(true)
        .await
        .map_err(driver)?;
    let row = store
        .agents
        .find_one(doc! {"_id":agent.id.as_str()})
        .read_concern(ReadConcern::snapshot())
        .session(&mut session)
        .await
        .map_err(driver)?
        .ok_or(SandboxStoreError::NotFound)?;
    if crate::store::agents::read_agent(&row)? != *agent {
        return Ok(None);
    }
    let mut digest = Sha256::new();
    digest.update(row.to_vec().map_err(corrupt)?);
    if segments::read_format(&row)?.is_some() {
        // Reuse the current audit, head, seal-boundary and aggregate checks.
        // This reads bounded metadata, never folds the active source interval.
        let head = segments::read_head(store, &row, &mut session, true)
            .await
            .map_err(transaction)?;
        let projection = segments::projection::read(store, agent, &head, &mut session)
            .await
            .map_err(transaction)?;
        digest.update(serde_json::to_vec(&(head, projection)).map_err(corrupt)?);
    }
    Ok(Some(digest.finalize().to_vec()))
}

fn corrupt(error: impl std::fmt::Display) -> SandboxStoreError {
    SandboxStoreError::Corrupt(error.to_string())
}
fn driver(error: mongodb::error::Error) -> SandboxStoreError {
    SandboxStoreError::Unavailable(error.to_string())
}
fn transaction(error: TransactionLogError) -> SandboxStoreError {
    match error {
        TransactionLogError::Domain(error) => error,
        TransactionLogError::Driver(error) => driver(error),
    }
}
