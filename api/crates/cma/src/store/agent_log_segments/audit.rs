//! Fenced scrub: snapshot source, bounded staged writes, resumable promotion.
use layer2_core::sandbox::{store::SandboxStoreError, Agent, AgentLogProjectionState};
use mongodb::{
    bson::{self, doc, Binary, Document},
    Client, ClientSession, Database,
};
use serde::{Deserialize, Serialize};

use super::{corrupt, digest, Head, AUDITS, EVIDENCE, HEADS, MAX_BYTES, MAX_ENTRIES};
use crate::store::{
    durability::majority_collection,
    sandbox_mongo::{commit_transaction, start_transaction, CommitOutcome},
};

mod collect;
mod persistence;
mod progress;
mod rebuild;
pub(in crate::store) use rebuild::install;

const LEASE_MS: i64 = 30_000;
const MAX_PROGRESS_BYTES: usize = 1024 * 1024;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Checkpoint {
    next_ordinal: i64,
    checked_entries: i64,
    checked_bytes: i64,
    #[serde(deserialize_with = "super::required_nullable")]
    previous_seal: Option<Binary>,
    semantic: AgentLogProjectionState,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case", deny_unknown_fields)]
enum Stage {
    Collecting {
        generation: String,
    },
    Promoting {
        generation: String,
        completed: Box<Checkpoint>,
        tail: bool,
        #[serde(deserialize_with = "super::required_nullable")]
        cursor: Option<String>,
    },
}
impl Stage {
    fn generation(&self) -> &str {
        match self {
            Self::Collecting { generation } | Self::Promoting { generation, .. } => generation,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Progress {
    #[serde(rename = "_id")]
    id: String,
    format_version: i64,
    agent_id: String,
    incarnation: String,
    audit_id: String,
    holder: String,
    expires_at_ms: i64,
    fence: i64,
    step: i64,
    status: String,
    through_seq: i64,
    head_revision: i64,
    result_generation: String,
    captured_head: Head,
    captured_agent: Agent,
    completed: Checkpoint,
    #[serde(deserialize_with = "super::required_nullable")]
    stage: Option<Stage>,
}

fn collection(database: &Database, name: &str) -> mongodb::Collection<Document> {
    majority_collection(database, name)
}
fn unavailable(error: mongodb::error::Error) -> SandboxStoreError {
    SandboxStoreError::Unavailable(format!("Agent log audit storage: {error}"))
}
fn now() -> i64 {
    bson::DateTime::now().timestamp_millis()
}
fn id(agent: &str, audit: &str) -> String {
    format!("{}:{agent}{}:{audit}", agent.len(), audit.len())
}

async fn commit(session: &mut ClientSession) -> Result<(), SandboxStoreError> {
    match commit_transaction(session).await {
        CommitOutcome::Committed => Ok(()),
        _ => Err(SandboxStoreError::Unavailable(
            "Agent log audit commit unresolved; resume the same audit ID".into(),
        )),
    }
}

/// No runtime lease, source writes or live generation changes. Each invocation
/// validates at most one physical unit; resumed promotion never rescans source.
pub(crate) async fn run_unit(
    client: &Client,
    database: &Database,
    agent: &str,
    audit: &str,
    cancel: bool,
) -> Result<String, SandboxStoreError> {
    let mut progress = progress::claim(client, database, agent, audit, cancel).await?;
    if progress.status != "running" {
        return Ok(progress.status);
    }
    let result = async {
        if !matches!(progress.stage, Some(Stage::Promoting { .. })) {
            collect::run(client, database, &mut progress).await?;
        }
        persistence::promote(client, database, &mut progress).await
    }
    .await;
    if matches!(
        result,
        Err(SandboxStoreError::Corrupt(_) | SandboxStoreError::SequenceConflict)
    ) {
        progress.status = "corrupt".into();
        progress.stage = None;
        persistence::write(
            client,
            database,
            &mut progress,
            &persistence::Batch::default(),
        )
        .await?;
        return Ok(progress.status);
    }
    result?;
    Ok(progress.status)
}

impl Progress {
    fn head(&self, generation: &str) -> Head {
        let mut head = self.captured_head.clone();
        head.evidence_generation = generation.into();
        head
    }
    fn document(&self) -> Result<Document, SandboxStoreError> {
        let row = bson::serialize_to_document(self).map_err(|_| corrupt())?;
        if bson::serialize_to_vec(&row).map_err(|_| corrupt())?.len() > MAX_PROGRESS_BYTES {
            return Err(corrupt());
        }
        Ok(row)
    }
    fn filter(&self, at: i64) -> Document {
        doc! {"_id":&self.id,"format_version":2_i64,"holder":&self.holder,"fence":self.fence,
        "step":self.step,"status":"running","expires_at_ms":{"$gt":at}}
    }
}

#[cfg(test)]
pub(in crate::store) mod test_support;
