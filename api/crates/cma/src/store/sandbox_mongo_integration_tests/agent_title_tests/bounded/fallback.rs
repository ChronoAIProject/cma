//! Complete title reads refuse corruption without acting as integrity audits.
use super::*;
use crate::store::agent_log_integrity::{INTEGRITY_FIELD, QUARANTINE_FIELD};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn every_corrupt_title_fallback_refuses_without_any_durable_or_transaction_write() {
    for case in [
        "missing",
        "unsupported",
        "stale",
        "switch_off",
        "cohort_off",
        "long_turn",
        "byte_limit",
    ] {
        let (end, bytes) = match case {
            "long_turn" => (600, 64),
            "byte_limit" => (6, 4 * 1024 * 1024),
            _ => (6, 64),
        };
        let mut f = Fixture::new(1_000, end, false, bytes).await;
        let filter = doc! { "_id": f.agent.id.as_str() };
        let update = match case {
            "missing" => Some(doc! { "$unset": { INTEGRITY_FIELD: "" } }),
            "unsupported" => Some(doc! { "$set": { "agent_log_integrity_v1.format": 2_i64 } }),
            "stale" => Some(doc! { "$inc": { "revision": 1_i64, "record.revision": 1_i64 } }),
            "switch_off" => {
                f.store = f.store.with_bounded_title_reads(false);
                None
            }
            "cohort_off" => {
                f.store = MongoSandboxStore::open(&f.client, &f.name)
                    .await
                    .unwrap()
                    .with_bounded_title_reads(true);
                None
            }
            _ => None,
        };
        if let Some(update) = update {
            f.store.agents.update_one(filter, update).await.unwrap();
        }
        // The damaged row is outside both the bounded prefix and tail proof.
        // Every case must reach complete validation and refuse the same gap.
        f.store
            .agent_log
            .delete_one(doc! { "agent_id": f.agent.id.as_str(), "seq": 500_i64 })
            .await
            .unwrap();
        assert_read_only_refusal(&f, AgentTitleStoreError::SequenceConflict).await;
        f.cleanup().await;
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn disabled_title_fallback_preserves_proof_lineage_codec_and_capacity_refusals() {
    for case in [
        "quarantine",
        "tail_hash",
        "extra_tail",
        "lineage",
        "codec",
        "capacity",
    ] {
        let mut f = Fixture::new(1_000, 6, false, 64).await;
        f.store = f.store.with_bounded_title_reads(false);
        let filter = doc! { "_id": f.agent.id.as_str() };
        match case {
            "quarantine" => {
                f.store
                    .agents
                    .update_one(filter, doc! { "$set": { QUARANTINE_FIELD: {} } })
                    .await
                    .unwrap();
            }
            "tail_hash" => {
                f.store
                    .agent_log
                    .update_one(
                        doc! { "agent_id": f.agent.id.as_str(), "seq": 1_000_i64 },
                        doc! { "$set": { "record.appended_at": TWO_MINUTES } },
                    )
                    .await
                    .unwrap();
            }
            "extra_tail" => {
                let mut extra = f.entries.last().unwrap().clone();
                extra.seq += 1;
                f.store
                    .agent_log
                    .insert_one(log_document(&extra).unwrap())
                    .await
                    .unwrap();
            }
            "lineage" => {
                f.store
                    .agent_log
                    .update_one(
                        doc! { "agent_id": f.agent.id.as_str(), "seq": 1_i64 },
                        doc! { "$set": { "record.kind.workspace_id": "wks_other" } },
                    )
                    .await
                    .unwrap();
            }
            "codec" => {
                f.store
                    .agent_log
                    .update_one(
                        doc! { "agent_id": f.agent.id.as_str(), "seq": 500_i64 },
                        doc! { "$set": { "record.agent_id": "agt_other" } },
                    )
                    .await
                    .unwrap();
            }
            "capacity" => {
                f.store.agents.update_one(filter, doc! {
                "$set": { "last_seq": 10_129_i64, "record.conversation.last_seq": 10_129_i64 },
                "$unset": { INTEGRITY_FIELD: "" }
            }).await.unwrap();
            }
            _ => unreachable!(),
        }
        let expected = if case == "capacity" {
            AgentTitleStoreError::AgentLogCapacityExceeded
        } else {
            AgentTitleStoreError::Corrupt
        };
        assert_read_only_refusal(&f, expected).await;
        f.cleanup().await;
    }
}

async fn assert_read_only_refusal(f: &Fixture, expected: AgentTitleStoreError) {
    let filter = doc! { "_id": f.agent.id.as_str() };
    let before = f
        .store
        .agents
        .find_one(filter.clone())
        .await
        .unwrap()
        .unwrap();
    for candidates in [false, true] {
        f.reset();
        if candidates {
            assert_eq!(
                f.store
                    .agent_title_candidates(&PageRequest {
                        cursor: None,
                        limit: 1
                    })
                    .await,
                Err(expected)
            );
        } else {
            assert_eq!(
                f.store.agent_title_transcript(&f.agent.id).await,
                Err(expected)
            );
        }
        f.assert_snapshot_read_only();
        assert_eq!(
            f.store
                .agents
                .find_one(filter.clone())
                .await
                .unwrap()
                .unwrap(),
            before,
            "refused readers must neither quarantine nor repair the captured Agent"
        );
    }
}
