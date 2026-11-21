use super::*;
use crate::store::agent_ownership::command_test_support::handoff;
use layer2_core::sandbox::agent_io::{CommandFailureCode, CommandLedgerError};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn trigger_terminal_projection_requires_the_current_owner_in_both_log_formats() {
    for (segmented, claimed) in [(false, false), (true, false), (false, true), (true, true)] {
        let accepted = Accepted::new().await;
        if segmented {
            super::segmented::adopt(&accepted.f, &accepted.agent).await;
        }
        let recorded = if claimed {
            accepted.claim().await
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
                        layer2_core::now_rfc3339(),
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
        accepted.f.close().await;
    }
}
