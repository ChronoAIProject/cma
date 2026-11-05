use super::*;
use crate::store::agent_log_segments::{self as segments, digest, Head, Seal};
use futures_util::TryStreamExt;
use mongodb::bson::{self, RawDocumentBuf};

pub(super) async fn segmented(
    store: &MongoSandboxStore,
    closed: i64,
) -> (Agent, Vec<AgentLogEntry>) {
    let (_, agent) = worktree_and_agent(&SandboxId("sbx_segments".into()));
    let entries: Vec<_> = (1..=closed * 2 + 2)
        .map(|seq| AgentLogEntry {
            agent_id: agent.id.clone(),
            seq: seq as u64,
            appended_at: NOW.into(),
            kind: if seq == 1 {
                AgentLogKind::Created {
                    workspace_id: agent.workspace_id.clone(),
                    worktree_id: agent.worktree_id.clone(),
                }
            } else {
                AgentLogKind::ModeChanged {
                    mode: "code".into(),
                }
            },
        })
        .collect();
    install(store, agent, entries, closed).await
}

async fn install(
    store: &MongoSandboxStore,
    mut agent: Agent,
    entries: Vec<AgentLogEntry>,
    closed: i64,
) -> (Agent, Vec<AgentLogEntry>) {
    store.create_agent(agent.clone()).await.unwrap();
    store
        .agent_log
        .insert_many(entries.iter().map(|e| log_document(e).unwrap()))
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
        let start = ordinal * 2 + 1;
        let rows = &raw[(ordinal * 2) as usize..(ordinal * 2 + 2) as usize];
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
            end_seq: start + 1,
            entry_count: 2,
            encoded_bytes: bytes,
            previous_seal: previous,
            prefix_count: start + 1,
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
        store.segment_logs.seals.insert_many(seals).await.unwrap();
    }
    let active_bytes = raw[(closed * 2) as usize..]
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
        active_start_seq: closed * 2 + 1,
        last_seq: entries.len() as i64,
        active_count: entries.len() as i64 - closed * 2,
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
    let mut state = layer2_core::sandbox::AgentLogProjectionState::default();
    for entry in &entries {
        let queued = layer2_core::sandbox::AgentLogProjectionState::queued_owner(entry);
        for change in state.observe(&agent, entry, queued).unwrap() {
            let row = segments::projection::plan(&head, change).unwrap();
            store
                .segment_logs
                .evidence
                .replace_one(doc! {"_id":row.get_str("_id").unwrap()}, row.clone())
                .upsert(true)
                .await
                .unwrap();
        }
    }
    state.validate_aggregate(&agent).unwrap();
    store
        .segment_logs
        .projections
        .insert_one(
            bson::serialize_to_document(&segments::projection::Projection {
                agent_id: agent.id.0.clone(),
                format_version: 1,
                incarnation: incarnation.clone(),
                through_seq: head.last_seq,
                head_revision: head.revision,
                evidence_generation: head.evidence_generation.clone(),
                record: state,
            })
            .unwrap(),
        )
        .await
        .unwrap();
    store
        .segment_logs
        .heads
        .insert_one(bson::serialize_to_document(&head).unwrap())
        .await
        .unwrap();
    let mut row = crate::store::agents::agent_document(&agent).unwrap();
    row.insert("document_schema_version", 2_i64);
    row.insert("agent_log_format", segments::FORMAT);
    row.insert("agent_log_incarnation", incarnation);
    row.insert("agent_log_head_revision", 1_i64);
    store
        .agents
        .replace_one(doc! {"_id":agent.id.as_str()}, row)
        .await
        .unwrap();
    (agent, entries)
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn segmented_ranges_replay_original_rows_across_boundaries_and_frozen_heads() {
    let (client, name) = database("segment_range").await;
    segments::migrate(&client.database(&name)).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (agent, entries) = segmented(&store, 3).await;
    assert_eq!(
        store.get_agent(&agent.id).await.unwrap(),
        Some(agent.clone())
    );
    for after in 0..=8 {
        for through in after..=8 {
            for limit in [1, 2, 4, 20, 100, 200] {
                let expected =
                    &entries[after as usize..(through.min(after + u64::from(limit))) as usize];
                assert_eq!(
                    store
                        .agent_log_range(&agent.id, after, through, limit)
                        .await
                        .unwrap(),
                    expected
                );
            }
        }
    }
    for entry in &entries {
        assert_eq!(
            store.agent_log_entry(&agent.id, entry.seq).await.unwrap(),
            Some(entry.clone())
        );
    }
    assert!(store.agent_log_entry(&agent.id, 9).await.unwrap().is_none());
    assert!(store
        .agent_log(&agent.id, u64::MAX, 200)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(store.agent_log_prefix(&agent.id, 8).await.unwrap(), entries);
    // Fitting future append changes no captured cursor or immutable prefix.
    let next = AgentLogEntry {
        seq: 9,
        ..entries[7].clone()
    };
    let row = log_document(&next).unwrap();
    let bytes = bson::serialize_to_vec(&row).unwrap().len() as i64;
    store.agent_log.insert_one(row).await.unwrap();
    store.segment_logs.heads.update_one(doc! {"_id":agent.id.as_str()}, doc! {"$inc":{"revision":1_i64,"last_seq":1_i64,"active_count":1_i64,"active_bytes":bytes,"lifetime_count":1_i64,"lifetime_bytes":bytes,"projection_through":1_i64}}).await.unwrap();
    store.agents.update_one(doc! {"_id":agent.id.as_str()}, doc! {"$inc":{"last_seq":1_i64,"record.conversation.last_seq":1_i64,"agent_log_head_revision":1_i64}}).await.unwrap();
    assert_eq!(
        store.agent_log_range(&agent.id, 3, 8, 4).await.unwrap(),
        entries[3..7]
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn segmented_ranges_refuse_corruption_missing_sidecars_and_uncommitted_tail() {
    let (client, name) = database("segment_corrupt").await;
    segments::migrate(&client.database(&name)).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (agent, entries) = segmented(&store, 3).await;
    for seq in [1, 3, 6, 8] {
        let filter = doc! {"agent_id":agent.id.as_str(),"seq":seq};
        let old = store
            .agent_log
            .find_one(filter.clone())
            .await
            .unwrap()
            .unwrap();
        store.agent_log.delete_one(filter.clone()).await.unwrap();
        assert!(store
            .agent_log_range(&agent.id, (seq - 1) as u64, seq as u64, 1)
            .await
            .is_err());
        store.agent_log.insert_one(old.clone()).await.unwrap();
        store
            .agent_log
            .update_one(
                filter.clone(),
                doc! {"$set":{"record.appended_at":"2026-10-01T00:00:00Z"}},
            )
            .await
            .unwrap();
        if seq <= 6 {
            assert!(store
                .agent_log_range(&agent.id, (seq - 1) as u64, seq as u64, 1)
                .await
                .is_err());
        }
        store.agent_log.replace_one(filter, old).await.unwrap();
    }
    let head = store
        .segment_logs
        .heads
        .find_one(doc! {"_id":agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    store
        .segment_logs
        .heads
        .delete_one(doc! {"_id":agent.id.as_str()})
        .await
        .unwrap();
    assert!(store.agent_log_range(&agent.id, 8, 8, 1).await.is_err());
    store.segment_logs.heads.insert_one(head).await.unwrap();
    for ordinal in 0..3_i64 {
        let filter = doc! {"agent_id":agent.id.as_str(),"ordinal":ordinal};
        let seal = store
            .segment_logs
            .seals
            .find_one(filter.clone())
            .await
            .unwrap()
            .unwrap();
        store.segment_logs.seals.delete_one(filter).await.unwrap();
        assert!(store.agent_log_range(&agent.id, 2, 4, 2).await.is_err());
        store.segment_logs.seals.insert_one(seal).await.unwrap();
    }
    let extra = AgentLogEntry {
        seq: 9,
        ..entries[7].clone()
    };
    store
        .agent_log
        .insert_one(log_document(&extra).unwrap())
        .await
        .unwrap();
    assert!(store.agent_log_range(&agent.id, 0, 1, 1).await.is_err());
    assert!(store.agent_log_range(&agent.id, 8, 8, 1).await.is_err());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn segment_schema_is_additive_restartable_and_refuses_drift() {
    let (client, name) = database("segment_schema").await;
    let db = client.database(&name);
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    segments::migrate(&db).await.unwrap();
    segments::migrate(&db).await.unwrap();
    let (agent, _) = segmented(&store, 1).await;
    MongoSandboxStore::open(&client, &name).await.unwrap();
    let before = store
        .agents
        .find_one(doc! {"_id":agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    let fields = agent_replacement_fields(&agent).unwrap();
    store
        .agents
        .update_one(doc! {"_id":agent.id.as_str()}, doc! {"$set":fields})
        .await
        .unwrap();
    assert_eq!(
        store
            .agents
            .find_one(doc! {"_id":agent.id.as_str()})
            .await
            .unwrap()
            .unwrap(),
        before
    );
    db.collection::<Document>(segments::SEGMENTS)
        .drop_index("agent_log_segment_start")
        .await
        .unwrap();
    assert!(MongoSandboxStore::open(&client, &name).await.is_err());
    segments::migrate(&db).await.unwrap();
    db.run_command(doc! {"collMod":segments::HEADS,"validationLevel":"off"})
        .await
        .unwrap();
    assert!(segments::migrate(&db).await.is_err());
    db.drop().await.unwrap();
}

mod audit_tests;
mod bounds_tests;
mod command_tests;
mod consumer_tests;
mod current_state_tests;
mod provenance_tests;
mod publication_tests;
mod rebuild_tests;
mod receipt_tests;
mod restore_tests;
mod retention_tests;
mod suggestion_tests;
mod task_status_tests;
