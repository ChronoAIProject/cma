use super::*;
use crate::store::agent_ownership::command_test_support::handoff;
use layer2_core::sandbox::agent_io::{CommandFailureCode, CommandLedgerError};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn trigger_terminal_projection_requires_the_current_owner_in_both_log_formats() {
    for (segmented, claimed) in [(false, false), (true, false), (false, true), (true, true)] {
        let accepted = if segmented {
            super::segmented::accepted().await
        } else {
            Accepted::new().await
        };
        let now = accepted.command.queue_deadline.clone();
        let recorded = if claimed {
            let observed_at = layer2_core::seconds_from_rfc3339(&now).unwrap();
            accepted
                .f
                .resources
                .claim_next_command(CommandClaimRequest {
                    agent_id: accepted.agent.id.clone(),
                    holder: "common-worker".into(),
                    now: now.clone(),
                    expires_at: layer2_core::rfc3339_at(
                        std::time::UNIX_EPOCH
                            + std::time::Duration::from_secs(observed_at as u64 + 60),
                    ),
                })
                .await
                .unwrap()
                .unwrap()
        } else {
            accepted.command.clone()
        };
        let before = accepted.current_agent().await;
        let managers = handoff(&accepted.f.resources, &before.id).await;
        let id = &accepted.command.receipt.envelope.command_id;
        let reject = || async {
            match &recorded.work.lease {
                Some(lease) => managers
                    .store
                    .reject_command(
                        &lease.claim,
                        CommandFailureCode::ExecutionFailed,
                        now.clone(),
                    )
                    .await
                    .map(|_| ()),
                None => {
                    managers
                        .store
                        .reject_unclaimed_command(
                            id,
                            recorded.revision,
                            CommandFailureCode::QueueTimeout,
                            recorded.queue_deadline.clone(),
                        )
                        .await
                }
            }
        };
        assert!(matches!(
            managers.past.run(reject()).await.unwrap(),
            Err(CommandLedgerError::Conflict)
        ));
        assert_eq!(accepted.current_agent().await, before);
        assert_eq!(
            managers.store.get_command(id).await.unwrap().unwrap(),
            recorded
        );
        managers.current.run(reject()).await.unwrap().unwrap();
        let after = accepted.current_agent().await;
        assert_eq!(
            after.conversation.last_seq,
            before.conversation.last_seq + 1
        );
        assert_eq!(
            after.conversation.first_message.as_ref().unwrap().state,
            FirstMessageDelivery::Failed
        );
        assert!(managers
            .store
            .get_command(id)
            .await
            .unwrap()
            .unwrap()
            .status
            .is_terminal());
        assert!(managers.current.run(reject()).await.unwrap().is_err());
        assert_eq!(accepted.current_agent().await, after);
        let rows = accepted
            .f
            .resources
            .agent_log(&after.id, 0, 100)
            .await
            .unwrap();
        assert_eq!(
            rows.iter()
                .filter(|row| matches!(
                    row.kind,
                    AgentLogKind::FirstMessageChanged {
                        state: FirstMessageDelivery::Failed,
                        ..
                    }
                ))
                .count(),
            1
        );
        assert!(!rows.iter().any(|row| matches!(
            row.kind,
            AgentLogKind::OwnerMessage { .. } | AgentLogKind::TurnStarted { .. }
        )));
        accepted.f.close().await;
    }
}
