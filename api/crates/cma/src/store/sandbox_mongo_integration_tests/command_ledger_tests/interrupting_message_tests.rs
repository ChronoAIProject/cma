use super::super::work_tests::{claim_request, live_request, message_mutation};
use super::*;
use layer2_core::sandbox::{AgentMessageAssembly, AgentUpdateRecorder, DurableAgentUpdateRecorder};

fn pair(control: TargetedControlReservation) -> InterruptingMessageReservation {
    let mut message = control.command.clone();
    message.envelope.command_id = AgentCommandId::generate();
    message.response_id = Some(AgentResponseId::generate());
    message.envelope.command = AgentCommand::Message {
        input: AgentInput::text("Replacement input"),
        order: AgentInputOrder::InterruptAfterTurn {
            turn_sequence: control.target.turn_sequence,
        },
        metadata: Default::default(),
    };
    message.envelope.request_fingerprint = message.envelope.command.fingerprint().unwrap();
    let audit = message.admission_audit.as_mut().unwrap();
    audit.capability = layer2_core::resource_access::ResourceCapability::Interact;
    audit.observations[0].capabilities = vec![audit.capability];
    let request = InterruptingMessageReservation { control, message };
    request.validate().unwrap();
    request
}

async fn finish(store: &MongoSandboxStore, agent: &Agent, source: &AdmittedAgentCommand) -> Agent {
    DurableAgentUpdateRecorder::new(std::sync::Arc::new(store.clone()))
        .record_update(
            &agent.id,
            1,
            &mut AgentMessageAssembly::default(),
            layer3_adapter::ConversationUpdate::TurnEnded(layer3_adapter::PromptResponse::new(
                layer3_adapter::TurnStopReason::Cancelled,
            )),
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
    store
        .reconcile_command(&source.receipt.envelope.command_id)
        .await
        .unwrap();
    store.get_agent(&agent.id).await.unwrap().unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn interruption_pair_replays_across_replicas_after_original_turn_ends() {
    let (client, name) = database("interruption_pair_replay").await;
    let (store, agent, control, source) = composed(&name, &client).await;
    let mut request = pair(control);
    let replica = MongoSandboxStore::open(&client, &name).await.unwrap();
    let mut retry = request.clone();
    retry.control.command.envelope.command_id = AgentCommandId::generate();
    retry.message.envelope.command_id = AgentCommandId::generate();
    retry.message.response_id = Some(AgentResponseId::generate());
    let (one, two) = tokio::join!(
        store.reserve_interrupting_message(request.clone()),
        replica.reserve_interrupting_message(retry)
    );
    let accepted = one.unwrap();
    assert_eq!(two.unwrap(), accepted);
    assert_eq!(accepted.control.record.receipt.agent_order, 2);
    assert_eq!(accepted.message.receipt.agent_order, 3);
    assert_ne!(
        accepted.message.receipt.response_id,
        source.receipt.response_id
    );
    assert_eq!(
        store.agent_commands.count_documents(doc! {}).await.unwrap(),
        3
    );
    assert_eq!(
        store
            .command_authorization_audits
            .count_documents(doc! {})
            .await
            .unwrap(),
        2
    );
    let next = finish_and_start(&store, &agent, &source).await;
    request.control.observed_agent_revision = next.revision;
    assert_eq!(
        replica
            .reserve_interrupting_message(request.clone())
            .await
            .unwrap(),
        accepted
    );
    for change in 0..3 {
        let mut changed = request.clone();
        match change {
            0 => {
                if let AgentCommand::Message { input, .. } = &mut changed.message.envelope.command {
                    *input = AgentInput::text("Different replacement");
                }
            }
            1 => changed.control.target.response_id = AgentResponseId::generate(),
            _ => {
                changed.control.target.turn_sequence += 1;
                if let AgentCommand::Message { order, .. } = &mut changed.message.envelope.command {
                    *order = AgentInputOrder::InterruptAfterTurn {
                        turn_sequence: changed.control.target.turn_sequence,
                    };
                }
            }
        }
        changed.message.envelope.request_fingerprint =
            changed.message.envelope.command.fingerprint().unwrap();
        let before = snapshot(&store).await;
        assert_eq!(
            replica
                .reserve_interrupting_message(changed)
                .await
                .unwrap_err(),
            CommandLedgerError::IdempotencyConflict
        );
        assert_eq!(snapshot(&store).await, before);
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn interruption_pair_cannot_adopt_a_legacy_half() {
    for control_exists in [false, true] {
        let (client, name) = database("interruption_partial_pair").await;
        let (store, _, control, _) = composed(&name, &client).await;
        let request = pair(control);
        if control_exists {
            store
                .reserve_targeted_control(request.control.clone())
                .await
                .unwrap();
        } else {
            store
                .reserve_command(request.message.clone())
                .await
                .unwrap();
        }
        let before = snapshot(&store).await;
        assert_eq!(
            store
                .reserve_interrupting_message(request)
                .await
                .unwrap_err(),
            CommandLedgerError::IdempotencyConflict
        );
        assert_eq!(snapshot(&store).await, before);
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn interruption_pair_rolls_back_control_audits_and_order_when_second_admission_fails() {
    let (client, name) = database("interruption_pair_rollback").await;
    let (store, _, control, _) = composed(&name, &client).await;
    let request = pair(control);
    let mut full = request.clone();
    full.control.command.queue_capacity = 1;
    full.message.queue_capacity = 1;
    let before = snapshot(&store).await;
    assert_eq!(
        store.reserve_interrupting_message(full).await.unwrap_err(),
        CommandLedgerError::QueueFull
    );
    assert_eq!(snapshot(&store).await, before);
    client
        .database(&name)
        .run_command(doc! {
            "collMod": "agent_commands",
            "validator": {"_id": {"$ne": request.message.envelope.command_id.as_str()}},
            "validationLevel": "strict", "validationAction": "error",
        })
        .await
        .unwrap();
    let result = store.reserve_interrupting_message(request.clone()).await;
    client
        .database(&name)
        .run_command(doc! {"collMod": "agent_commands", "validator": {}})
        .await
        .unwrap();
    assert_eq!(result.unwrap_err(), CommandLedgerError::Unavailable);
    assert_eq!(snapshot(&store).await, before);
    let accepted = store.reserve_interrupting_message(request).await.unwrap();
    assert_eq!(accepted.control.record.receipt.agent_order, 2);
    assert_eq!(accepted.message.receipt.agent_order, 3);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn interruption_replacement_gets_its_own_turn_before_older_queued_messages() {
    let (client, name) = database("interruption_priority").await;
    let (store, agent, control, source) = composed(&name, &client).await;
    let parent = store.get_sandbox(&agent.sandbox_id).await.unwrap().unwrap();
    let mut earlier = Vec::new();
    for key in ["ordinary-one", "ordinary-two"] {
        let mut request = live_request(&agent, &parent, key);
        request.queue_capacity = 8;
        earlier.push(record(store.reserve_command(request).await.unwrap()));
    }
    let request = pair(control);
    let accepted = store
        .reserve_interrupting_message(request.clone())
        .await
        .unwrap();
    assert_eq!(accepted.message.receipt.agent_order, 5);
    assert!(store
        .claim_next_command(claim_request(
            &agent,
            "before-stop",
            &layer2_core::now_rfc3339()
        ))
        .await
        .unwrap()
        .is_none());
    let ready = finish(&store, &agent, &source).await;
    assert_eq!(ready.status, AgentStatus::Ready);
    assert!(store
        .claim_observed_command(
            claim_request(
                &ready,
                "stale-ordinary-observation",
                &layer2_core::now_rfc3339()
            ),
            &earlier[0].receipt.envelope.command_id,
            earlier[0].revision,
        )
        .await
        .unwrap()
        .is_none());
    let replacement = store
        .claim_next_command(claim_request(
            &ready,
            "replacement",
            &layer2_core::now_rfc3339(),
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(replacement.receipt, accepted.message.receipt);
    let claim = &replacement.work.lease.as_ref().unwrap().claim;
    let mut mutation = message_mutation(&ready, &parent, claim);
    mutation
        .agent
        .conversation
        .operation
        .as_mut()
        .unwrap()
        .intent = layer2_core::sandbox::AgentOperationIntent::send_message("Replacement input");
    mutation.agent_logs[2] = AgentLogKind::OwnerMessage {
        content: "Replacement input".into(),
    };
    store.commit_agent_conversation(mutation).await.unwrap();
    store
        .release_command_claim(claim, layer2_core::now_rfc3339())
        .await
        .unwrap();
    let dispatched = store
        .get_command(&replacement.receipt.envelope.command_id)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(
        dispatched.work.dispatch.as_ref().unwrap().turn_sequence,
        source.work.dispatch.as_ref().unwrap().turn_sequence,
    );
    assert_eq!(
        dispatched.receipt.response_id,
        accepted.message.receipt.response_id
    );
    let active = store.get_agent(&agent.id).await.unwrap().unwrap();
    let ready = finish(&store, &active, &dispatched).await;
    for expected in earlier {
        let claimed = store
            .claim_next_command(claim_request(
                &ready,
                "ordinary",
                &layer2_core::now_rfc3339(),
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(claimed.receipt, expected.receipt);
        store
            .reject_command(
                &claimed.work.lease.as_ref().unwrap().claim,
                CommandFailureCode::AuthorityRevoked,
                layer2_core::now_rfc3339(),
            )
            .await
            .unwrap();
    }
    client.database(&name).drop().await.unwrap();
}

#[path = "interrupting_message_race_tests.rs"]
mod races;
