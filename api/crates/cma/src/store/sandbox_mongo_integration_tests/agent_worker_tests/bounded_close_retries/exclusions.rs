use super::*;
use layer2_core::sandbox::store::plan_agent_cleanup_capacity_retry;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn close_retry_capacity_and_metadata_only_settlement_keep_full_audits() {
    let f = Fixture::new(10_000).await;
    scans::isolated_test(async {
        assert_eq!(
            f.store.commit_agent_work(retry(&f.agent)).await,
            Err(SandboxStoreError::AgentLogCapacityExceeded)
        );
        assert_eq!(scans::snapshot(CALLER, true)[0], 1);
    })
    .await;
    for (at, ready_at) in [(MINUTE, TWO_MINUTES), (TWO_MINUTES, "2026-09-05T00:03:00Z")] {
        let current = if at == MINUTE {
            f.agent.clone()
        } else {
            claim(&f.store, &f.agent.id, at).await
        };
        let last = f
            .store
            .agent_log_entry(&current.id, current.conversation.last_seq)
            .await
            .unwrap();
        let mutation = plan_agent_cleanup_capacity_retry(
            &current,
            last.as_ref(),
            "worker",
            current.work.lease.as_ref().unwrap().fence,
            at,
            ready_at,
        )
        .unwrap();
        assert_eq!(mutation.agent_logs.len(), usize::from(at == MINUTE));
        scans::isolated_test(async {
            f.store.commit_agent_work(mutation).await.unwrap();
            assert_eq!(
                scans::snapshot(
                    ScanCaller::CapacitySettlement(AgentWorkStep::CloseProcess),
                    true
                )[0],
                1
            );
        })
        .await;
    }
    assert_eq!(
        f.store
            .agent_log
            .count_documents(doc! { "agent_id": f.agent.id.as_str() })
            .await
            .unwrap(),
        10_001
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn successful_close_keeps_full_validation_and_exact_effect_receipt() {
    let f = Fixture::new(500).await;
    let mut mutation = retry(&f.agent);
    mutation.agent = f.agent.clone();
    mutation
        .agent
        .work
        .advance(
            "worker",
            mutation.fence,
            AgentWorkStep::PublishStopped,
            MINUTE.into(),
        )
        .unwrap();
    mutation.agent.revision += 1;
    mutation.agent.updated_at = MINUTE.into();
    mutation.agent.conversation.last_seq += 1;
    mutation.agent_logs = vec![AgentLogKind::WorkStepCompleted {
        step: AgentWorkStep::CloseProcess,
    }];
    scans::isolated_test(async {
        assert_eq!(
            f.store.commit_agent_work(mutation.clone()).await,
            Err(SandboxStoreError::Conflict)
        );
        assert_eq!(scans::snapshot(CALLER, true)[0], 1);
    })
    .await;
    mutation.agent.work.receipts.push(AgentWorkReceipt {
        step: AgentWorkStep::CloseProcess,
        operation_id: format!("agent:{}:close:v1", f.agent.id),
        request_sha256: "b".repeat(64),
        completed_at: MINUTE.into(),
    });
    scans::isolated_test(async {
        f.store.commit_agent_work(mutation.clone()).await.unwrap();
        assert_eq!(scans::snapshot(CALLER, true)[0], 1);
    })
    .await;
    assert_eq!(
        f.store.get_agent(&f.agent.id).await.unwrap(),
        Some(mutation.agent)
    );
    f.cleanup().await;
}
