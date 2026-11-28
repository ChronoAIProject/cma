//! Exercise busy appends using the production worker and actual Mongo claims.
use super::*;
use layer2_core::agent_ownership::OwnershipStore;
use layer3_adapter::{
    ConversationUpdate, PromptResponse, RuntimeOutputReceipt, RuntimeOutputSource, TurnStopReason,
};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn ordinary_appends_wait_for_the_durable_turn_with_two_managers_and_five_agents() {
    let (client, name) = database("queued_turn_contention").await;
    let (unowned, initial, parent, _, _, _) = composed(&client, &name).await;
    let (store, first_owner) = super::cancel_wake_tests::replica(&unowned, "first", 5).await;
    let (first_execution, first_sends) = fixture_command_execution(&store, &initial);
    let first = AgentCommandWorker::new(store.clone(), first_execution.clone(), "first".into())
        .unwrap()
        .with_ownership(Some(first_owner.clone()));

    let mut pending = Vec::new();
    for index in 0..5 {
        let agent = if index == 0 {
            initial.clone()
        } else {
            let agent = ready_agent(&parent, &format!("agt_queued_{index}"));
            insert_agent(&unowned, &agent, NOW).await;
            let (mut worktree, _) = worktree_and_agent(&parent.id);
            worktree.id = agent.worktree_id.clone();
            worktree.workspace_id = agent.workspace_id.clone();
            worktree.agent_id = Some(agent.id.clone());
            worktree.name = format!("worktree-{index}");
            worktree.path = format!("/home/cma/project/.worktrees/worktree-{index}");
            worktree.branch = format!("cma/worktree-{index}");
            unowned.create_worktree(worktree).await.unwrap();
            agent
        };
        let original = record(
            store
                .reserve_command(live_request(&agent, &parent, "original"))
                .await
                .unwrap(),
        );
        first
            .run_scheduled(&original.receipt.envelope.command_id)
            .await
            .unwrap();
        let working = store.get_agent(&agent.id).await.unwrap().unwrap();
        assert_eq!(working.status, AgentStatus::Working);
        assert!(working.conversation.active_turn.is_some());
        let mut reservation = live_request(&working, &parent, "follow-up");
        reservation.envelope.source.kind = AgentSourceKind::Api;
        let owner = first_owner.acquire(&agent.id).await.unwrap().unwrap();
        pending.push((
            working,
            record(store.reserve_command(reservation).await.unwrap()),
            owner,
        ));
    }

    let (successor, second_owner) = super::cancel_wake_tests::replica(&unowned, "second", 5).await;
    let (second_execution, second_sends) = fixture_command_execution(&successor, &initial);
    let second =
        AgentCommandWorker::new(successor.clone(), second_execution.clone(), "second".into())
            .unwrap()
            .with_ownership(Some(second_owner.clone()));
    for (agent, queued, owner) in &pending {
        let id = &queued.receipt.envelope.command_id;
        assert_eq!(
            first_execution.check_queued(queued).await,
            CommandReadiness::WaitingForActiveTurn
        );
        let (first_delay, second_delay) =
            tokio::join!(first.run_scheduled(id), second.run_scheduled(id));
        assert_eq!(first_delay.unwrap(), Duration::from_secs(1));
        assert_eq!(second_delay.unwrap(), Duration::from_secs(1));
        assert_eq!(store.get_command(id).await.unwrap().as_ref(), Some(queued));
        assert_eq!(
            store
                .agent_ownership
                .as_ref()
                .unwrap()
                .owner(&agent.id)
                .await
                .unwrap()
                .unwrap()
                .token,
            owner.token,
        );
    }
    assert_eq!(first_sends.load(Ordering::SeqCst), 5);
    assert_eq!(second_sends.load(Ordering::SeqCst), 0);
    let mut expired = live_request(&pending[0].0, &parent, "already-expired");
    expired.envelope.source.kind = AgentSourceKind::Api;
    let admission_time = SystemTime::now() - Duration::from_secs(301);
    expired.admitted_at = layer2_core::rfc3339_at(admission_time);
    expired.queue_deadline = layer2_core::rfc3339_at(admission_time + Duration::from_secs(300));
    let expired = record(store.reserve_command(expired).await.unwrap());
    let expired_id = &expired.receipt.envelope.command_id;
    first.run_scheduled(expired_id).await.unwrap();
    let refused = store.get_command(expired_id).await.unwrap().unwrap();
    assert_eq!(
        refused.work.failure.as_ref().map(|failure| failure.code),
        Some(CommandFailureCode::QueueTimeout)
    );
    assert!(refused.work.dispatch.is_none());

    first_owner.begin_drain().await;
    let recorder = DurableAgentUpdateRecorder::new(successor.clone());
    for (working, queued, old_owner) in pending {
        let id = &queued.receipt.envelope.command_id;
        let generation = working.conversation.runtime_generation.unwrap();
        first_owner
            .handoff(&old_owner, async { Ok(()) })
            .await
            .unwrap();
        let owner = second_owner.acquire(&working.id).await.unwrap().unwrap();
        assert!(owner.token.epoch > old_owner.token.epoch);
        let (ack, _receiver) = tokio::sync::mpsc::channel(1);
        let source = RuntimeOutputSource {
            stream_id: uuid::Uuid::new_v4().to_string(),
            cursor: 1,
            native_conversation_id: working.conversation.conversation_id.clone().unwrap(),
            payload_sha256: "a".repeat(64),
        };
        let runtime_terminal_at = SystemTime::now();
        let outcome = owner
            .run(
                recorder.record_update(
                    &working.id,
                    generation,
                    &mut AgentMessageAssembly::default(),
                    ConversationUpdate::RuntimeOutput {
                        receipt: RuntimeOutputReceipt::new(source.clone(), ack),
                        update: Some(
                            Box::new(ConversationUpdate::TurnEnded(PromptResponse::new(
                                TurnStopReason::EndTurn,
                            )))
                            .into(),
                        ),
                    },
                    layer2_core::rfc3339_at(runtime_terminal_at),
                ),
            )
            .await
            .unwrap()
            .unwrap();
        let ready = successor.get_agent(&working.id).await.unwrap().unwrap();
        assert_eq!(ready.status, AgentStatus::Ready);
        assert!(ready.conversation.active_turn.is_none());
        assert!(outcome.appended.iter().any(|event| matches!(
            event.kind,
            AgentLogKind::TurnCompleted { turn_sequence }
                if Some(turn_sequence) == working.conversation.active_turn
        )));
        println!(
            "agent={} runtime_terminal={} runtime_cursor={} durable_ready={} durable_cursor={}",
            working.id,
            layer2_core::rfc3339_at(runtime_terminal_at),
            source.cursor,
            layer2_core::now_rfc3339(),
            ready.conversation.last_seq
        );
        assert_eq!(
            second_execution.check_queued(&queued).await,
            CommandReadiness::Ready
        );
        for result in [
            second.run_scheduled(id).await,
            first.run_scheduled(id).await,
        ] {
            assert!(result.is_ok(), "{result:?}");
        }
        let dispatched = store.get_command(id).await.unwrap().unwrap();
        assert_eq!(dispatched.receipt, queued.receipt);
        assert_eq!(dispatched.queue_deadline, queued.queue_deadline);
        assert!(dispatched.work.dispatch.is_some());
    }
    assert_eq!(first_sends.load(Ordering::SeqCst), 5);
    assert_eq!(second_sends.load(Ordering::SeqCst), 5);
    second.run_scheduled(expired_id).await.unwrap();
    assert_eq!(store.get_command(expired_id).await.unwrap(), Some(refused));
    assert_eq!(second_sends.load(Ordering::SeqCst), 5);
    client.database(&name).drop().await.unwrap();
}
