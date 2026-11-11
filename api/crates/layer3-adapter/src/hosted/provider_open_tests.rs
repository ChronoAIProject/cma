use super::*;

#[tokio::test]
async fn open_pins_one_endpoint_and_retires_the_authenticated_link() {
    open_with_output_policy(false, false).await;
}

#[tokio::test]
async fn transport_overrides_renderer_shared_policy_in_both_directions() {
    for shared in [false, true] {
        open_with_output_policy(shared, true).await;
    }
}

#[allow(clippy::result_large_err)]
async fn open_with_output_policy(shared: bool, retained: bool) {
    let handle = RuntimeHandle::new("runtime-1");
    let launch = AgentLaunch {
        operation_id: "agent:agent-1:open:v1".into(),
        request_sha256: "a".repeat(64),
        agent_id: AgentSlot::new("agent-1"),
        cwd: "/workspace".into(),
        home: "/agent-home".into(),
        program: "codex-acp".into(),
        args: Vec::new(),
        env: BTreeMap::new(),
    };
    let fenced = FencedRequest::new(9, launch.clone());
    let control_response = ResponseFrame::OpenAgent(FencedReceipt::new(
        9,
        AgentOpenReceipt {
            operation_id: launch.operation_id.clone(),
            request_sha256: launch.request_sha256.clone(),
            agent_id: launch.agent_id.clone(),
            cwd: launch.cwd.clone(),
            home: launch.home.clone(),
            conversation_id: None,
        },
    ));

    let runtime = listener().await;
    let runtime_address = runtime.local_addr().expect("runtime address");
    let websocket_request = Arc::new(StdMutex::new(None));
    let observed_websocket = Arc::clone(&websocket_request);
    let runtime_task = tokio::spawn(async move {
        let (mut control, _) = runtime.accept().await.expect("control request accepts");
        let control_request = read_request(&mut control).await;
        control
            .write_all(&json_response(&control_response))
            .await
            .expect("control response writes");
        control.shutdown().await.expect("control response closes");

        let (websocket, _) = runtime.accept().await.expect("WebSocket accepts");
        let mut websocket = tokio_tungstenite::accept_hdr_async(
            websocket,
            move |request: &Request, response: Response| {
                let auth = request
                    .headers()
                    .get("OPEN-SANDBOX-API-KEY")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string);
                *observed_websocket
                    .lock()
                    .expect("WebSocket request record lock") =
                    Some((request.uri().to_string(), auth));
                Ok(response)
            },
        )
        .await
        .expect("WebSocket handshake succeeds");
        let mut ready = serde_json::json!({
            "t": "ready", "wireVersion": 1,
            "lanes": ["ctl", "conv"], "agentId": "agent-1",
        });
        if retained {
            ready["outputAcknowledgement"] = serde_json::json!({
                "version": 1, "streamId": uuid::Uuid::from_u128(1),
            });
        }
        websocket
            .send(Message::Text(ready.to_string()))
            .await
            .expect("Ready sends");
        let initialize = receive_websocket_json(&mut websocket).await;
        assert_eq!(initialize["method"], "initialize");
        answer_policy(
            &mut websocket,
            &initialize,
            serde_json::json!({}),
            retained,
            1,
        )
        .await;
        let session = receive_websocket_json(&mut websocket).await;
        assert_eq!(session["method"], "session/new");
        answer_policy(
            &mut websocket,
            &session,
            serde_json::json!({ "sessionId": "conversation-1" }),
            retained,
            2,
        )
        .await;
        if retained {
            retained_frame(&mut websocket, 3, serde_json::json!({
                "jsonrpc": "2.0", "method": "session/update", "params": {
                    "sessionId": "conversation-1", "update": {
                        "sessionUpdate": "agent_message_chunk", "content": {"type":"text", "text":"shared payload"},
                    },
                },
            })).await;
        }
        while let Some(frame) = websocket.next().await {
            match frame {
                Ok(Message::Close(_)) | Err(_) => break,
                _ => {}
            }
        }
        control_request
    });

    let provider = listener().await;
    let provider_address = provider.local_addr().expect("provider address");
    let endpoint = serde_json::json!({
        "endpoint": format!("{runtime_address}/proxy/3001"),
    });
    let provider_task = spawn_http(provider, json_response(&endpoint));
    let factory: Arc<dyn CheckpointArchiveStoreFactory> = Arc::new(RecordingFactory::default());
    let transport = ProviderHostedControlTransport::new(
        SandboxClient::new(format!("http://{provider_address}"), "provider-secret"),
        factory,
    )
    .expect("transport builds")
    .with_shared_runtime_output(shared);
    let (updates, mut received) = tokio::sync::mpsc::channel(4);

    let (channel, receipt) = transport
        .open_agent(
            &handle,
            fenced,
            None,
            HostedAgentOpenSettings {
                bounded_permission_state: false,
                output_json_budget: true, // Disabled transport overrides renderer policy.
                shared_runtime_output: !shared,
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
            },
            updates,
        )
        .await
        .expect("agent opens through concrete transport");
    assert_eq!(channel.conversation_id.as_deref(), Some("conversation-1"));
    assert_eq!(
        receipt.receipt.conversation_id, channel.conversation_id,
        "control receipt is completed only after ACP handshake"
    );
    assert_eq!(transport.state.lock().await.links.len(), 1);
    if retained {
        for cursor in 1..=3 {
            let update = tokio::time::timeout(Duration::from_secs(2), received.recv())
                .await
                .unwrap()
                .unwrap();
            let ConversationUpdate::RuntimeOutput { receipt, update } = update else {
                panic!("retained receipt required")
            };
            assert_eq!(receipt.source.cursor, cursor);
            if cursor == 3 {
                let effect = update.expect("domain effect required");
                assert_eq!(effect.is_shared(), shared);
                assert_eq!(std::ptr::eq(&*effect, &*effect.clone()), shared);
            }
        }
    }

    let provider_request = request_text(provider_task.await.expect("provider fixture completes"));
    assert!(provider_request
        .starts_with("GET /v1/sandboxes/runtime-1/endpoints/9090?use_server_proxy=true HTTP/1.1"));
    transport
        .retire(&handle)
        .await
        .expect("runtime links retire");
    assert!(transport.state.lock().await.links.is_empty());
    let control_request = request_text(runtime_task.await.expect("runtime fixture completes"));
    assert!(control_request.starts_with("POST /proxy/3001/runtime/control HTTP/1.1"));
    assert!(control_request
        .to_ascii_lowercase()
        .contains("open-sandbox-api-key: provider-secret"));
    assert_eq!(
        websocket_request
            .lock()
            .expect("WebSocket request record lock")
            .as_ref(),
        Some(&(
            "/proxy/3001/runtime/agents/agent-1/conversation?outputAck=1".into(),
            Some("provider-secret".into()),
        ))
    );
}

async fn retained_frame(
    socket: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    cursor: u64,
    value: serde_json::Value,
) {
    let mut frame = vec![0x04];
    frame.extend_from_slice(uuid::Uuid::from_u128(1).as_bytes());
    frame.extend_from_slice(&cursor.to_be_bytes());
    frame.extend(serde_json::to_vec(&value).unwrap());
    socket.send(Message::Binary(frame)).await.unwrap();
}

async fn answer_policy(
    socket: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    request: &serde_json::Value,
    result: serde_json::Value,
    retained: bool,
    cursor: u64,
) {
    if retained {
        retained_frame(
            socket,
            cursor,
            serde_json::json!({"jsonrpc":"2.0", "id":request["id"], "result":result}),
        )
        .await;
    } else {
        answer_websocket(socket, request, result).await;
    }
}
