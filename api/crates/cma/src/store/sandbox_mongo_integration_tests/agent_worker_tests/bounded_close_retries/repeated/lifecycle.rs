use super::contracts::{proposal, seeded};
use super::*;
use layer2_core::sandbox::store::plan_agent_cleanup_capacity_retry;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn repeated_failure_mixed_writers_commit_one_exact_publication() {
    let f = seeded().await;
    let peer = MongoSandboxStore::open(&f.client, &f.name).await.unwrap();
    let marker = f
        .store
        .agent_log_entry(&f.agent.id, f.agent.conversation.last_seq)
        .await
        .unwrap()
        .unwrap();
    let repeated = proposal(&f.agent, &marker, TWO_MINUTES, "2026-09-05T00:03:00Z");
    let mut old = retry(&f.agent);
    old.appended_at = TWO_MINUTES.into();
    old.agent.updated_at = TWO_MINUTES.into();
    old.agent.work.ready_at = "2026-09-05T00:03:00Z".into();
    let (new_result, old_result) = tokio::join!(
        f.store
            .commit_agent_work_repeated_failure(repeated.clone(), None),
        peer.commit_agent_work(old.clone()),
    );
    assert_ne!(new_result.is_ok(), old_result.is_ok());
    let expected = if new_result.is_ok() {
        assert_eq!(old_result, Err(SandboxStoreError::Conflict));
        repeated.mutation.agent
    } else {
        assert_eq!(new_result, Err(SandboxStoreError::Conflict));
        old.agent
    };
    assert_eq!(
        f.store.get_agent(&f.agent.id).await.unwrap(),
        Some(expected.clone())
    );
    assert_eq!(
        f.store
            .agent_log
            .count_documents(doc! {"agent_id": f.agent.id.as_str()})
            .await
            .unwrap(),
        expected.conversation.last_seq
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn repeated_failure_policy_keeps_first_and_changed_class_full_audits() {
    let mut f = Fixture::new(500).await;
    f.store = f.store.with_retry_single_marker(true);
    for (at, due, class) in [
        (MINUTE, TWO_MINUTES, "first"),
        (TWO_MINUTES, "2026-09-05T00:03:00Z", "changed"),
    ] {
        if at != MINUTE {
            f.agent = claim(&f.store, &f.agent.id, at).await;
        }
        let mut mutation = retry(&f.agent);
        mutation.appended_at = at.into();
        mutation.agent.updated_at = at.into();
        mutation.agent.work.ready_at = due.into();
        if let AgentLogKind::WorkStepFailed { failure, .. } = &mut mutation.agent_logs[0] {
            failure.class = class.into();
        }
        scans::isolated_test(async {
            f.store.commit_agent_work(mutation).await.unwrap();
            assert_eq!(scans::snapshot(CALLER, true)[0], 1);
        })
        .await;
    }
    assert_eq!(
        f.store
            .agent_log
            .count_documents(doc! {"agent_id": f.agent.id.as_str()})
            .await
            .unwrap(),
        502
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn repeated_failure_at_capacity_audits_once_and_preserves_reserve() {
    let f = Fixture::new(10_000).await;
    for attempt in 0..4 {
        let seconds = seconds_from_rfc3339(MINUTE).unwrap() + attempt * 60;
        let at = rfc3339_at(UNIX_EPOCH + Duration::from_secs(seconds));
        let due = rfc3339_at(UNIX_EPOCH + Duration::from_secs(seconds + 60));
        let current = if attempt == 0 {
            f.agent.clone()
        } else {
            claim(&f.store, &f.agent.id, &at).await
        };
        let marker = f
            .store
            .agent_log_entry(&current.id, current.conversation.last_seq)
            .await
            .unwrap();
        let retry = plan_agent_cleanup_capacity_retry(
            &current,
            marker.as_ref(),
            "worker",
            current.work.lease.as_ref().unwrap().fence,
            &at,
            &due,
        )
        .unwrap();
        assert_eq!(retry.agent_logs.len(), usize::from(attempt == 0));
        scans::isolated_test(async {
            f.store.commit_agent_work(retry.clone()).await.unwrap();
            assert_eq!(
                scans::snapshot(
                    ScanCaller::CapacitySettlement(AgentWorkStep::CloseProcess),
                    true
                )[0],
                u64::from(attempt < 2)
            );
        })
        .await;
        assert_eq!(retry.agent.conversation.last_seq, 10_001);
        assert_eq!(retry.agent.work.attempts, attempt as u32 + 1);
    }
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn repeated_failure_then_success_still_appends_and_checks_effect_receipt() {
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
    let mut done = retry(&f.agent);
    done.agent = f.agent.clone();
    done.appended_at = "2026-09-05T00:03:00Z".into();
    done.agent
        .work
        .advance(
            "worker",
            done.fence,
            AgentWorkStep::PublishStopped,
            done.appended_at.clone(),
        )
        .unwrap();
    done.agent.revision += 1;
    done.agent.updated_at = done.appended_at.clone();
    done.agent.conversation.last_seq += 1;
    done.agent_logs = vec![AgentLogKind::WorkStepCompleted {
        step: AgentWorkStep::CloseProcess,
    }];
    assert_eq!(
        f.store.commit_agent_work(done.clone()).await,
        Err(SandboxStoreError::Conflict)
    );
    done.agent.work.receipts.push(AgentWorkReceipt {
        step: AgentWorkStep::CloseProcess,
        operation_id: format!("agent:{}:close:v1", f.agent.id),
        request_sha256: "b".repeat(64),
        completed_at: done.appended_at.clone(),
    });
    scans::isolated_test(async {
        f.store.commit_agent_work(done.clone()).await.unwrap();
        assert_eq!(scans::snapshot(CALLER, true)[0], 1);
    })
    .await;
    assert_eq!(
        f.store.get_agent(&f.agent.id).await.unwrap(),
        Some(done.agent)
    );
    assert_eq!(
        f.store
            .agent_log_entry(&f.agent.id, 1_002)
            .await
            .unwrap()
            .unwrap()
            .kind,
        done.agent_logs[0]
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn repeated_failure_unknown_commit_uses_full_receipt_without_appending() {
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
    let retry = proposal(
        &f.agent,
        &marker,
        "2026-09-05T00:03:00Z",
        "2026-09-05T00:04:00Z",
    );
    let admin = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap())
        .await
        .unwrap();
    admin.database("admin").run_command(doc! {"configureFailPoint": "failCommand", "mode": {"times": 5},
        "data": {"failCommands": ["commitTransaction"], "appName": &f.app,
            "writeConcernError": {"code": 50, "codeName": "MaxTimeMSExpired", "errmsg": "injected ambiguous commit"}}}).await.unwrap();
    let before = f.complete_reads.load(Ordering::SeqCst);
    let result = f
        .store
        .commit_agent_work_repeated_failure(retry.clone(), None)
        .await;
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint": "failCommand", "mode": "off"})
        .await
        .unwrap();
    result.unwrap();
    assert_eq!(f.ambiguous_commits.load(Ordering::SeqCst), 5);
    assert_eq!(f.complete_reads.load(Ordering::SeqCst) - before, 1);
    assert_eq!(
        f.store.get_agent(&f.agent.id).await.unwrap(),
        Some(retry.mutation.agent)
    );
    assert_eq!(
        f.store
            .agent_log
            .count_documents(doc! {"agent_id": f.agent.id.as_str()})
            .await
            .unwrap(),
        1_001
    );
    f.cleanup().await;
}
