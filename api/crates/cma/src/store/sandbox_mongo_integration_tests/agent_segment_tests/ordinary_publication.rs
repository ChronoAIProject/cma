//! Actual adopted roots: runtime assembly, publication and command settlement.
use super::admission_contexts::support::{claim_request, live_request, message_mutation, record};
use super::*;
use crate::store::agent_conversation_store as conversation;
use crate::store::agent_log_segments::{
    conversation as scoped, transaction_scope::TransactionScope,
};
use layer2_core::sandbox::agent_io::*;
use layer2_core::sandbox::{AgentMessageAssembly, AgentUpdateRecorder, DurableAgentUpdateRecorder};
use std::sync::Arc;
mod settlement;
mod settlement_campaign;
mod support;
mod uncertain;
mod writer_caller_tests;
use support::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn adopted_agent_publishes_runtime_output_after_restart_and_settles_command() {
    let (client, name, store, current, command) = working("v2_output_settlement").await;
    let recorder = DurableAgentUpdateRecorder::new(Arc::new(store.clone()));
    let prefix = recorder
        .record_update(
            &current.id,
            1,
            &mut AgentMessageAssembly::default(),
            chunk("prefix "),
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
    let observed = store.get_agent(&current.id).await.unwrap().unwrap();
    assert_eq!(observed.revision, current.revision + 1);
    assert_eq!(
        observed.conversation.last_seq,
        current.conversation.last_seq + 1
    );
    assert_eq!(prefix.appended.len(), 1);
    assert!(matches!(
        prefix.appended[0].kind,
        AgentLogKind::OperationDeliveryChanged {
            state: layer2_core::sandbox::AgentOperationDelivery::ExecutionObserved,
            ..
        }
    ));
    assert_eq!(
        store
            .agent_log(&current.id, current.conversation.last_seq, 10)
            .await
            .unwrap(),
        prefix.appended,
    );
    let private = store
        .agent_message_checkpoint(&current.id)
        .await
        .unwrap()
        .unwrap();
    assert!(private.matches(&observed));
    assert_eq!(
        serde_json::to_value(&private.assembly).unwrap()["content"],
        "prefix "
    );
    drop(recorder);
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    let recorder = DurableAgentUpdateRecorder::new(Arc::new(reopened.clone()));
    recorder
        .record_update(
            &current.id,
            1,
            &mut AgentMessageAssembly::default(),
            chunk("suffix"),
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
    drop(recorder);
    let terminal = DurableAgentUpdateRecorder::new(Arc::new(reopened.clone()));
    let result = terminal
        .record_update(
            &current.id,
            1,
            &mut AgentMessageAssembly::default(),
            layer3_adapter::ConversationUpdate::TurnEnded(layer3_adapter::PromptResponse::new(
                layer3_adapter::TurnStopReason::EndTurn,
            )),
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
    assert!(result.turn_closed);
    let logs = reopened.agent_log(&current.id, 0, 100).await.unwrap();
    let answers: Vec<_> = logs
        .iter()
        .filter_map(|entry| match &entry.kind {
            AgentLogKind::AgentMessage { content } => Some(content.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(answers, ["prefix suffix"]);
    let settled = crate::store::agent_log_telemetry::isolated_test(async {
        let settled = reopened
            .reconcile_command(&command.receipt.envelope.command_id)
            .await
            .unwrap();
        for success in [false, true] {
            assert_eq!(
                crate::store::agent_log_telemetry::snapshot(
                    crate::store::agent_log_telemetry::ScanCaller::CommandAdmission,
                    success,
                )[0],
                0,
                "segmented settlement must never fall back to a lifetime scan",
            );
        }
        settled
    })
    .await;
    assert_eq!(settled.status, AgentResponseStatus::Completed);
    assert!(settled.work.lease.is_none());
    let checkpoint = reopened
        .agent_message_checkpoint(&current.id)
        .await
        .unwrap()
        .unwrap();
    assert!(checkpoint.assembly.is_empty());
    assert_eq!(checkpoint.outcome, result);
    let finished = reopened.get_agent(&current.id).await.unwrap().unwrap();
    assert!(checkpoint.matches(&finished));
    assert_eq!(finished.status, AgentStatus::Ready);
    let row = reopened
        .agents
        .find_one(doc! {"_id":current.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.get_i64("document_schema_version").unwrap(), 2);
    let stable = snapshot(&client, &name).await;
    assert_eq!(
        reopened
            .reconcile_command(&command.receipt.envelope.command_id)
            .await
            .unwrap(),
        settled
    );
    assert_eq!(snapshot(&client, &name).await, stable);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn adopted_output_abort_refusal_and_foreign_receipt_cannot_publish_or_acknowledge() {
    let (client, name, store, current, _) = working("v2_output_abort").await;
    let mutation = output(&current, "original output");
    let before = snapshot(&client, &name).await;
    let mut session = client.start_session().await.unwrap();
    let mut scope = TransactionScope::start(&mut session).await.unwrap();
    let receipt = scoped::apply(
        scoped::PublicationSource::Conversation,
        &store,
        &mutation,
        None,
        None,
        None,
        &mut scope,
    )
    .await
    .unwrap();
    scope.release().abort_transaction().await.unwrap();
    assert_eq!(snapshot(&client, &name).await, before);
    assert!(!conversation::reconcile_conversation(&store, &receipt)
        .await
        .unwrap());
    let mut invalid = mutation.clone();
    invalid.agent.worktree_id = layer2_core::ids::WorktreeId("foreign-worktree".into());
    assert!(matches!(
        store.commit_agent_conversation(invalid).await,
        Err(SandboxStoreError::Conflict)
    ));
    assert_eq!(snapshot(&client, &name).await, before);

    let mut scope = TransactionScope::start(&mut session).await.unwrap();
    let receipt = scoped::apply(
        scoped::PublicationSource::Conversation,
        &store,
        &mutation,
        None,
        None,
        None,
        &mut scope,
    )
    .await
    .unwrap();
    scope.release().commit_transaction().await.unwrap();
    assert!(conversation::reconcile_conversation(&store, &receipt)
        .await
        .unwrap());
    let mut foreign = receipt.clone();
    foreign
        .segmented
        .as_mut()
        .unwrap()
        .publication
        .result_digest
        .bytes[0] ^= 1;
    assert!(conversation::reconcile_conversation(&store, &foreign)
        .await
        .is_err());
    // An exact original publication remains provable after later progress.
    let next = store.get_agent(&current.id).await.unwrap().unwrap();
    store
        .commit_agent_conversation(output(&next, "later output"))
        .await
        .unwrap();
    assert!(conversation::reconcile_conversation(&store, &receipt)
        .await
        .unwrap());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn adopted_checkpoint_abort_and_wrong_boundary_preserve_private_assembly() {
    let (client, name, store, current, _) = working("v2_checkpoint_abort").await;
    let previous = partial(&current, "private prefix");
    store
        .commit_agent_message_update(current.clone(), None, previous.clone(), None)
        .await
        .unwrap();
    let mutation = output(&current, "private prefix and suffix");
    let next = boundary(&mutation);
    let before = snapshot(&client, &name).await;
    let mut session = client.start_session().await.unwrap();
    let mut scope = TransactionScope::start(&mut session).await.unwrap();
    let (_, receipt) = scoped::checkpoint::apply(
        &store,
        (&current, None),
        Some(&previous),
        &next,
        Some(&mutation),
        &conversation::checkpoint::Authority::Live,
        &mut scope,
    )
    .await
    .unwrap();
    scope.release().abort_transaction().await.unwrap();
    assert_eq!(snapshot(&client, &name).await, before);
    assert!(
        !conversation::reconcile_conversation(&store, receipt.as_ref().unwrap())
            .await
            .unwrap()
    );
    let mut wrong = next.clone();
    wrong.outcome.appended[0].kind = AgentLogKind::AgentMessage {
        content: "foreign boundary".into(),
    };
    assert!(matches!(
        store
            .commit_agent_message_update(
                current.clone(),
                Some(previous.clone()),
                wrong,
                Some(mutation.clone())
            )
            .await,
        Err(SandboxStoreError::Conflict)
    ));
    assert_eq!(snapshot(&client, &name).await, before);
    let committed = store
        .commit_agent_message_update(
            current.clone(),
            Some(previous.clone()),
            next.clone(),
            Some(mutation.clone()),
        )
        .await
        .unwrap();
    assert_eq!(committed, next.outcome);
    let after = snapshot(&client, &name).await;
    assert_eq!(
        store
            .commit_agent_message_update(
                current.clone(),
                Some(previous.clone()),
                next.clone(),
                Some(mutation.clone())
            )
            .await
            .unwrap(),
        committed
    );
    assert_eq!(snapshot(&client, &name).await, after);
    let publications = client
        .database(&name)
        .collection::<mongodb::bson::Document>(segments::PUBLICATIONS);
    let key = format!(
        "conversation:{}:{}",
        mutation.expected_agent_revision, mutation.expected_last_seq
    );
    let original = publications
        .find_one(doc! {"agent_id":current.id.as_str(),"publication_key":key})
        .await
        .unwrap()
        .unwrap();
    let id = original.get_str("_id").unwrap();
    publications.delete_one(doc! {"_id":id}).await.unwrap();
    let missing = snapshot(&client, &name).await;
    assert!(matches!(
        store
            .commit_agent_message_update(
                current.clone(),
                Some(previous.clone()),
                next.clone(),
                Some(mutation.clone())
            )
            .await,
        Err(SandboxStoreError::Unavailable(_))
    ));
    assert_eq!(snapshot(&client, &name).await, missing);
    let mut foreign = original.clone();
    foreign.insert(
        "fingerprint",
        mongodb::bson::Binary {
            subtype: mongodb::bson::spec::BinarySubtype::Generic,
            bytes: vec![0; 32],
        },
    );
    publications.insert_one(foreign).await.unwrap();
    let corrupted = snapshot(&client, &name).await;
    assert!(store
        .commit_agent_message_update(current, Some(previous), next, Some(mutation))
        .await
        .is_err());
    assert_eq!(snapshot(&client, &name).await, corrupted);
    publications
        .replace_one(doc! {"_id":id}, original.clone())
        .await
        .unwrap();
    assert_eq!(snapshot(&client, &name).await, after);
    client.database(&name).drop().await.unwrap();
}
