//! Query the authoritative obligation with an explicitly installed index.
use futures_util::TryStreamExt;
use layer2_core::{sandbox::store::SandboxStoreError, AgentId};
use mongodb::{
    bson::{doc, Document},
    options::Hint,
};

use super::super::sandbox_mongo::MongoSandboxStore;

pub(crate) const PAGE_SIZE: u32 = 32;
pub(crate) const INDEX_NAME: &str = "agents_turn_policy_due";
const STATE: &str = "record.conversation.turn_timeout.disposition.state";
const DEADLINE: &str = "record.conversation.turn_timeout.deadline_at_ms";

pub(crate) fn index() -> Document {
    doc! {"name":INDEX_NAME,"key":{STATE:1, DEADLINE:1,"_id":1}}
}

#[derive(Clone, Debug)]
pub(crate) struct Candidate {
    pub id: AgentId,
    pub deadline_at_ms: i64,
}

pub(crate) async fn page(
    store: &MongoSandboxStore,
    after: Option<&Candidate>,
    now_ms: i64,
) -> Result<Vec<Candidate>, SandboxStoreError> {
    if !super::floor::retained(store).await? {
        return Ok(Vec::new());
    }
    let mut filter = doc! {STATE:"armed", DEADLINE:{"$lte":now_ms}};
    if let Some(after) = after {
        filter.insert(
            "$or",
            vec![
                doc! {DEADLINE:{"$gt":after.deadline_at_ms}},
                doc! {DEADLINE:after.deadline_at_ms,"_id":{"$gt":after.id.as_str()}},
            ],
        );
    }
    let rows: Vec<Document> = store
        .agents
        .find(filter)
        .hint(Hint::Name(INDEX_NAME.into()))
        .sort(doc! {DEADLINE:1,"_id":1})
        .projection(doc! {"_id":1, DEADLINE:1})
        .limit(i64::from(PAGE_SIZE))
        .batch_size(PAGE_SIZE)
        .max_time(std::time::Duration::from_secs(1))
        .await
        .map_err(super::unavailable)?
        .try_collect()
        .await
        .map_err(super::unavailable)?;
    rows.into_iter()
        .map(|row| {
            let invalid = |_| SandboxStoreError::Corrupt("invalid turn policy due row".into());
            let policy = row
                .get_document("record")
                .map_err(invalid)?
                .get_document("conversation")
                .map_err(invalid)?
                .get_document("turn_timeout")
                .map_err(invalid)?;
            Ok(Candidate {
                id: AgentId(row.get_str("_id").map_err(invalid)?.into()),
                deadline_at_ms: policy.get_i64("deadline_at_ms").map_err(invalid)?,
            })
        })
        .collect()
}
