use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn close_retry_keeps_all_publication_fences_and_rolls_back() {
    let f = Fixture::new(500).await;
    let original = f.row().await;
    for mode in [
        "holder",
        "fence",
        "revision",
        "expired",
        "owner",
        "worktree",
        "sequence",
        "receipt",
        "stop_intent",
    ] {
        let mut mutation = retry(&f.agent);
        match mode {
            "holder" => mutation.holder = "stale-worker".into(),
            "fence" => mutation.fence += 1,
            "revision" => mutation.expected_agent_revision += 1,
            "expired" => {
                mutation.appended_at = "2026-09-05T00:11:00Z".into();
                mutation.agent.updated_at = mutation.appended_at.clone();
            }
            "owner" => mutation.agent.owner = SubjectId::new("different-owner"),
            "worktree" => mutation.agent.worktree_id = WorktreeId("wkt_other".into()),
            "sequence" => mutation.agent.conversation.last_seq += 1,
            "receipt" => mutation.agent.work.receipts.clear(),
            "stop_intent" => mutation.agent.requested_stop_reason = None,
            _ => unreachable!(),
        }
        assert!(f.store.commit_agent_work(mutation).await.is_err(), "{mode}");
        assert_eq!(f.row().await, original, "{mode}");
        assert_eq!(
            f.store
                .agent_log
                .count_documents(doc! { "agent_id": f.agent.id.as_str() })
                .await
                .unwrap(),
            500
        );
    }
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn close_retry_proof_corruption_refuses_and_keeps_durable_quarantine() {
    for mode in ["quarantine", "tail", "extra", "ahead", "identity"] {
        let f = Fixture::new(500).await;
        match mode {
            "quarantine" => f.change(doc! { "$set": { QUARANTINE_FIELD: {} }}).await,
            "ahead" => {
                f.change(doc! { "$set": { "agent_log_integrity_v1.head_seq": 501_i64 }})
                    .await
            }
            "identity" => {
                f.change(doc! { "$set": { "agent_log_integrity_v1.history_identity": mongodb::bson::Binary { subtype: mongodb::bson::spec::BinarySubtype::Generic, bytes: vec![7; 32] } }})
                    .await
            }
            "tail" => {
                f.store
                    .agent_log
                    .update_one(
                        doc! { "agent_id": f.agent.id.as_str(), "seq": 500_i64 },
                        doc! { "$set": { "record.appended_at": TWO_MINUTES }},
                    )
                    .await
                    .unwrap();
            }
            "extra" => {
                f.store
                    .agent_log
                    .insert_one(
                        log_document(&AgentLogEntry {
                            agent_id: f.agent.id.clone(),
                            seq: 501,
                            appended_at: MINUTE.into(),
                            kind: retry(&f.agent).agent_logs.remove(0),
                        })
                        .unwrap(),
                    )
                    .await
                    .unwrap();
            }
            _ => unreachable!(),
        }
        let before = f.row().await;
        assert!(
            matches!(
                f.store.commit_agent_work(retry(&f.agent)).await,
                Err(SandboxStoreError::Corrupt(_))
            ),
            "{mode}"
        );
        let mut after = f.row().await;
        let marker = after.remove(QUARANTINE_FIELD);
        assert!(marker.is_some());
        let mut before = before;
        before.remove(QUARANTINE_FIELD);
        assert_eq!(
            after, before,
            "only the existing durable quarantine may change"
        );
        f.cleanup().await;
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn close_retry_full_fallback_audits_middle_damage() {
    for mode in ["off", "cohort_off", "missing", "stale"] {
        let mut f = Fixture::new(500).await;
        match mode {
            "off" => f.store = f.store.with_bounded_close_retries(false),
            "cohort_off" => f.store.bounded_admission_agents.clear(),
            "missing" => f.change(doc! { "$unset": { INTEGRITY_FIELD: "" }}).await,
            "stale" => {
                f.change(doc! { "$inc": { "agent_log_integrity_v1.bound_agent_revision": -1_i64 }})
                    .await
            }
            _ => unreachable!(),
        }
        f.store
            .agent_log
            .delete_one(doc! { "agent_id": f.agent.id.as_str(), "seq": 250_i64 })
            .await
            .unwrap();
        assert_eq!(
            f.store.commit_agent_work(retry(&f.agent)).await,
            Err(SandboxStoreError::SequenceConflict),
            "{mode}"
        );
        assert!(f.row().await.contains_key(QUARANTINE_FIELD));
        f.cleanup().await;
    }
}
