use super::*;

#[path = "targeted_control_approval_tests.rs"]
mod approvals;
#[path = "capacity_control_qualification.rs"]
mod capacity_control_qualification;
#[path = "targeted_control_failure_tests.rs"]
mod failures;
#[path = "targeted_control_fixtures.rs"]
mod fixtures;
#[path = "operator_stop_controls_tests.rs"]
mod operator_stop_controls;
use fixtures::*;
#[path = "targeted_control_contention_tests.rs"]
mod contention;
#[path = "targeted_control_inspection_tests.rs"]
mod inspection;
#[path = "interrupting_message_tests.rs"]
mod interrupting_message;
#[path = "targeted_control_public_tests.rs"]
mod public;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn explicit_controls_deduplicate_across_replicas_and_replay_after_new_turn() {
    let (client, name) = database("targeted_replay").await;
    let (one, agent, mut request, source) = composed(&name, &client).await;
    let two = MongoSandboxStore::open(&client, &name).await.unwrap();
    let mut retry = request.clone();
    retry.command.envelope.command_id = AgentCommandId::generate();
    let (left, right) = tokio::join!(
        one.reserve_targeted_control(request.clone()),
        two.reserve_targeted_control(retry)
    );
    let original = left.unwrap();
    assert_eq!(right.unwrap(), original);
    assert_eq!(original.record.receipt.response_id, None);
    assert_eq!(
        one.command_authorization_audits
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    let next = finish_and_start(&one, &agent, &source).await;
    request.observed_agent_revision = next.revision;
    request.command.queue_capacity = 1;
    assert_ne!(
        next.conversation.active_turn,
        Some(request.target.turn_sequence)
    );
    let before = one
        .agent_commands
        .find_one(doc! { "_id": original.binding.command_id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        two.reserve_targeted_control(request.clone()).await.unwrap(),
        original
    );
    assert_eq!(
        two.lookup_targeted_control(request.clone()).await.unwrap(),
        Some(original.clone())
    );
    assert_eq!(one.get_agent(&agent.id).await.unwrap(), Some(next));
    assert_eq!(
        one.agent_commands
            .find_one(doc! { "_id": original.binding.command_id.as_str() })
            .await
            .unwrap(),
        Some(before)
    );
    for change in 0..3 {
        let mut changed = request.clone();
        match change {
            0 => changed.target.response_id = AgentResponseId::generate(),
            1 => changed.target.turn_sequence += 1,
            _ => changed.command.expected_agent_revision += 1,
        }
        assert_eq!(
            two.reserve_targeted_control(changed).await.unwrap_err(),
            CommandLedgerError::IdempotencyConflict
        );
    }
    assert_eq!(
        one.agent_commands
            .count_documents(doc! { "explicit_control": { "$exists": true } })
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        one.command_authorization_audits
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn v1_replay_and_old_worker_updates_preserve_explicit_control_compatibility() {
    let (client, name) = database("targeted_v1").await;
    let (store, agent, mut request, _) = composed(&name, &client).await;
    let legacy = record(
        store
            .reserve_command(request.command.clone())
            .await
            .unwrap(),
    );
    assert_eq!(
        store
            .reserve_targeted_control(request.clone())
            .await
            .unwrap_err(),
        CommandLedgerError::IdempotencyConflict
    );
    assert_eq!(
        record(
            store
                .reserve_command(request.command.clone())
                .await
                .unwrap()
        ),
        legacy
    );
    request.command.key_sha256 = command_key_digest("explicit-second").unwrap();
    request.command.envelope.command_id = AgentCommandId::generate();
    let explicit = store
        .reserve_targeted_control(request.clone())
        .await
        .unwrap();
    let raw = store
        .agent_commands
        .find_one(doc! { "_id": explicit.binding.command_id.as_str() })
        .await
        .unwrap()
        .unwrap();
    let old_reader: AdmittedAgentCommand =
        mongodb::bson::deserialize_from_bson(raw.get("record").unwrap().clone()).unwrap();
    assert_eq!(old_reader, explicit.record);
    assert_eq!(
        old_reader.receipt.envelope.request_fingerprint,
        AgentCommand::Interrupt.fingerprint().unwrap()
    );
    let now = layer2_core::now_rfc3339();
    // The source has only a dispatch fence, so another runtime write must wait.
    // Ordinary worker refusal can still settle unclaimed controls and must
    // preserve the additive sidecar exactly as any other v1 record update does.
    assert!(store
        .claim_next_command(super::work_tests::claim_request(&agent, "v1-worker", &now))
        .await
        .unwrap()
        .is_none());
    for command in [&legacy, &explicit.record] {
        store
            .reject_unclaimed_command(
                &command.receipt.envelope.command_id,
                command.revision,
                CommandFailureCode::TargetTurnChanged,
                now.clone(),
            )
            .await
            .unwrap();
    }
    let retained = store
        .lookup_targeted_control(request)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retained.binding, explicit.binding);
    assert_eq!(retained.record.target_turn, explicit.record.target_turn);
    assert_eq!(retained.record.status, AgentResponseStatus::Failed);
    let current = store
        .agent_commands
        .find_one(doc! { "_id": explicit.binding.command_id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.get("explicit_control"), raw.get("explicit_control"));
    client.database(&name).drop().await.unwrap();
}
