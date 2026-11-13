use super::*;
use crate::store::agent_log_integrity::{INTEGRITY_FIELD, QUARANTINE_FIELD};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn title_prefix_keeps_disabled_missing_stale_and_unknown_metadata_on_full_reads() {
    for case in [
        "switch_off",
        "cohort_off",
        "missing",
        "stale",
        "unsupported",
    ] {
        let mut f = Fixture::new(1_000, 6, false, 64).await;
        let filter = doc! { "_id": f.agent.id.as_str() };
        let update = match case {
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
            "missing" => Some(doc! { "$unset": { INTEGRITY_FIELD: "" } }),
            "stale" => Some(doc! { "$inc": { "revision": 1_i64, "record.revision": 1_i64 } }),
            "unsupported" => Some(doc! { "$set": { "agent_log_integrity_v1.format": 2_i64 } }),
            _ => unreachable!(),
        };
        if let Some(update) = update {
            f.store
                .agents
                .update_one(filter.clone(), update)
                .await
                .unwrap();
        }
        let before_row = f
            .store
            .agents
            .find_one(filter.clone())
            .await
            .unwrap()
            .unwrap();
        let before = f.reset();
        reads::isolated_test(async {
            assert_eq!(
                f.store.agent_title_transcript(&f.agent.id).await.unwrap(),
                f.expected()
            );
            assert!(reads::snapshot(AGENT_LOG)[2] >= 1_000, "{case}");
        })
        .await;
        if matches!(case, "switch_off" | "cohort_off") {
            assert_eq!(
                f.used(before),
                6,
                "disabled reads keep their original query budget"
            );
        }
        eprintln!(
            "title_prefix_untrusted case={case} checkouts={}",
            f.used(before)
        );
        f.assert_snapshot_read_only();
        assert_eq!(
            f.store.agents.find_one(filter).await.unwrap().unwrap(),
            before_row
        );
        f.cleanup().await;
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn title_prefix_refuses_corrupt_proof_or_requested_rows_without_writes() {
    for case in [
        "quarantine",
        "ahead",
        "tail_hash",
        "missing_tail",
        "extra_tail",
        "prefix_gap",
        "prefix_codec",
        "creation",
    ] {
        let f = Fixture::new(1_000, 6, false, 64).await;
        let filter = doc! { "_id": f.agent.id.as_str() };
        match case {
            "quarantine" => {
                f.store
                    .agents
                    .update_one(filter.clone(), doc! { "$set": { QUARANTINE_FIELD: {} } })
                    .await
                    .unwrap();
            }
            "ahead" => {
                f.store
                    .agents
                    .update_one(
                        filter.clone(),
                        doc! { "$set": { "agent_log_integrity_v1.head_seq": 1_001_i64 } },
                    )
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
            "missing_tail" | "prefix_gap" => {
                f.store.agent_log.delete_one(doc! { "agent_id": f.agent.id.as_str(), "seq": if case == "missing_tail" { 1_000_i64 } else { 5_i64 } }).await.unwrap();
            }
            "extra_tail" => {
                let mut entry = f.entries.last().unwrap().clone();
                entry.seq += 1;
                f.store
                    .agent_log
                    .insert_one(log_document(&entry).unwrap())
                    .await
                    .unwrap();
            }
            "prefix_codec" => {
                f.store
                    .agent_log
                    .update_one(
                        doc! { "agent_id": f.agent.id.as_str(), "seq": 5_i64 },
                        doc! { "$set": { "document_schema_version": 99_i64 } },
                    )
                    .await
                    .unwrap();
            }
            "creation" => {
                f.store
                    .agent_log
                    .update_one(
                        doc! { "agent_id": f.agent.id.as_str(), "seq": 1_i64 },
                        doc! { "$set": { "record.kind.workspace_id": "wks_other" } },
                    )
                    .await
                    .unwrap();
            }
            _ => unreachable!(),
        }
        let before = f
            .store
            .agents
            .find_one(filter.clone())
            .await
            .unwrap()
            .unwrap();
        f.reset();
        let result = f.store.agent_title_transcript(&f.agent.id).await;
        assert!(
            matches!(
                result,
                Err(AgentTitleStoreError::Corrupt | AgentTitleStoreError::SequenceConflict)
            ),
            "{case}: {result:?}"
        );
        f.assert_snapshot_read_only();
        assert_eq!(
            f.store.agents.find_one(filter).await.unwrap().unwrap(),
            before,
            "{case}"
        );
        f.cleanup().await;
    }
}
