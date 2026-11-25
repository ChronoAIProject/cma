use super::*;
use axum::{
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    routing::{get, post},
    Json, Router,
};
use layer3_adapter::{
    hosted::{HostedAgentOpenSettings, HostedControlTransport, ProviderHostedControlTransport},
    provider::SandboxClient,
    wire::{AgentOpenReceipt, FencedReceipt, FencedRequest, RequestFrame, ResponseFrame},
    AgentLaunch, AgentSlot, RuntimeHandle,
};
use sha2::{Digest, Sha256};
use tokio::{
    net::TcpListener,
    sync::{mpsc, oneshot},
    time::timeout,
};

const SLOT: &str = "agt_commands";
const CONVERSATION: &str = "fixture-recorder-conversation";
const CURSOR: u64 = 3;
const CONVERSATION_LANE: u8 = 0x02;
const RETAINED_OUTPUT_LANE: u8 = 0x04;
const OUTPUT_ACK_LANE: u8 = 0x05;
const ACP_PAYLOAD: &[u8] = br#"{ "jsonrpc":"2.0","method":"session/update","params":{"sessionId":"fixture-recorder-conversation","update":{"sessionUpdate":"available_commands_update","availableCommands":[]}} }"#;
const DEADLINE: Duration = Duration::from_secs(10);

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn stale_turn_settlement_compares_the_acknowledged_acp_checkpoint() {
    let (client, name) = database("stale_turn_recorder_checkpoint").await;
    let mut parent = active_sandbox("sbx_stale_turn_recorder");
    parent.lifecycle.home = Some(Box::new(
        crate::store::sandbox_unpublished_recovery::mongo_integration_tests::home(&parent.id),
    ));
    let runtime = parent.runtime.as_mut().unwrap();
    runtime.runtime_id = "agent-sandbox:member:sandbox-uid".into();
    runtime.boot_id = Some("b".repeat(32));
    let handle = RuntimeHandle::new(runtime.runtime_id.clone());
    let stream = uuid::Uuid::new_v4();
    let (observed_tx, observed_rx) = oneshot::channel();
    let (dispatch_tx, dispatch_rx) = oneshot::channel();
    let attachment = Arc::new(tokio::sync::Mutex::new(Some((dispatch_rx, observed_tx))));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let routes = Router::new()
        .route(
            "/v1/sandboxes/{runtime}/endpoints/9090",
            get(move || async move { Json(serde_json::json!({"endpoint": address.to_string()})) }),
        )
        .route("/runtime/control", post(open_receipt))
        .route(
            "/runtime/agents/{agent}/conversation",
            get(move |upgrade: WebSocketUpgrade| {
                let attachment = attachment.clone();
                async move {
                    let (dispatch, observed) =
                        attachment.lock().await.take().expect("one ACP attachment");
                    upgrade.on_upgrade(move |socket| serve_acp(socket, stream, dispatch, observed))
                }
            }),
        );
    let server = tokio::spawn(async move { axum::serve(listener, routes).await.unwrap() });
    let transport = ProviderHostedControlTransport::for_retained_opensandbox(SandboxClient::new(
        format!("http://{address}"),
        "synthetic-credential",
    ))
    .unwrap();
    let (updates, received) = mpsc::channel(8);
    let (channel, _) = timeout(
        DEADLINE,
        transport.open_agent(
            &handle,
            FencedRequest::new(1, launch()),
            None,
            settings(),
            updates,
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(channel.conversation_id.as_deref(), Some(CONVERSATION));

    let (original, agent, parent, execution, _, sends) = composed_with_input_history(
        &client,
        &name,
        Arc::new(FixedSandboxCredentialPrerequisite::Ready),
        parent,
        Some((competing::history_rows(None), false)),
        Some((channel.input.clone(), CONVERSATION.into())),
    )
    .await;
    let command = record(
        original
            .reserve_command(live_request(&agent, &parent, "recorder-dispatch"))
            .await
            .unwrap(),
    );
    let id = &command.receipt.envelope.command_id;
    let writer = replica(&client, &name).await;
    let reader_client = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let reader = replica(&reader_client, &name).await;
    let coordinator = OwnershipCoordinator::register(
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
    let owner = coordinator.acquire(&agent.id).await.unwrap().unwrap();
    dispatch_tx
        .send(())
        .expect("ACP attachment is waiting for dispatch");
    AgentCommandWorker::new(original.clone(), execution, "recorder-worker".into())
        .unwrap()
        .run_one(id)
        .await
        .unwrap();
    original.reconcile_command(id).await.unwrap();

    let registry = Arc::new(LiveAgentRegistry::default());
    registry.install_ownership(coordinator.clone());
    owner
        .run(async {
            let attachment = registry
                .attach(
                    agent.id.clone(),
                    CONVERSATION.into(),
                    agent.conversation.runtime_generation.unwrap(),
                    1,
                    agent.capabilities.clone(),
                    channel.input,
                    received,
                    DurableAgentUpdateRecorder::new(writer.clone()),
                )
                .unwrap();
            registry.publish_and_activate(&attachment).unwrap();
        })
        .await
        .unwrap();

    let prompt_id = timeout(DEADLINE, observed_rx).await.unwrap().unwrap();
    let source = RuntimeOutputSource {
        stream_id: stream.to_string(),
        cursor: CURSOR,
        native_conversation_id: CONVERSATION.into(),
        payload_sha256: format!("{:x}", Sha256::digest(ACP_PAYLOAD)),
    };
    let observed = reader
        .agent_message_checkpoint(&agent.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(observed.output.as_ref(), Some(&source));
    assert!(observed.assembly.is_empty());
    // Project the newly recorded Agent event before capturing the command
    // revision. The mismatch assertions below only read that captured state.
    assert_eq!(
        reader.reconcile_command(id).await.unwrap().status,
        AgentResponseStatus::InProgress
    );

    let row = reader
        .agent_commands
        .find_one(doc! {"_id": id.as_str()})
        .await
        .unwrap()
        .unwrap();
    let future = DateTime::from_millis(DateTime::now().timestamp_millis() + 16 * 60 * 1000);
    let candidate = Candidate::from_row(&row, future).unwrap();
    let scan = StaleTurnStore::new(reader.clone());
    let Observation::Quiet { evidence, .. } = scan
        .observe(&candidate, future, &mut ProofBudget::default())
        .await
        .unwrap()
    else {
        panic!("expected fresh durable checkpoint on second client");
    };
    let target = StaleTurnSettlement::target(
        &evidence.snapshot,
        &evidence.parent,
        &evidence.command,
        &owner.token,
    )
    .unwrap();
    assert_eq!(target.prompt().as_str(), prompt_id);
    assert_eq!(target.command_id, id.as_str());
    let synthetic_seal = StaleTurnSeal {
        version: SEAL_VERSION,
        prompt_id: target.prompt().as_str().to_owned(),
        target,
        stop_receipt_id: "synthetic-producer-seal".into(),
        output: SealedOutput::Through {
            source: source.clone(),
        },
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
    let mut wrong_digest = synthetic_seal.clone();
    wrong_digest.output = SealedOutput::Through {
        source: RuntimeOutputSource {
            payload_sha256: format!("{:x}", Sha256::digest(b"different ACP payload")),
            ..source.clone()
        },
    };
    let mut wrong_stream = synthetic_seal.clone();
    wrong_stream.output = SealedOutput::Through {
        source: RuntimeOutputSource {
            stream_id: uuid::Uuid::new_v4().to_string(),
            ..source
        },
    };
    let mut wrong_dispatch = synthetic_seal.clone();
    wrong_dispatch.target.command_id = "different-command".into();
    let log_before = reader.agent_log(&agent.id, 0, 100).await.unwrap();
    for (mismatch, seal) in [
        ("digest at the same cursor", wrong_digest),
        ("stream", wrong_stream),
        ("dispatch", wrong_dispatch),
    ] {
        assert!(
            matches!(prepare(seal), Err(SandboxStoreError::Conflict)),
            "{mismatch}"
        );
        let command = reader
            .agent_commands
            .find_one(doc! {"_id": id.as_str()})
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            crate::store::agent_commands::records::read(&command).unwrap(),
            evidence.command
        );
        assert_eq!(
            reader.agent_log(&agent.id, 0, 100).await.unwrap(),
            log_before
        );
    }

    let proof = prepare(synthetic_seal).unwrap();
    owner
        .run(async {
            scan.settle(&proof, &mut ProofBudget::default())
                .await
                .unwrap();
            let retry = StaleTurnStore::new(writer.clone());
            retry
                .settle(&proof, &mut ProofBudget::default())
                .await
                .unwrap();
        })
        .await
        .unwrap();
    assert_eq!(
        reader.reconcile_command(id).await.unwrap().status,
        AgentResponseStatus::Failed
    );
    let logs = reader.agent_log(&agent.id, 0, 100).await.unwrap();
    assert_eq!(
        logs.iter()
            .filter(
                |row| matches!(&row.kind, AgentLogKind::TurnFailed { failure, .. }
        if failure.class == "agent_turn_no_progress")
            )
            .count(),
        1
    );
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    registry.detach_owner(&owner.token);
    transport.retire(&handle).await.unwrap();
    server.abort();
    client.database(&name).drop().await.unwrap();
}

fn launch() -> AgentLaunch {
    AgentLaunch {
        operation_id: "fixture-acp-open".into(),
        request_sha256: "a".repeat(64),
        agent_id: AgentSlot::new(SLOT),
        cwd: "/workspace".into(),
        home: "/agent-home".into(),
        program: "synthetic-acp".into(),
        args: Vec::new(),
        env: Default::default(),
    }
}

fn settings() -> HostedAgentOpenSettings {
    HostedAgentOpenSettings {
        coordinated_ingress: false,
        retained_json: false,
        bounded_raw_output: false,
        bounded_permission_state: false,
        output_json_budget: false,
        shared_runtime_output: false,
        already_initialized_error: None,
        open_completion: None,
        context_refresh: None,
        permission_mode: None,
        own_credential: false,
        channel_tool: None,
        provider_tool: None,
        access_tool: None,
        recovered_prompt: None,
        recovered_context_sequence: None,
    }
}

async fn open_receipt(Json(request): Json<RequestFrame>) -> Json<ResponseFrame> {
    let RequestFrame::OpenAgent(FencedRequest {
        authority_fence,
        request,
        restart_runtime: None,
    }) = request
    else {
        panic!("expected an open request");
    };
    Json(ResponseFrame::OpenAgent(FencedReceipt::new(
        authority_fence,
        AgentOpenReceipt {
            operation_id: request.operation_id,
            request_sha256: request.request_sha256,
            agent_id: request.agent_id,
            cwd: request.cwd,
            home: request.home,
            conversation_id: None,
        },
    )))
}

fn retained_frame(lane: u8, stream: uuid::Uuid, cursor: u64, payload: &[u8]) -> Vec<u8> {
    let mut frame = vec![lane];
    frame.extend_from_slice(stream.as_bytes());
    frame.extend_from_slice(&cursor.to_be_bytes());
    frame.extend_from_slice(payload);
    frame
}

async fn binary(socket: &mut WebSocket, phase: &str) -> Vec<u8> {
    let Message::Binary(frame) = timeout(DEADLINE, socket.recv())
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for ACP {phase}"))
        .unwrap()
        .unwrap()
    else {
        panic!("expected binary ACP frame");
    };
    frame.to_vec()
}

async fn serve_acp(
    mut socket: WebSocket,
    stream: uuid::Uuid,
    dispatch: oneshot::Receiver<()>,
    observed: oneshot::Sender<String>,
) {
    let ready = serde_json::json!({
        "t": "ready", "wireVersion": 1, "lanes": ["ctl", "conv"], "agentId": SLOT,
        "outputAcknowledgement": {"version": 1, "streamId": stream},
    });
    socket
        .send(Message::Text(ready.to_string().into()))
        .await
        .unwrap();
    for (cursor, method, result) in [
        (1, "initialize", serde_json::json!({})),
        (
            2,
            "session/new",
            serde_json::json!({"sessionId": CONVERSATION}),
        ),
    ] {
        let frame = binary(&mut socket, method).await;
        assert_eq!(frame[0], CONVERSATION_LANE);
        let rpc: serde_json::Value = serde_json::from_slice(&frame[1..]).unwrap();
        assert_eq!(rpc["method"], method);
        let reply = serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0", "id": rpc["id"], "result": result,
        }))
        .unwrap();
        socket
            .send(Message::Binary(
                retained_frame(RETAINED_OUTPUT_LANE, stream, cursor, &reply).into(),
            ))
            .await
            .unwrap();
    }
    // Replica setup may outlast an individual ACP exchange.
    if dispatch.await.is_err() {
        return;
    }
    let frame = binary(&mut socket, "prompt").await;
    assert_eq!(frame[0], CONVERSATION_LANE);
    let prompt: serde_json::Value = serde_json::from_slice(&frame[1..]).unwrap();
    assert_eq!(prompt["method"], "session/prompt");
    assert_eq!(prompt["params"]["sessionId"], CONVERSATION);
    let prompt_id = prompt["id"].as_str().unwrap().to_owned();
    socket
        .send(Message::Binary(
            retained_frame(RETAINED_OUTPUT_LANE, stream, CURSOR, ACP_PAYLOAD).into(),
        ))
        .await
        .unwrap();
    let mut acknowledged = 0;
    while acknowledged < CURSOR {
        let ack = binary(&mut socket, "output acknowledgement").await;
        assert_eq!(ack.len(), 25);
        let cursor = u64::from_be_bytes(ack[17..].try_into().unwrap());
        assert!((acknowledged + 1..=CURSOR).contains(&cursor));
        assert_eq!(ack, retained_frame(OUTPUT_ACK_LANE, stream, cursor, &[]));
        acknowledged = cursor;
    }
    observed.send(prompt_id).unwrap();
    let _ = socket.recv().await;
}
