use super::*;

#[tokio::test]
#[allow(clippy::result_large_err)]
async fn open_pins_one_endpoint_and_overrides_raw_output_policy_both_ways() {
    for enabled in [false, true] {
        open_with_raw_output_policy(enabled).await;
    }
}

#[allow(clippy::result_large_err)]
async fn open_with_raw_output_policy(enabled: bool) {
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
        websocket
            .send(Message::Text(
                serde_json::json!({
                    "t": "ready", "wireVersion": 1,
                    "lanes": ["ctl", "conv"], "agentId": "agent-1",
                    "outputAcknowledgement": {"version":1,"streamId":uuid::Uuid::from_u128(33)},
                })
                .to_string(),
            ))
            .await
            .expect("Ready sends");
        let initialize = receive_websocket_json(&mut websocket).await;
        assert_eq!(initialize["method"], "initialize");
        answer_retained(&mut websocket, 1, &initialize, serde_json::json!({})).await;
        let session = receive_websocket_json(&mut websocket).await;
        assert_eq!(session["method"], "session/new");
        answer_retained(
            &mut websocket,
            2,
            &session,
            serde_json::json!({ "sessionId": "conversation-1" }),
        )
        .await;
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
    .with_bounded_raw_output(enabled);
    let (updates, _) = tokio::sync::mpsc::channel(4);

    let (channel, receipt) = transport
        .open_agent(
            &handle,
            fenced,
            None,
            HostedAgentOpenSettings {
                bounded_raw_output: !enabled,
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
    let state = transport.state.lock().await;
    let bridge = state.links.values().next().unwrap().bridge.clone();
    drop(state);
    assert_eq!(bridge.bounded_raw_output, enabled);
    assert_eq!(transport.state.lock().await.links.len(), 1);

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

async fn answer_retained(
    socket: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    cursor: u64,
    request: &serde_json::Value,
    result: serde_json::Value,
) {
    let mut bytes = vec![0x04];
    bytes.extend_from_slice(uuid::Uuid::from_u128(33).as_bytes());
    bytes.extend_from_slice(&cursor.to_be_bytes());
    bytes.extend(
        serde_json::to_vec(
            &serde_json::json!({"jsonrpc":"2.0","id":request["id"],"result":result}),
        )
        .unwrap(),
    );
    socket.send(Message::Binary(bytes)).await.unwrap();
}
