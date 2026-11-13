use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn capacity_and_marked_retries_park_without_consuming_completion_reserve() {
    let f = Fixture::new().await;
    let initial = f.pending(AgentWorkStep::CloseProcess).await;
    f.client
        .database(&f.name)
        .collection::<Document>("agents")
        .update_one(
            doc! { "_id": initial.id.as_str() },
            doc! { "$set": { "record.work.attempts": 12_i64 } },
        )
        .await
        .unwrap();
    let worker = crate::sandbox::agent_worker_tests::work_retry_policy_tests::worker(
        &f.h,
        "agent-worker",
        true,
    );
    let mut now = SystemTime::now();
    for attempt in 0..2 {
        let current = f.claim(&initial.id, now).await;
        let result = worker
            .handle_failure(
                current.clone(),
                WorkerFailure::retryable("agent_cleanup_unavailable", "fixture operation failed"),
                now,
            )
            .await;
        worker
            .finish_capacity_attempt(&current, result, now)
            .await
            .unwrap();
        let saved = f.h.stored(&initial.id).await;
        assert_eq!(saved.work.attempts, 13 + attempt);
        assert_eq!(saved.work.step, initial.work.step);
        assert_eq!(saved.status, initial.status);
        assert_eq!(saved.work.receipts, current.work.receipts);
        assert_eq!(saved.work.cleanup_debt, current.work.cleanup_debt);
        assert_eq!(saved.conversation.last_seq, 10_001);
        assert_eq!(
            layer2_core::seconds_from_rfc3339(&saved.work.ready_at).unwrap()
                - layer2_core::seconds_from_rfc3339(&saved.updated_at).unwrap(),
            1800
        );
        assert!(!worker
            .run_one(&initial.id, due(&saved) - Duration::from_secs(1))
            .await
            .unwrap());
        now = due(&saved);
    }
    assert!(worker.run_one(&initial.id, now).await.unwrap());
    let completed = f.h.stored(&initial.id).await;
    assert_eq!(completed.work.step, AgentWorkStep::PublishStopped);
    assert_eq!(completed.work.attempts, 0);
    assert_eq!(completed.conversation.last_seq, 10_002);
    f.cleanup().await;
}
