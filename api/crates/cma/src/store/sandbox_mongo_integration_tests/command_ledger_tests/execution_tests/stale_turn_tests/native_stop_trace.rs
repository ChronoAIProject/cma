//! Verify the recorder boundary using output from the isolated native seal fixture.
use super::settlement::replica;
use super::*;
use layer2_core::{
    agent_ownership::{Member, OwnershipCoordinator, PROTOCOL_VERSION},
    sandbox::store::{AgentConversationStore, SandboxStoreError, StaleTurnSettlement},
};
use layer3_adapter::{
    stale_turn::{SealedOutput, StaleTurnSeal, SEAL_VERSION},
    ContentBlock, ConversationInput, ConversationUpdate, PromptIdentity, RuntimeOutputReceipt,
    RuntimeOutputSource, SessionUpdate,
};
use sha2::{Digest, Sha256};

#[derive(serde::Deserialize)]
struct Trace {
    conversation: String,
    dispatch: String,
    before: Vec<String>,
    marker: String,
    last_payload_sha256: String,
}

fn evidence<T: serde::de::DeserializeOwned>(env: &str) -> T {
    let bytes = std::fs::read(std::env::var_os(env).unwrap()).unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set and the isolated native ACP trace"]
async fn native_stop_trace_recorder_ack_matches_a_fresh_checkpoint() {
    let trace: Trace = evidence("CMA_NATIVE_STOP_TRACE");
    let supervisor_ack: RuntimeOutputSource = evidence("CMA_NATIVE_STOP_ACK_EVIDENCE");
    assert_eq!(supervisor_ack.native_conversation_id, trace.conversation);
    assert_eq!(supervisor_ack.cursor, trace.before.len() as u64);
    assert_eq!(supervisor_ack.payload_sha256, trace.last_payload_sha256);
    let marker: serde_json::Value = serde_json::from_str(&trace.marker).unwrap();
    assert_eq!(marker["method"], "cma/codex/turn-stop-fence");
    assert_eq!(marker["params"]["threadId"], trace.conversation);
    assert_eq!(marker["params"]["dispatchKey"], trace.dispatch);

    let (client, name) = database("native_stop_trace").await;
    let mut parent = active_sandbox("sbx_stale_turn");
    parent.lifecycle.home = Some(Box::new(
        crate::store::sandbox_unpublished_recovery::mongo_integration_tests::home(&parent.id),
    ));
    let runtime = parent.runtime.as_mut().unwrap();
    runtime.runtime_id = "agent-sandbox:member:sandbox-uid".into();
    runtime.boot_id = Some("b".repeat(32));
    let (dispatched, mut dispatches) = tokio::sync::mpsc::channel(1);
    let input = ConversationInputSender::spawn(2, move |message| {
        let dispatched = dispatched.clone();
        async move {
            dispatched.send(message).await.unwrap();
            Ok(())
        }
    });
    let (store, agent, parent, execution, _, sends) = composed_with_input_history(
        &client,
        &name,
        Arc::new(FixedSandboxCredentialPrerequisite::Ready),
        parent,
        Some((6, false)),
        Some((input.clone(), trace.conversation.clone())),
    )
    .await;
    let writer = replica(&client, &name).await;
    let ownership = OwnershipCoordinator::register(
        writer.agent_ownership.as_ref().unwrap().clone(),
        Member {
            instance_id: "test-owner".into(),
            protocol: PROTOCOL_VERSION,
            relay_address: "http://127.0.0.1:1".into(),
            expires_at_ms: 0,
            draining: false,
            owned_agents: 0,
            max_owned_agents: 1,
        },
    )
    .await
    .unwrap();
    let owner = ownership.acquire(&agent.id).await.unwrap().unwrap();
    let registry = Arc::new(LiveAgentRegistry::default());
    registry.install_ownership(ownership);
    let (output, receiver) = tokio::sync::mpsc::channel(16);
    let generation = agent.conversation.runtime_generation.unwrap();
    owner
        .run(async {
            let attachment = registry
                .attach(
                    agent.id.clone(),
                    trace.conversation.clone(),
                    generation,
                    1,
                    agent.capabilities.clone(),
                    input,
                    receiver,
                    DurableAgentUpdateRecorder::new(writer.clone()),
                )
                .unwrap();
            registry.publish_and_activate(&attachment).unwrap();
        })
        .await
        .unwrap();
    let admitted = record(
        store
            .reserve_command(live_request(&agent, &parent, "native-trace"))
            .await
            .unwrap(),
    );
    let id = &admitted.receipt.envelope.command_id;
    let worker = AgentCommandWorker::new(store.clone(), execution, "trace-worker".into()).unwrap();
    worker.run_one(id).await.unwrap();
    store.reconcile_command(id).await.unwrap();
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    let active = store.get_agent(&agent.id).await.unwrap().unwrap();
    let turn = active.conversation.active_turn.unwrap();
    let dispatched_head = active.conversation.last_seq;
    assert_eq!(
        PromptIdentity::new(&trace.conversation, generation, turn).as_str(),
        trace.dispatch
    );
    let ConversationInput::RecoverablePrompt { prompt_id, .. } = dispatches.try_recv().unwrap()
    else {
        panic!("the command must dispatch a bound prompt");
    };
    assert_eq!(prompt_id.as_str(), trace.dispatch);

    // Decode the captured protocol effects and preserve each exact payload's hash.
    let (acknowledgements, mut acks) = tokio::sync::mpsc::channel(16);
    let mut expected_messages = Vec::new();
    for (index, payload) in trace.before.iter().enumerate() {
        let frame: serde_json::Value = serde_json::from_str(payload).unwrap();
        let update = if frame["method"] == "session/update" {
            assert_eq!(frame["params"]["sessionId"], trace.conversation);
            let update: SessionUpdate =
                serde_json::from_value(frame["params"]["update"].clone()).unwrap();
            if let SessionUpdate::AgentMessageChunk(chunk) = &update {
                if let ContentBlock::Text(text) = &chunk.content {
                    expected_messages.push(text.text.clone());
                }
            }
            Some(Box::new(ConversationUpdate::Said(update)).into())
        } else {
            assert!(frame.get("result").is_some());
            assert_ne!(frame["id"], trace.dispatch);
            None
        };
        output
            .send(ConversationUpdate::RuntimeOutput {
                receipt: RuntimeOutputReceipt::new(
                    RuntimeOutputSource {
                        stream_id: supervisor_ack.stream_id.clone(),
                        cursor: index as u64 + 1,
                        native_conversation_id: trace.conversation.clone(),
                        payload_sha256: format!("{:x}", Sha256::digest(payload.as_bytes())),
                    },
                    acknowledgements.clone(),
                ),
                update,
            })
            .await
            .unwrap();
    }
    assert_eq!(expected_messages.len(), 2);
    assert_eq!(expected_messages[1], "One partial model response.");
    let final_ack = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let ack = acks.recv().await.unwrap();
            if ack.cursor >= supervisor_ack.cursor {
                break ack;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(final_ack, supervisor_ack);

    let peer_client = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let peer = replica(&peer_client, &name).await;
    let snapshot = peer
        .agent_message_snapshot(&agent.id)
        .await
        .unwrap()
        .unwrap();
    let checkpoint = snapshot.checkpoint.as_ref().unwrap();
    assert!(checkpoint.matches(&snapshot.agent));
    assert_eq!(checkpoint.active_turn, Some(turn));
    assert_eq!(checkpoint.output.as_ref(), Some(&final_ack));

    assert!(!checkpoint.assembly.is_empty());
    let row = peer
        .agent_commands
        .find_one(doc! { "_id": id.as_str() })
        .await
        .unwrap()
        .unwrap();
    let future = DateTime::from_millis(DateTime::now().timestamp_millis() + 16 * 60 * 1000);
    let candidate = Candidate::from_row(&row, future).unwrap();
    assert!(matches!(
        StaleTurnStore::new(peer.clone())
            .observe(&candidate, future, &mut ProofBudget::default())
            .await
            .unwrap(),
        Observation::Hold("output_uncommitted")
    ));
    let scan = StaleTurnStore::new(writer);
    let Observation::Quiet { evidence, .. } = scan
        .observe_for_terminal(&candidate, future, &mut ProofBudget::default())
        .await
        .unwrap()
    else {
        panic!("the terminal lookup needs the exact durable partial checkpoint");
    };
    assert!(matches!(
        StaleTurnSettlement::target(
            &evidence.snapshot,
            &evidence.parent,
            &evidence.command,
            &owner.token,
        ),
        Err(SandboxStoreError::Conflict)
    ));
    let target = StaleTurnSettlement::receipt_target(
        &evidence.snapshot,
        &evidence.parent,
        &evidence.command,
        &owner.token,
    )
    .unwrap();
    assert_eq!(target.prompt().as_str(), trace.dispatch);
    let seal = StaleTurnSeal {
        version: SEAL_VERSION,
        target,
        prompt_id: trace.dispatch,
        stop_receipt_id: marker["params"]["receipt"].as_str().unwrap().into(),
        output: SealedOutput::Through { source: final_ack },
    };
    let prepare = |seal| {
        StaleTurnSettlement::prepare(
            evidence.snapshot.clone(),
            evidence.parent.clone(),
            evidence.command.clone(),
            owner.token.clone(),
            seal,
            layer2_core::now_rfc3339(),
        )
    };
    let mut wrong_digest = seal.clone();
    let SealedOutput::Through { source } = &mut wrong_digest.output else {
        unreachable!();
    };
    source.payload_sha256 = "0".repeat(64);
    assert!(matches!(
        prepare(wrong_digest),
        Err(SandboxStoreError::Conflict)
    ));
    let proof = prepare(seal).unwrap();
    owner
        .run(async {
            for _ in 0..2 {
                scan.settle(&proof, &mut ProofBudget::default())
                    .await
                    .unwrap();
            }
        })
        .await
        .unwrap();
    let logs = peer
        .agent_log(&agent.id, dispatched_head, 100)
        .await
        .unwrap();
    let messages = logs
        .iter()
        .filter_map(|entry| match &entry.kind {
            AgentLogKind::AgentMessage { content } => Some(content),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(messages, expected_messages.iter().collect::<Vec<_>>());
    assert_eq!(
        logs.iter()
            .filter(|entry| matches!(&entry.kind, AgentLogKind::TurnFailed { turn_sequence, failure } if *turn_sequence == turn && failure.class == "agent_turn_no_progress"))
            .count(),
        1
    );
    assert_eq!(
        peer.reconcile_command(id).await.unwrap().status,
        AgentResponseStatus::Failed
    );
    assert_eq!(
        peer.get_command(id).await.unwrap().unwrap().status,
        AgentResponseStatus::Failed
    );
    drop(output);
    drop(registry);
    client.database(&name).drop().await.unwrap();
}
