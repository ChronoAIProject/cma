use super::*;

#[tokio::test]
async fn enabled_transport_cannot_be_downgraded_by_renderer_settings() {
    let handle = RuntimeHandle::new("runtime-policy");
    let launch = AgentLaunch {
        operation_id: "agent:agent-policy:open:v1".into(),
        request_sha256: "b".repeat(64),
        agent_id: AgentSlot::new("agent-policy"),
        cwd: "/workspace".into(),
        home: "/agent-home".into(),
        program: "codex-acp".into(),
        args: Vec::new(),
        env: BTreeMap::new(),
    };
    let response = ResponseFrame::OpenAgent(FencedReceipt::new(
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
    let runtime_address = runtime.local_addr().unwrap();
    let runtime_task = tokio::spawn(async move {
        let (mut control, _) = runtime.accept().await.unwrap();
        read_request(&mut control).await;
        control.write_all(&json_response(&response)).await.unwrap();
        control.shutdown().await.unwrap();
        let (stream, _) = runtime.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        socket
            .send(Message::Text(
                serde_json::json!({"t":"ready", "wireVersion":1,
                    "lanes":["ctl","conv"], "agentId":"agent-policy"})
                .to_string(),
            ))
            .await
            .unwrap();
        // The legacy runtime cannot see initialize or a prompt before refusal.
        let next = tokio::time::timeout(Duration::from_secs(2), socket.next())
            .await
            .expect("refused attachment closes");
        assert!(matches!(
            next,
            None | Some(Ok(Message::Close(_))) | Some(Err(_))
        ));
    });
    let provider = listener().await;
    let provider_address = provider.local_addr().unwrap();
    let provider_task = spawn_http(
        provider,
        json_response(&serde_json::json!({"endpoint": format!("{runtime_address}/proxy/3001")})),
    );
    let transport = ProviderHostedControlTransport::for_retained_opensandbox(SandboxClient::new(
        format!("http://{provider_address}"),
        "fixture",
    ))
    .unwrap()
    .with_output_json_budget(true);
    let (updates, mut received) = tokio::sync::mpsc::channel(4);
    let result = transport
        .open_agent(
            &handle,
            FencedRequest::new(9, launch),
            None,
            HostedAgentOpenSettings {
                output_json_budget: false,
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
        .await;
    assert!(
        matches!(result, Err(AdapterError::Rejected(message)) if message.contains("requires retained"))
    );
    assert!(transport.state.lock().await.links.is_empty());
    assert!(received.recv().await.is_none());
    provider_task.await.unwrap();
    runtime_task.await.unwrap();
    transport.retire(&handle).await.unwrap();
}
