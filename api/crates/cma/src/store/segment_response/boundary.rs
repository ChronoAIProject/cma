//! Find only a coordinate; the shared interval reader still proves its source.
use layer2_core::{ids::AgentId, sandbox::store::SandboxStoreError};
use mongodb::{
    bson::doc,
    options::{Hint, ReadConcern},
    ClientSession,
};

use crate::store::{
    mongo_read_telemetry::Read,
    sandbox_mongo::{signed, MongoSandboxStore},
};

pub(super) async fn next_turn(
    store: &MongoSandboxStore,
    agent_id: &AgentId,
    turn: u64,
    through: u64,
    session: &mut ClientSession,
) -> Result<Option<u64>, SandboxStoreError> {
    if turn == through {
        return Ok(None);
    }
    let mut read = Read::find(store.agent_log.name());
    let mut cursor = store
        .agent_log
        .find(doc! {
            "agent_id": agent_id.as_str(),
            "seq": {
                "$gt": signed(turn, "response turn").map_err(SandboxStoreError::Corrupt)?,
                "$lte": signed(through, "response boundary").map_err(SandboxStoreError::Corrupt)?,
            },
            "record.kind.type": "turn_started",
        })
        // Pin the existing sequence index: even without a witness the server
        // examines at most the bounded candidate interval, never lifetime rows.
        .hint(Hint::Name("agent_log_sequence".into()))
        .sort(doc! { "seq": 1 })
        .projection(doc! { "_id": 0, "seq": 1 })
        .limit(1)
        .batch_size(1)
        .read_concern(ReadConcern::snapshot())
        .session(&mut *session)
        .await
        .map_err(unavailable)?;
    let next = read
        .next_in_session(&mut cursor, session)
        .await
        .map_err(unavailable)?
        .map(|row| {
            row.get_i64("seq")
                .ok()
                .and_then(|seq| u64::try_from(seq).ok())
                .filter(|seq| *seq > turn && *seq <= through)
                .ok_or(SandboxStoreError::SequenceConflict)
        })
        .transpose()?;
    read.complete();
    Ok(next)
}

fn unavailable(error: mongodb::error::Error) -> SandboxStoreError {
    SandboxStoreError::Unavailable(error.to_string())
}
