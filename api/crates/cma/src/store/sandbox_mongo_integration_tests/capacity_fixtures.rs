//! Exact cumulative BSON and entry budgets shared by capacity contracts.
use super::*;
use crate::store::agent_log_integrity::{NORMAL_AGENT_LOG_BYTES, NORMAL_AGENT_LOG_ENTRIES};
use futures_util::TryStreamExt;
use sha2::{Digest, Sha256};

pub(crate) async fn fill_normal_capacity(
    store: &MongoSandboxStore,
    agent: Agent,
    bytes: bool,
) -> Agent {
    fill_to(
        store,
        agent,
        if bytes {
            None
        } else {
            Some(NORMAL_AGENT_LOG_ENTRIES)
        },
        if bytes {
            Some(NORMAL_AGENT_LOG_BYTES)
        } else {
            None
        },
    )
    .await
}

pub(crate) async fn fill_to(
    store: &MongoSandboxStore,
    mut agent: Agent,
    count: Option<u64>,
    byte_limit: Option<usize>,
) -> Agent {
    let mut existing = store
        .agent_log
        .find(doc! {"agent_id": agent.id.as_str()})
        .await
        .unwrap();
    let mut used = 0;
    while let Some(doc) = existing.try_next().await.unwrap() {
        used += doc.to_vec().unwrap().len();
    }
    let mut documents = Vec::new();
    while byte_limit.map_or_else(
        || agent.conversation.last_seq < count.unwrap(),
        |limit| used < limit,
    ) {
        agent.conversation.last_seq += 1;
        let mut entry = AgentLogEntry {
            agent_id: agent.id.clone(),
            seq: agent.conversation.last_seq,
            appended_at: NOW.into(),
            kind: AgentLogKind::ToolOutputReported {
                call_id: "retained-capacity-fixture".into(),
                output: layer2_core::sandbox::RecordedToolOutput {
                    text: "Retained tool output".into(),
                    truncated: false,
                },
            },
        };
        if let Some(limit) = byte_limit {
            entry.kind = AgentLogKind::ToolOutputReported {
                call_id: "retained-capacity-fixture".into(),
                output: layer2_core::sandbox::RecordedToolOutput {
                    text: String::new(),
                    truncated: false,
                },
            };
            let overhead = log_document(&entry).unwrap().to_vec().unwrap().len();
            let remaining = limit - used;
            let size = if remaining <= 16 * 1024 + overhead {
                remaining
            } else {
                16 * 1024
            };
            entry.kind = AgentLogKind::ToolOutputReported {
                call_id: "retained-capacity-fixture".into(),
                output: layer2_core::sandbox::RecordedToolOutput {
                    text: "x".repeat(size - overhead),
                    truncated: false,
                },
            };
        }
        let document = log_document(&entry).unwrap();
        used += document.to_vec().unwrap().len();
        documents.push(document);
    }
    if let Some(limit) = byte_limit {
        assert_eq!(used, limit);
    }
    store.agent_log.insert_many(documents).await.unwrap();
    let mut fields = agent_replacement_fields(&agent).unwrap();
    fields.insert(
        "last_seq",
        i64::try_from(agent.conversation.last_seq).unwrap(),
    );
    store
        .agents
        .update_one(doc! {"_id": agent.id.as_str()}, doc! {"$set":fields})
        .await
        .unwrap();
    agent
}

pub(crate) async fn fingerprint(store: &MongoSandboxStore, id: &AgentId, through: u64) -> String {
    let mut cursor = store
        .agent_log
        .find(doc! {"agent_id":id.as_str(), "seq":{"$lte":i64::try_from(through).unwrap()}})
        .sort(doc! {"seq":1})
        .await
        .unwrap();
    let mut hash = Sha256::new();
    while let Some(document) = cursor.try_next().await.unwrap() {
        hash.update(document.to_vec().unwrap());
    }
    format!("{:x}", hash.finalize())
}
