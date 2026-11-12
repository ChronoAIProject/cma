//! Actual unclaimed settlement ports, never a synthetic maintenance decision.
use super::*;
use crate::store::ledger_maintenance_publication::test_support as maintenance;

#[path = "ledger_maintenance_unclaimed_race_tests.rs"]
mod races;

async fn reserved(
    store: &MongoSandboxStore,
) -> (layer2_core::sandbox::Sandbox, AdmittedAgentCommand) {
    let (_, input) = fixture(store).await;
    let source = &input.accepted.plan().source;
    let result = store
        .reserve_command(reservation(
            &source.agents[0],
            &source.sandbox,
            "unclaimed-fixture",
        ))
        .await
        .unwrap();
    let CommandReservationResult::Created(command) = result else {
        panic!("fixture must create command")
    };
    (source.sandbox.clone(), *command)
}

async fn reject(
    store: &MongoSandboxStore,
    command: &AdmittedAgentCommand,
) -> Result<(), CommandLedgerError> {
    store
        .reject_unclaimed_command(
            &command.receipt.envelope.command_id,
            command.revision,
            CommandFailureCode::AuthorityRevoked,
            NOW.into(),
        )
        .await
}

async fn assert_settled_once(store: &MongoSandboxStore, original: &AdmittedAgentCommand) {
    let actual = store
        .get_command(&original.receipt.envelope.command_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(actual.receipt, original.receipt);
    assert_eq!(actual.key_sha256, original.key_sha256);
    assert_eq!(actual.status, AgentResponseStatus::Failed);
    assert_eq!(actual.revision, original.revision + 1);
    assert!(actual.work.lease.is_none());
    assert!(actual.work.dispatch.is_none());
    let failure = actual.work.failure.as_ref().unwrap();
    assert_eq!(failure.code, CommandFailureCode::AuthorityRevoked);
    assert_eq!(failure.at, NOW);
}

#[tokio::test]
#[ignore = "requires an owned disposable replica set and unclaimed hooks"]
async fn unclaimed_refusal_succeeds_unheld_and_refuses_both_holds_after_reopen() {
    for hold in [0, 1, 2] {
        let (client, name, store) = database().await;
        let (parent, command) = reserved(&store).await;
        let fence = maintenance::fence_value(&store, &parent).await;
        if hold != 0 {
            maintenance::hold(&store, &parent, hold == 1).await;
        }
        let before = all_rows(&store).await;
        let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
        let result = reject(&reopened, &command).await;
        if hold == 0 {
            result.unwrap();
            assert_settled_once(&store, &command).await;
            assert_eq!(maintenance::fence_value(&store, &parent).await, fence + 1);
            let settled = all_rows(&store).await;
            assert_eq!(
                reject(&reopened, &command).await,
                Err(CommandLedgerError::StaleClaim)
            );
            assert_eq!(all_rows(&store).await, settled);
        } else {
            assert_eq!(result, Err(CommandLedgerError::NotAdmissible));
            assert_eq!(all_rows(&store).await, before);
        }
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable replica set and unclaimed hooks"]
async fn unclaimed_refusal_rejects_incomplete_hold_audit_without_partial_settlement() {
    for v1 in [false, true] {
        let (client, name, store) = database().await;
        let (parent, command) = reserved(&store).await;
        maintenance::hold(&store, &parent, v1).await;
        let audit = if v1 {
            crate::store::ledger_maintenance_transaction::audit_collection(&store)
        } else {
            crate::store::ledger_maintenance_pre_exclusion_transaction::audit_collection(&store)
        };
        audit.delete_many(doc! {}).await.unwrap();
        let before = all_rows(&store).await;
        assert_eq!(
            reject(&store, &command).await,
            Err(CommandLedgerError::Corrupt)
        );
        assert_eq!(all_rows(&store).await, before);
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable replica set and unclaimed hooks"]
async fn unclaimed_refusal_requires_actual_child_and_parent_custody() {
    for missing in [0, 1, 2] {
        let (client, name, store) = database().await;
        let (mut parent, command) = reserved(&store).await;
        match missing {
            0 => {
                store
                    .agents
                    .delete_one(doc! {"_id":command.receipt.envelope.agent_id.as_str()})
                    .await
                    .unwrap();
            }
            1 => {
                store
                    .sandboxes
                    .delete_one(doc! {"_id":parent.id.as_str()})
                    .await
                    .unwrap();
            }
            _ => {
                parent.owner = layer2_core::SubjectId::new("foreign-fixture");
                store
                    .sandboxes
                    .replace_one(
                        doc! {"_id":parent.id.as_str()},
                        crate::store::sandboxes::sandbox_document(&parent, 0).unwrap(),
                    )
                    .await
                    .unwrap();
            }
        }
        let before = all_rows(&store).await;
        assert_eq!(
            reject(&store, &command).await,
            Err(if missing == 2 {
                CommandLedgerError::NotAdmissible
            } else {
                CommandLedgerError::NotFound
            })
        );
        assert_eq!(all_rows(&store).await, before);
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable replica set and unclaimed hooks"]
async fn unclaimed_refusal_keeps_original_invalid_stale_and_live_claim_decisions() {
    let (client, name, store) = database().await;
    let (_, command) = reserved(&store).await;
    for (revision, code, at, error) in [
        (
            command.revision + 1,
            CommandFailureCode::AuthorityRevoked,
            NOW,
            CommandLedgerError::StaleClaim,
        ),
        (
            command.revision,
            CommandFailureCode::QueueTimeout,
            NOW,
            CommandLedgerError::Invalid,
        ),
        (
            command.revision,
            CommandFailureCode::AuthorityRevoked,
            "invalid-time",
            CommandLedgerError::Invalid,
        ),
    ] {
        let before = all_rows(&store).await;
        assert_eq!(
            store
                .reject_unclaimed_command(
                    &command.receipt.envelope.command_id,
                    revision,
                    code,
                    at.into()
                )
                .await,
            Err(error)
        );
        assert_eq!(all_rows(&store).await, before);
    }
    let agent = store
        .get_agent(&command.receipt.envelope.agent_id)
        .await
        .unwrap()
        .unwrap();
    let claimed = store
        .claim_next_command(claim_request(&agent, NOW))
        .await
        .unwrap()
        .unwrap();
    let before = all_rows(&store).await;
    assert_eq!(
        reject(&store, &claimed).await,
        Err(CommandLedgerError::StaleClaim)
    );
    assert_eq!(all_rows(&store).await, before);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires an owned disposable replica set with failpoints and unclaimed hooks"]
async fn failed_unclaimed_commit_rolls_back_parent_and_command_then_retry_settles_once() {
    let (client, name, store) = database().await;
    let (_, command) = reserved(&store).await;
    let before = all_rows(&store).await;
    client
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":{"times":1},
        "data":{"failCommands":["commitTransaction"],"errorCode":13}})
        .await
        .unwrap();
    let result = reject(&store, &command).await;
    client
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
    assert_eq!(result, Err(CommandLedgerError::Unavailable));
    assert_eq!(all_rows(&store).await, before);
    support::observe_transaction_cleanup(&store).await;
    assert_eq!(all_rows(&store).await, before);
    reject(&store, &command).await.unwrap();
    assert_settled_once(&store, &command).await;
    client.database(&name).drop().await.unwrap();
}
