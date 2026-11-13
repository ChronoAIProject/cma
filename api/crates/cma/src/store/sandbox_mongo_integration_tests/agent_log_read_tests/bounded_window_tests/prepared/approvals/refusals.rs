use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn approval_reads_refuse_corrupt_proof_and_suffix_without_writes() {
    for mode in [
        "quarantine",
        "tail_hash",
        "extra_tail",
        "gap",
        "codec",
        "creation",
        "ahead",
    ] {
        let f = Fixture::new(1_000)
            .await
            .prepare()
            .await
            .approvals(false, false)
            .await;
        match mode {
            "quarantine" => {
                f.change_proof(doc! { "$set": { QUARANTINE_FIELD: {} }})
                    .await
            }
            "ahead" => {
                f.change_proof(doc! { "$set": { "agent_log_integrity_v1.head_seq": 1_001_i64 }})
                    .await
            }
            "tail_hash" => {
                f.store
                    .agent_log
                    .update_one(
                        doc! { "agent_id": f.id.as_str(), "seq": 1_000_i64 },
                        doc! { "$set": { "record.appended_at": TWO_MINUTES }},
                    )
                    .await
                    .unwrap();
            }
            "extra_tail" => {
                f.store
                    .agent_log
                    .insert_one(log_document(&entry(&f.id, 1_001)).unwrap())
                    .await
                    .unwrap();
            }
            "gap" => {
                f.store
                    .agent_log
                    .delete_one(doc! { "agent_id": f.id.as_str(), "seq": 997_i64 })
                    .await
                    .unwrap();
            }
            "codec" => {
                f.store
                    .agent_log
                    .update_one(
                        doc! { "agent_id": f.id.as_str(), "seq": 997_i64 },
                        doc! { "$set": { "record.agent_id": "agt_other" }},
                    )
                    .await
                    .unwrap();
            }
            "creation" => {
                let agent = f.store.get_agent(&f.id).await.unwrap().unwrap();
                f.replace(
                    997,
                    AgentLogKind::Created {
                        workspace_id: agent.workspace_id,
                        worktree_id: agent.worktree_id,
                    },
                )
                .await;
            }
            _ => unreachable!(),
        }
        assert_refusal_without_writes(&f, mode == "gap").await;
        f.cleanup().await;
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn every_approval_complete_fallback_refuses_gaps_without_quarantine() {
    for mode in [
        "missing",
        "stale",
        "unsupported",
        "switch_off",
        "cohort_off",
        "long",
        "bytes",
    ] {
        let mut f = Fixture::new(1_000)
            .await
            .prepare()
            .await
            .approvals(mode == "long", mode == "bytes")
            .await;
        match mode {
            "missing" => {
                f.change_proof(doc! { "$unset": { INTEGRITY_FIELD: "" }})
                    .await
            }
            "stale" => {
                f.change_proof(doc! { "$inc": { "revision": 1_i64, "record.revision": 1_i64 }})
                    .await
            }
            "unsupported" => {
                f.change_proof(doc! { "$set": { "agent_log_integrity_v1.format": 2_i64 }})
                    .await
            }
            "switch_off" => f.store = f.store.with_bounded_approval_reads(false),
            "cohort_off" => f.store.bounded_admission_agents.clear(),
            _ => {}
        }
        // Outside the suffix/tail proof: complete validation must see this gap.
        f.store
            .agent_log
            .delete_one(doc! { "agent_id": f.id.as_str(), "seq": 500_i64 })
            .await
            .unwrap();
        assert_refusal_without_writes(&f, true).await;
        f.cleanup().await;
    }
}

async fn assert_refusal_without_writes(f: &Fixture, gap: bool) {
    let filter = doc! { "_id": f.id.as_str() };
    let before = f
        .store
        .agents
        .find_one(filter.clone())
        .await
        .unwrap()
        .unwrap();
    f.reset_commands();
    let result = f.store.pending_agent_approvals(&f.id).await;
    if gap {
        assert_eq!(result, Err(SandboxStoreError::SequenceConflict));
    } else {
        assert!(
            matches!(result, Err(SandboxStoreError::Corrupt(_))),
            "{result:?}"
        );
    }
    f.assert_snapshot_reads();
    assert_eq!(
        f.store.agents.find_one(filter).await.unwrap().unwrap(),
        before
    );
}
