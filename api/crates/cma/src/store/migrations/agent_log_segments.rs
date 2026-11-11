//! Additive reader-format migration. Registration and ledger live in the runner.
use mongodb::Database;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::store::agent_log_segments::{schema, COLLECTIONS, MIGRATION, READER};

pub(super) const ID: &str = MIGRATION;

pub(super) fn checksum() -> String {
    let mut digest = Sha256::new();
    digest.update(include_bytes!("agent_log_segments.rs"));
    digest.update(include_bytes!("../agent_log_segments/schema.rs"));
    format!("{:x}", digest.finalize())
}

pub(super) async fn inspect(database: &Database) -> Result<Value, &'static str> {
    let pending = schema::pending_count(database)
        .await
        .map_err(|_| "segment schema drift or unavailable")?;
    Ok(
        json!({"migration":ID,"checksum":checksum(),"phase":"expand",
        "required_reader":READER,"collections":COLLECTIONS,"pending_steps":pending,
        "effects":"add sidecar collections, validators and indexes only",
        "activates_agents":false,"rewrites_events":false,"writes":false}),
    )
}

pub(super) async fn apply_next(database: &Database) -> Result<bool, &'static str> {
    schema::apply_next(database)
        .await
        .map_err(|_| "segment DDL outcome uncertain; inspect and resume")
}
