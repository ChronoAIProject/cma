//! Registered under checkpoint so races exercise the actual transaction helper.
use super::super::checkpoint_tests::{boundary, partial, update, working};
use super::*;
use crate::store::ledger_maintenance_publication::test_support as maintenance;
use crate::store::ledger_maintenance_test_support as support;
use crate::store::sandbox_maintenance::mongo_integration_tests::database;
use layer2_core::sandbox::store::{AgentLogStore, AgentStore, SandboxStore};
use layer2_core::sandbox::{AgentLogKind, Sandbox};

#[path = "ledger_maintenance_output_race_tests.rs"]
mod races;
#[path = "ledger_maintenance_output_reopen_tests.rs"]
mod reopen_tests;

const AT: &str = "2026-09-17T00:00:00Z";

#[derive(Clone, Copy)]
enum Write {
    Append,
    Conversation,
    Checkpoint,
}
const WRITES: [Write; 3] = [Write::Append, Write::Conversation, Write::Checkpoint];

async fn fixture() -> (mongodb::Client, String, MongoSandboxStore, Sandbox, Agent) {
    let (client, name, store) = database().await;
    let current = working(&store, "sbx_held_output").await;
    let mut parent = store
        .get_sandbox(&current.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let revision = parent.revision;
    parent.runtime.as_mut().unwrap().boot_id = Some("b".repeat(32));
    support::retain_home(&mut parent);
    parent.revision += 1;
    store.save_sandbox(revision, parent.clone()).await.unwrap();
    (client, name, store, parent, current)
}

async fn write_output(
    store: &MongoSandboxStore,
    current: &Agent,
    next: &AgentMessageCheckpoint,
    kind: Write,
) -> Result<(), SandboxStoreError> {
    let mutation = update(current, "retained output");
    match kind {
        Write::Append => {
            let entry = store
                .append_agent_log(&current.id, AT.into(), mutation.agent_logs[0].clone())
                .await?;
            assert_eq!(
                entry,
                AgentLogEntry {
                    agent_id: current.id.clone(),
                    seq: current.conversation.last_seq + 1,
                    appended_at: AT.into(),
                    kind: mutation.agent_logs[0].clone()
                }
            );
            let mut expected = current.clone();
            expected.conversation.last_seq += 1;
            assert!(store.get_agent(&current.id).await.unwrap() == Some(expected));
        }
        Write::Conversation => {
            let logs = store.commit_agent_conversation(mutation.clone()).await?;
            assert_eq!(logs, conversation_logs(&mutation).unwrap());
            assert!(store.get_agent(&current.id).await.unwrap() == Some(mutation.agent));
        }
        Write::Checkpoint => {
            assert_eq!(
                store
                    .commit_agent_message_update(current.clone(), None, next.clone(), None)
                    .await?,
                next.outcome
            );
            assert!(store.get_agent(&current.id).await.unwrap() == Some(current.clone()));
            assert_eq!(
                store.agent_message_checkpoint(&current.id).await.unwrap(),
                Some(next.clone())
            );
        }
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires an owned disposable replica set and output hooks"]
async fn outputs_succeed_unheld_and_refuse_both_holds_after_reopen() {
    for kind in WRITES {
        for hold in [0, 1, 2] {
            let (client, name, store, parent, current) = fixture().await;
            let next = partial(&current, "retained output");
            let fence = maintenance::fence_value(&store, &parent).await;
            if hold != 0 {
                maintenance::hold(&store, &parent, hold == 1).await;
            }
            let before = support::all_rows(&store).await;
            let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
            let result = write_output(&reopened, &current, &next, kind).await;
            if hold == 0 {
                result.unwrap();
                assert_ne!(support::all_rows(&store).await, before);
                assert_eq!(maintenance::fence_value(&store, &parent).await, fence + 1);
            } else {
                assert_eq!(result, Err(SandboxStoreError::Conflict));
                assert_eq!(support::all_rows(&store).await, before);
            }
            client.database(&name).drop().await.unwrap();
        }
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable replica set and output hooks"]
async fn immutable_checkpoint_replay_under_hold_keeps_exact_receipt_and_zero_writes() {
    for v1 in [false, true] {
        let (client, name, store, parent, current) = fixture().await;
        let next = partial(&current, "retained output");
        write_output(&store, &current, &next, Write::Checkpoint)
            .await
            .unwrap();
        maintenance::hold(&store, &parent, v1).await;
        let before = support::all_rows(&store).await;
        let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
        write_output(&reopened, &current, &next, Write::Checkpoint)
            .await
            .unwrap();
        assert_eq!(support::all_rows(&store).await, before);
        let mut wrong = next;
        wrong.update_fingerprint = "b".repeat(64);
        assert_eq!(
            write_output(&reopened, &current, &wrong, Write::Checkpoint).await,
            Err(SandboxStoreError::Conflict)
        );
        assert_eq!(support::all_rows(&store).await, before);
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable replica set and output hooks"]
async fn output_refuses_incomplete_hold_authority_before_any_side_effect() {
    for kind in WRITES {
        for v1 in [false, true] {
            let (client, name, store, parent, current) = fixture().await;
            maintenance::hold(&store, &parent, v1).await;
            let audit = if v1 {
                crate::store::ledger_maintenance_transaction::audit_collection(&store)
            } else {
                crate::store::ledger_maintenance_pre_exclusion_transaction::audit_collection(&store)
            };
            audit.delete_many(doc! {}).await.unwrap();
            let before = support::all_rows(&store).await;
            assert!(matches!(
                write_output(
                    &store,
                    &current,
                    &partial(&current, "retained output"),
                    kind
                )
                .await,
                Err(SandboxStoreError::Corrupt(_))
            ));
            assert_eq!(support::all_rows(&store).await, before);
            client.database(&name).drop().await.unwrap();
        }
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable replica set and output hooks"]
async fn output_refuses_missing_or_foreign_parent_without_writes() {
    for kind in WRITES {
        for missing in [false, true] {
            let (client, name, store, mut parent, current) = fixture().await;
            if missing {
                store
                    .sandboxes
                    .delete_one(doc! {"_id":parent.id.as_str()})
                    .await
                    .unwrap();
            } else {
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
            let before = support::all_rows(&store).await;
            assert_eq!(
                write_output(
                    &store,
                    &current,
                    &partial(&current, "retained output"),
                    kind
                )
                .await,
                Err(if missing {
                    SandboxStoreError::NotFound
                } else {
                    SandboxStoreError::Conflict
                })
            );
            assert_eq!(support::all_rows(&store).await, before);
            client.database(&name).drop().await.unwrap();
        }
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable replica set and output hooks"]
async fn checkpoint_boundary_and_carry_preserve_atomicity_and_refuse_hold() {
    for boundary_write in [false, true] {
        for hold in [0, 1, 2] {
            let (client, name, store, parent, current) = fixture().await;
            let previous = partial(&current, "prefix");
            write_output(&store, &current, &previous, Write::Checkpoint)
                .await
                .unwrap();
            let mutation = update(&current, "retained output");
            let next = boundary(&mutation);
            if hold != 0 {
                maintenance::hold(&store, &parent, hold == 1).await;
            }
            let before = support::all_rows(&store).await;
            let result = if boundary_write {
                store
                    .commit_agent_message_update(
                        current.clone(),
                        Some(previous.clone()),
                        next.clone(),
                        Some(mutation.clone()),
                    )
                    .await
                    .map(|result| {
                        assert_eq!(result, next.outcome);
                    })
            } else {
                store
                    .commit_agent_conversation(mutation.clone())
                    .await
                    .map(|logs| {
                        assert_eq!(logs, conversation_logs(&mutation).unwrap());
                    })
            };
            if hold == 0 {
                result.unwrap();
                assert!(
                    store.get_agent(&current.id).await.unwrap() == Some(mutation.agent.clone())
                );
                let expected = if boundary_write {
                    Some(next)
                } else {
                    previous.advance(&current, &mutation.agent)
                };
                assert_eq!(
                    store.agent_message_checkpoint(&current.id).await.unwrap(),
                    expected
                );
            } else {
                assert_eq!(result, Err(SandboxStoreError::Conflict));
                assert_eq!(support::all_rows(&store).await, before);
            }
            client.database(&name).drop().await.unwrap();
        }
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable replica set with failpoints and output hooks"]
async fn failed_output_commit_rolls_back_parent_and_all_sidecars_before_retry() {
    for kind in WRITES {
        let (client, name, store, _, current) = fixture().await;
        let next = partial(&current, "retained output");
        let before = support::all_rows(&store).await;
        client
            .database("admin")
            .run_command(
                doc! { "configureFailPoint":"failCommand", "mode":{"times":1},
                "data":{"failCommands":["commitTransaction"],"errorCode":13} },
            )
            .await
            .unwrap();
        let result = write_output(&store, &current, &next, kind).await;
        client
            .database("admin")
            .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
            .await
            .unwrap();
        assert!(matches!(result, Err(SandboxStoreError::Unavailable(_))));
        assert_eq!(support::all_rows(&store).await, before);
        write_output(&store, &current, &next, kind).await.unwrap();
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable replica set and output hooks"]
async fn checkpoint_reanchor_is_a_guarded_write_and_keeps_original_receipt() {
    for hold in [0, 1, 2] {
        let (client, name, store, parent, current) = fixture().await;
        let previous = partial(&current, "retained prefix");
        write_output(&store, &current, &previous, Write::Checkpoint)
            .await
            .unwrap();
        store
            .append_agent_log(
                &current.id,
                AT.into(),
                AgentLogKind::WorkStepCompleted {
                    step: layer2_core::sandbox::AgentWorkStep::None,
                },
            )
            .await
            .unwrap();
        let current = store.get_agent(&current.id).await.unwrap().unwrap();
        let next = previous.reanchored(&current).unwrap();
        assert_ne!(next.after_seq, previous.after_seq);
        if hold != 0 {
            maintenance::hold(&store, &parent, hold == 1).await;
        }
        let before = support::all_rows(&store).await;
        let result = store
            .commit_agent_message_update(
                current.clone(),
                Some(previous.clone()),
                next.clone(),
                None,
            )
            .await;
        if hold == 0 {
            assert_eq!(result.unwrap(), previous.outcome);
            assert_eq!(
                store.agent_message_checkpoint(&current.id).await.unwrap(),
                Some(next.clone())
            );
            maintenance::hold(&store, &parent, false).await;
            let held = support::all_rows(&store).await;
            assert_eq!(
                store
                    .commit_agent_message_update(current, Some(previous), next.clone(), None)
                    .await
                    .unwrap(),
                next.outcome
            );
            assert_eq!(support::all_rows(&store).await, held);
        } else {
            assert_eq!(result, Err(SandboxStoreError::Conflict));
            assert_eq!(support::all_rows(&store).await, before);
        }
        client.database(&name).drop().await.unwrap();
    }
}
