use super::*;
use layer3_adapter::{HomeVolumePolicy, HomeVolumeReceipt};

#[tokio::test]
async fn retirement_loss_stops_agents_atomically_without_revival_or_losing_history() {
    for with_checkpoint in [false, true] {
        for retained_home in [false, true] {
            let mut live = agent("agt_a", "conversation-a", 4);
            live.status = AgentStatus::Working;
            live.conversation.active_turn = Some(2);
            live.conversation.operation = Some(
                AgentOperationRecord::new(
                    1,
                    4,
                    2,
                    AgentOperationIntent::send_message("owner work"),
                    IDLE_AT.into(),
                )
                .unwrap(),
            );
            live.conversation.queued_messages =
                vec![
                    QueuedAgentMessage::new(4, "queued owner work".into(), IDLE_AT.into()).unwrap(),
                ];
            let mut starting = agent("agt_starting", "conversation-starting", 1);
            starting.status = AgentStatus::Starting;
            starting.ready_at = None;
            starting.conversation.conversation_id = None;
            starting.conversation.runtime_generation = None;
            starting.conversation.first_message =
                Some(FirstMessageRecord::new("launch", "first work", IDLE_AT).unwrap());
            let mut stopping = agent("agt_stopping", "conversation-stopping", 1);
            stopping
                .request_stop(AgentStopReason::Owner, IDLE_AT.into())
                .unwrap();
            let mut terminal = agent("agt_terminal", "conversation-terminal", 1);
            terminal
                .transition(
                    AgentStatus::Stopped,
                    IDLE_AT.into(),
                    Some(AgentStopReason::Owner),
                )
                .unwrap();
            let agents = vec![
                live.clone(),
                starting.clone(),
                stopping.clone(),
                terminal.clone(),
            ];
            let (store, mut original) = fixture(agents.clone(), with_checkpoint);
            if retained_home {
                let mut home = SandboxHome::reserve(
                    &original.id,
                    HomeVolumePolicy {
                        storage_class: "storage".into(),
                        capacity_gib: 64,
                        template_name: "empty".into(),
                        template_generation: "seed".into(),
                    },
                )
                .unwrap();
                home.bind(HomeVolumeReceipt {
                    volume_id: home.request.volume_id.clone(),
                    request_sha256: home.request.computed_sha256(),
                    storage_reference: "original-disk".into(),
                    storage_generation: "disk-generation".into(),
                })
                .unwrap();
                original.lifecycle.home = Some(Box::new(home));
            }
            original.begin_retirement(NOW.into()).unwrap();
            store
                .lock()
                .sandboxes
                .insert(original.id.clone(), original.clone());
            let history = store.agent_log(&live.id, 0, 100).await.unwrap();
            let request = loss_request(&original);
            assert_eq!(
                store.recover_runtime_loss(request.clone()).await.unwrap(),
                SandboxRuntimeLossOutcome::RecoveryScheduled
            );
            let retired_runtime = store.get_sandbox(&original.id).await.unwrap().unwrap();
            assert_eq!(retired_runtime.status, SandboxStatus::Retiring);
            assert!(retired_runtime.runtime.is_none());
            assert!(retired_runtime.lifecycle.operation.is_none());
            assert!(retired_runtime.lifecycle.checkpoint_capture.is_none());
            assert_eq!(
                retired_runtime.lifecycle.checkpoints,
                original.lifecycle.checkpoints
            );
            assert_eq!(retired_runtime.lifecycle.home, original.lifecycle.home);
            assert_eq!(retired_runtime.credential, original.credential);
            assert_eq!(retired_runtime.work.step, SandboxWorkStep::StopAgents);
            for before in [&live, &starting, &stopping] {
                let after = store.get_agent(&before.id).await.unwrap().unwrap();
                assert_eq!(after.status, AgentStatus::Stopped);
                assert_eq!(
                    after.stop_reason,
                    Some(
                        before
                            .requested_stop_reason
                            .clone()
                            .unwrap_or(AgentStopReason::SandboxRetired)
                    )
                );
                assert_eq!(
                    after.conversation.conversation_id,
                    before.conversation.conversation_id
                );
                assert_eq!(after.conversation.active_turn, None);
                assert!(after.conversation.queued_messages.is_empty());
                assert_eq!(after.work.step, AgentWorkStep::None);
                assert!(after.work.lease.is_none());
                assert!(after.work.cleanup_debt.is_empty());
                after.validate_conversation().unwrap();
            }
            assert_eq!(store.get_agent(&terminal.id).await.unwrap(), Some(terminal));
            let stopped = store.get_agent(&live.id).await.unwrap().unwrap();
            assert_eq!(stopped.conversation.runtime_generation, Some(5));
            assert_eq!(
                stopped.conversation.operation.unwrap().state,
                AgentOperationDelivery::RuntimeLost
            );
            assert_eq!(
                store
                    .get_agent(&starting.id)
                    .await
                    .unwrap()
                    .unwrap()
                    .conversation
                    .first_message
                    .unwrap()
                    .state,
                FirstMessageDelivery::Failed
            );
            let logs = store.agent_log(&live.id, 0, 100).await.unwrap();
            assert_eq!(&logs[..history.len()], history.as_slice());
            assert!(logs
                .iter()
                .any(|entry| entry.kind == AgentLogKind::TurnInterrupted { turn_sequence: 2 }));
            assert!(!logs
                .iter()
                .any(|entry| matches!(entry.kind, AgentLogKind::TurnCompleted { .. })));
            assert_eq!(
                store.recover_runtime_loss(request).await.unwrap(),
                SandboxRuntimeLossOutcome::Superseded
            );
            assert_eq!(store.agent_log(&live.id, 0, 100).await.unwrap(), logs);
        }
    }
}
