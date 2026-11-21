use super::*;
use layer2_core::sandbox::store::AGENT_LOG_CAPACITY_FAILURE;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn capacity_failure_refuses_queued_input_and_preserves_dispatched_recovery() {
    for dispatched in [false, true] {
        let (client, name) = database("capacity_command_recovery").await;
        let (store, agent, parent, execution, _, sends) = composed(&client, &name).await;
        let admitted = record(
            store
                .reserve_command(live_request(&agent, &parent, "one"))
                .await
                .unwrap(),
        );
        let id = &admitted.receipt.envelope.command_id;
        if dispatched {
            AgentCommandWorker::new(store.clone(), execution.clone(), "original-worker".into())
                .unwrap()
                .run_one(id)
                .await
                .unwrap();
        }
        let before = store.get_command(id).await.unwrap().unwrap();
        let mut failed = store.get_agent(&agent.id).await.unwrap().unwrap();
        // Model the retained terminal record, independently of the lifecycle
        // settlement writer. The accepted command and any dispatch are exact.
        let conversation = failed.conversation.clone();
        failed.status = AgentStatus::Failed;
        failed.failure = Some(TypedFailure {
            class: AGENT_LOG_CAPACITY_FAILURE.into(),
            detail: "Retained Agent history reached its lifetime budget.".into(),
        });
        failed
            .work
            .schedule(AgentWorkStep::None, layer2_core::now_rfc3339());
        failed.revision += 1;
        store
            .agents
            .replace_one(
                doc! {"_id":agent.id.as_str()},
                crate::store::agents::agent_document(&failed).unwrap(),
            )
            .await
            .unwrap();
        for _ in 0..2 {
            AgentCommandWorker::new(store.clone(), execution.clone(), "recovery-worker".into())
                .unwrap()
                .run_one(id)
                .await
                .unwrap();
        }
        let after = store.get_command(id).await.unwrap().unwrap();
        assert_eq!(after.receipt, admitted.receipt);
        assert_eq!(after.work.dispatch, before.work.dispatch);
        assert_eq!(sends.load(Ordering::SeqCst), usize::from(dispatched));
        if dispatched {
            // Existing terminal-Agent reconciliation settles the response; it
            // does not erase/reclassify the exact native dispatch observation.
            assert_eq!(after.status, AgentResponseStatus::Failed);
            assert_eq!(
                after.work.failure.as_ref().unwrap().code,
                CommandFailureCode::RuntimeLost
            );
            assert!(after.work.dispatch.is_some());
            assert_eq!(after.work.observation, before.work.observation);
            assert!(after.work.lease.is_none());
        } else {
            assert_eq!(after.status, AgentResponseStatus::Failed);
            assert_eq!(
                after.work.failure.unwrap().code,
                CommandFailureCode::AgentUnavailable
            );
            assert!(after.work.lease.is_none());
        }
        let retained = store.get_agent(&agent.id).await.unwrap().unwrap();
        assert_eq!(retained, failed);
        assert_eq!(retained.conversation, conversation);
        client.database(&name).drop().await.unwrap();
    }
}
