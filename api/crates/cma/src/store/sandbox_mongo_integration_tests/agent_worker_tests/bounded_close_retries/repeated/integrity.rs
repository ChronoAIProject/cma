use super::contracts::{proposal, seeded};
use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn repeated_failure_full_and_bounded_proofs_refuse_damage() {
    for bounded in [false, true] {
        for damage in ["tail", "gap", "extra", "quarantine", "middle_audit"] {
            let mut f = seeded().await;
            let marker = f
                .store
                .agent_log_entry(&f.agent.id, f.agent.conversation.last_seq)
                .await
                .unwrap()
                .unwrap();
            let mut at = TWO_MINUTES;
            let mut due = "2026-09-05T00:03:00Z";
            if bounded {
                f.store
                    .commit_agent_work_repeated_failure(proposal(&f.agent, &marker, at, due), None)
                    .await
                    .unwrap();
                at = due;
                due = "2026-09-05T00:04:00Z";
                f.agent = claim(&f.store, &f.agent.id, at).await;
            }
            match damage {
                "tail" => {
                    f.store.agent_log.update_one(doc! {"agent_id": f.agent.id.as_str(), "seq": f.agent.conversation.last_seq as i64}, doc! {"$set": {"appended_at": NOW}}).await.unwrap();
                }
                "gap" => {
                    f.store.agent_log.delete_one(doc! {"agent_id": f.agent.id.as_str(), "seq": f.agent.conversation.last_seq as i64}).await.unwrap();
                }
                "extra" => {
                    let mut extra = marker.clone();
                    extra.seq += 1;
                    f.store
                        .agent_log
                        .insert_one(log_document(&extra).unwrap())
                        .await
                        .unwrap();
                }
                "quarantine" => {
                    f.change(doc! {"$set": {QUARANTINE_FIELD: {"format": 1_i64, "reason": "test", "observed_head": f.agent.conversation.last_seq as i64}}}).await;
                }
                "middle_audit" => {
                    f.store
                        .agent_log
                        .delete_one(doc! {"agent_id": f.agent.id.as_str(), "seq": 250_i64})
                        .await
                        .unwrap();
                    f.store.retry_audits.expire(&f.agent);
                }
                _ => unreachable!(),
            }
            let result = f
                .store
                .commit_agent_work_repeated_failure(proposal(&f.agent, &marker, at, due), None)
                .await;
            assert!(result.is_err(), "bounded={bounded} damage={damage}");
            assert_eq!(
                f.store.get_agent(&f.agent.id).await.unwrap(),
                Some(f.agent.clone())
            );
            f.cleanup().await;
        }
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn repeated_failure_missing_stale_or_untrusted_proofs_fully_scan() {
    for mode in ["off", "cohort_off", "missing", "stale", "untrusted"] {
        let mut f = seeded().await;
        let marker = f
            .store
            .agent_log_entry(&f.agent.id, f.agent.conversation.last_seq)
            .await
            .unwrap()
            .unwrap();
        f.store
            .commit_agent_work_repeated_failure(
                proposal(&f.agent, &marker, TWO_MINUTES, "2026-09-05T00:03:00Z"),
                None,
            )
            .await
            .unwrap();
        f.agent = claim(&f.store, &f.agent.id, "2026-09-05T00:03:00Z").await;
        match mode {
            "off" => f.store = f.store.with_bounded_close_retries(false),
            "cohort_off" => f.store.bounded_admission_agents.clear(),
            "missing" => f.change(doc! {"$unset": {INTEGRITY_FIELD: ""}}).await,
            "stale" => {
                f.change(doc! {"$inc": {"agent_log_integrity_v1.bound_agent_revision": -1_i64}})
                    .await
            }
            "untrusted" => {
                f.change(
                    doc! {"$set": {"agent_log_integrity_v1.semantic_checkpoint_v1.valid": false}},
                )
                .await
            }
            _ => unreachable!(),
        }
        scans::isolated_test(async {
            f.store
                .commit_agent_work_repeated_failure(
                    proposal(
                        &f.agent,
                        &marker,
                        "2026-09-05T00:03:00Z",
                        "2026-09-05T00:04:00Z",
                    ),
                    None,
                )
                .await
                .unwrap();
            assert_eq!(scans::snapshot(CALLER, true)[0], 1, "{mode}");
        })
        .await;
        f.cleanup().await;
    }
}
