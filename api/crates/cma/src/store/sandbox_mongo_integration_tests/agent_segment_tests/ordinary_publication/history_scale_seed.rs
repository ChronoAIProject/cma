//! Offline test-fixture construction: preserve all retained prefix BSON rows.
use super::history_scale::*;
use crate::store::agent_log::log_document;
use crate::store::agent_log_segments::{digest, Head, Seal, HEADS, SEGMENTS};
use futures_util::TryStreamExt;
use mongodb::bson::{self, RawDocumentBuf};
fn collection(store: &MongoSandboxStore, name: &str) -> mongodb::Collection<Document> {
    store
        .client
        .database(&store.agents.namespace().db)
        .collection(name)
}
pub(super) async fn install(
    store: &MongoSandboxStore,
    mut agent: Agent,
    entries: Vec<AgentLogEntry>,
    closed: i64,
    width: i64,
) -> (Agent, Vec<AgentLogEntry>) {
    let original = store
        .agents
        .find_one(doc! {"_id":agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    let retained_prefix = agent.conversation.last_seq as usize;
    store
        .agent_log
        .insert_many(
            entries
                .iter()
                .skip(retained_prefix)
                .map(|e| log_document(e).unwrap()),
        )
        .await
        .unwrap();
    let raw: Vec<RawDocumentBuf> = store
        .agent_log
        .clone_with_type()
        .find(doc! {"agent_id":agent.id.as_str()})
        .sort(doc! {"seq":1})
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    let incarnation = digest::incarnation(&agent);
    let mut previous = None;
    let mut prefix_bytes = 0;
    let mut seals = Vec::new();
    for ordinal in 0..closed {
        let start = ordinal * width + 1;
        let rows = &raw[(ordinal * width) as usize..((ordinal + 1) * width) as usize];
        let bytes = rows.iter().map(|r| r.as_bytes().len() as i64).sum::<i64>();
        prefix_bytes += bytes;
        let leaves = rows
            .iter()
            .enumerate()
            .flat_map(|(i, r)| {
                digest::leaf(
                    agent.id.as_str(),
                    &incarnation,
                    start + i as i64,
                    r.as_bytes(),
                )
                .bytes
            })
            .collect::<Vec<_>>();
        let mut seal = Seal {
            id: Seal::identity(agent.id.as_str(), &incarnation, ordinal),
            format_version: 1,
            agent_id: agent.id.0.clone(),
            incarnation: incarnation.clone(),
            ordinal,
            start_seq: start,
            end_seq: start + width - 1,
            entry_count: width,
            encoded_bytes: bytes,
            previous_seal: previous,
            prefix_count: start + width - 1,
            prefix_bytes,
            leaf_digests: digest::binary(leaves),
            seal_digest: digest::binary(vec![]),
            digest_version: digest::VERSION.into(),
            sealing_operation: format!("fixture:{ordinal}"),
            source_format: "legacy_v1".into(),
            storage_kind: "mongo".into(),
        };
        seal.seal_digest = seal.digest();
        seal.validate().unwrap();
        previous = Some(seal.seal_digest.clone());
        seals.push(bson::serialize_to_document(&seal).unwrap());
    }
    if !seals.is_empty() {
        collection(store, SEGMENTS)
            .insert_many(seals)
            .await
            .unwrap();
    }
    let active_bytes = raw[(closed * width) as usize..]
        .iter()
        .map(|r| r.as_bytes().len() as i64)
        .sum::<i64>();
    agent.conversation.last_seq = entries.len() as u64;
    let head = Head {
        agent_id: agent.id.0.clone(),
        format_version: 1,
        incarnation: incarnation.clone(),
        revision: 1,
        active_ordinal: closed,
        active_start_seq: closed * width + 1,
        last_seq: entries.len() as i64,
        active_count: entries.len() as i64 - closed * width,
        active_bytes,
        previous_seal: previous,
        lifetime_count: entries.len() as i64,
        lifetime_bytes: prefix_bytes + active_bytes,
        projection_format: 1,
        evidence_generation: "initial".into(),
        projection_through: entries.len() as i64,
        migration_id: segments::MIGRATION.into(),
        required_reader: segments::READER.into(),
    };
    head.validate().unwrap();
    collection(store, HEADS)
        .replace_one(
            doc! {"_id":agent.id.as_str()},
            bson::serialize_to_document(&head).unwrap(),
        )
        .await
        .unwrap();
    let mut row = original;
    for (key, value) in crate::store::agents::agent_document(&agent).unwrap() {
        row.insert(key, value);
    }
    row.insert("document_schema_version", 2_i64);
    row.insert("agent_log_format", segments::FORMAT);
    row.insert("agent_log_incarnation", incarnation);
    row.insert("agent_log_head_revision", 1_i64);
    store
        .agents
        .replace_one(doc! {"_id":agent.id.as_str()}, row)
        .await
        .unwrap();
    for name in [segments::PROJECTIONS, segments::EVIDENCE] {
        collection(store, name).delete_many(doc! {}).await.unwrap();
    }
    segments::mongo_tests::fixtures::certify(store, &agent, &entries).await;
    segments::mongo_tests::fixtures::certify_output(store, &agent, &entries).await;
    (agent, entries)
}
