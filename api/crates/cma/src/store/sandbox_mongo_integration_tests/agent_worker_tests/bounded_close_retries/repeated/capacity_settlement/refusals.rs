use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn capacity_settlement_refuses_corrupt_history_and_stale_authority() {
    for damage in [
        "tail",
        "missing_tail",
        "extra",
        "quarantine",
        "malformed",
        "prefix_audit",
        "middle_audit",
        "truncation_audit",
        "holder",
        "fence",
        "revision",
        "expired",
        "marker",
    ] {
        let mut f = seeded(AgentWorkStep::CloseProcess).await;
        warm(&mut f).await;
        let mut mutation = proposal(&f, THIRD, FOURTH).await;
        match damage {
            "tail" => {
                f.store
                    .agent_log
                    .update_one(
                        doc! {"agent_id": f.agent.id.as_str(), "seq": 10_001_i64},
                        doc! {"$set": {"appended_at": NOW}},
                    )
                    .await
                    .unwrap();
            }
            "missing_tail" => {
                f.store
                    .agent_log
                    .delete_one(doc! {"agent_id": f.agent.id.as_str(), "seq": 10_001_i64})
                    .await
                    .unwrap();
            }
            "extra" => {
                let mut extra = f
                    .store
                    .agent_log_entry(&f.agent.id, 10_001)
                    .await
                    .unwrap()
                    .unwrap();
                extra.seq += 1;
                f.store
                    .agent_log
                    .insert_one(log_document(&extra).unwrap())
                    .await
                    .unwrap();
            }
            "quarantine" => {
                f.change(doc! {"$set": {QUARANTINE_FIELD: {
                "format": 1_i64, "reason": "test", "observed_head": 10_001_i64}}})
                    .await
            }
            "malformed" => f.change(doc! {"$set": {INTEGRITY_FIELD: "invalid"}}).await,
            "prefix_audit" | "middle_audit" => {
                let seq = if damage == "prefix_audit" {
                    1_i64
                } else {
                    250_i64
                };
                f.store
                    .agent_log
                    .delete_one(doc! {"agent_id": f.agent.id.as_str(), "seq": seq})
                    .await
                    .unwrap();
                f.store.retry_audits.expire(&f.agent);
            }
            "truncation_audit" => {
                f.store
                    .agent_log
                    .delete_many(doc! {"agent_id": f.agent.id.as_str(), "seq": {"$gt": 9_990_i64}})
                    .await
                    .unwrap();
                f.store.retry_audits.expire(&f.agent);
            }
            "holder" => mutation.holder = "other".into(),
            "fence" => mutation.fence += 1,
            "revision" => mutation.expected_agent_revision -= 1,
            "expired" => {
                mutation.appended_at = "2026-09-05T00:10:00Z".into();
                mutation.agent.updated_at = mutation.appended_at.clone();
                mutation.agent.work.ready_at = "2026-09-05T00:11:00Z".into();
            }
            "marker" => {
                // A validly published different tail must never stand in for
                // the exact retained capacity marker of this zero-entry retry.
                let mut tail = f
                    .store
                    .agent_log_entry(&f.agent.id, 10_001)
                    .await
                    .unwrap()
                    .unwrap();
                tail.kind = AgentLogKind::WorkStepFailed {
                    step: AgentWorkStep::EnsureProcessClosed,
                    failure: TypedFailure {
                        class: "agent_log_capacity".into(),
                        detail: "different marker".into(),
                    },
                };
                f.store
                    .agent_log
                    .replace_one(
                        doc! {"agent_id": f.agent.id.as_str(), "seq": 10_001_i64},
                        log_document(&tail).unwrap(),
                    )
                    .await
                    .unwrap();
                // Force the complete path to prove the refusal also comes from
                // marker semantics, independently of trusted-tail hash checks.
                f.change(doc! {"$unset": {INTEGRITY_FIELD: ""}}).await;
            }
            _ => unreachable!(),
        }
        let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
        let metrics = recorder.handle();
        let observation = metrics::set_default_local_recorder(&recorder);
        assert!(
            f.store.commit_agent_work(mutation).await.is_err(),
            "{damage}"
        );
        drop(observation);
        if matches!(
            damage,
            "tail" | "missing_tail" | "extra" | "quarantine" | "malformed"
        ) {
            decision(&metrics.render(), f.agent.work.step, "proof_error", 1);
        }
        assert_eq!(
            f.store.get_agent(&f.agent.id).await.unwrap(),
            Some(f.agent.clone())
        );
        f.cleanup().await;
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn capacity_settlement_unknown_commit_keeps_complete_receipt_reconciliation() {
    let mut f = seeded(AgentWorkStep::CloseProcess).await;
    warm(&mut f).await;
    let mutation = proposal(&f, THIRD, FOURTH).await;
    let admin = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap())
        .await
        .unwrap();
    admin.database("admin").run_command(doc! {"configureFailPoint": "failCommand", "mode": {"times": 5},
        "data": {"failCommands": ["commitTransaction"], "appName": &f.app,
        "writeConcernError": {"code": 50, "codeName": "MaxTimeMSExpired", "errmsg": "injected ambiguous commit"}}}).await.unwrap();
    let before_reads = f.complete_reads.load(Ordering::SeqCst);
    let result = f.store.commit_agent_work(mutation.clone()).await;
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint": "failCommand", "mode": "off"})
        .await
        .unwrap();
    result.unwrap();
    assert_eq!(f.ambiguous_commits.load(Ordering::SeqCst), 5);
    assert_eq!(f.complete_reads.load(Ordering::SeqCst) - before_reads, 1);
    assert_eq!(
        f.store.get_agent(&f.agent.id).await.unwrap(),
        Some(mutation.agent)
    );
    assert_eq!(
        f.store
            .agent_log
            .count_documents(doc! {"agent_id": f.agent.id.as_str()})
            .await
            .unwrap(),
        10_001
    );
    f.cleanup().await;
}
