use super::*;

#[tokio::test]
async fn managed_context_unknown_or_native_goal_never_writes_or_closes_the_thread() {
    let (client, mut server) = socket_pair().await;
    send_ready(&mut server, "agent-1").await;
    let serving = tokio::spawn(async move {
        let initialize = receive_json(&mut server).await;
        respond(&mut server, &initialize, serde_json::json!({})).await;
        let open = receive_json(&mut server).await;
        respond(
            &mut server,
            &open,
            serde_json::json!({"sessionId":"retained-session"}),
        )
        .await;
        while let Some(Ok(frame)) = server.next().await {
            assert!(
                !matches!(frame, Message::Binary(_)),
                "maintenance must not close a potentially busy thread"
            );
        }
    });
    let (binding, files) = crate::hosted::managed_context::writer_tests::binding(0);
    let mut config = settings(None, false);
    config.context_refresh = Some(binding);
    let (updates, _receiver) = tokio::sync::mpsc::channel(8);
    let opened = Bridge::open(
        client,
        "agent-1",
        "/workspace".into(),
        config,
        None,
        updates,
    )
    .await
    .unwrap();
    for goal in [
        None,
        Some(serde_json::json!({"status":"active"})),
        Some(serde_json::json!({"status":"paused"})),
        Some(serde_json::Value::Null),
    ] {
        if let Some(goal) = goal {
            let update =
                serde_json::json!({"sessionUpdate":"session_info_update","_meta":{"goal":goal}});
            opened
                .bridge
                .observe_context_goal("retained-session", &update)
                .await;
        }
        assert!(opened
            .channel
            .input
            .managed_context_receipt(1)
            .await
            .is_none());
        let outcome = opened
            .channel
            .input
            .send_classified(ConversationInput::RefreshContext {
                service_origin: None,
                operation_sequence: 1,
                readiness_receipt: "invalid".into(),
                guidance: "New managed".into(),
                previous_guidance: vec!["Old managed".into()],
                model: "gpt-6".into(),
                reasoning_effort: crate::ReasoningEffort::High,
                permission: crate::PermissionLevel::Autonomous,
                working_directory: None,
            })
            .await;
        assert!(matches!(
            outcome,
            crate::ConversationSendOutcome::PreWriteFailure(
                crate::AdapterError::OperationIdentityConflict(_)
            )
        ));
        assert_eq!(files.writes.load(std::sync::atomic::Ordering::SeqCst), 0);
    }
    stop(opened).await;
    serving.await.unwrap();
}

#[tokio::test]
async fn managed_context_null_goal_loads_exact_thread_and_settings_after_file_ack() {
    let (client, mut server) = socket_pair().await;
    send_ready(&mut server, "agent-1").await;
    let (binding, files) = crate::hosted::managed_context::writer_tests::binding(0);
    let checked_files = files.clone();
    let serving = tokio::spawn(async move {
        let initialize = receive_json(&mut server).await;
        respond(&mut server, &initialize, serde_json::json!({})).await;
        let open = receive_json(&mut server).await;
        send_json(&mut server, serde_json::json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"retained-session","update":{"sessionUpdate":"session_info_update","_meta":{"goal":null}}}})).await;
        respond(
            &mut server,
            &open,
            serde_json::json!({"sessionId":"retained-session"}),
        )
        .await;
        for method in [
            "session/close",
            "session/load",
            "session/set_model",
            "session/set_config_option",
            "session/set_mode",
        ] {
            let request = receive_json(&mut server).await;
            assert_eq!(request["method"], method);
            assert_eq!(request["params"]["sessionId"], "retained-session");
            assert_eq!(
                checked_files
                    .writes
                    .load(std::sync::atomic::Ordering::SeqCst),
                1
            );
            if method == "session/load" {
                assert_eq!(request["params"]["cwd"], "/home/cma/work");
            }
            let result = if method == "session/set_config_option" {
                serde_json::json!({"configOptions":[{"id":"reasoning_effort","currentValue":"high"}]})
            } else {
                serde_json::json!({})
            };
            respond(&mut server, &request, result).await;
        }
        while server.next().await.is_some() {}
    });
    let mut config = settings(None, false);
    config.context_refresh = Some(binding);
    let (updates, _receiver) = tokio::sync::mpsc::channel(8);
    let opened = Bridge::open(
        client,
        "agent-1",
        "/workspace".into(),
        config,
        None,
        updates,
    )
    .await
    .unwrap();
    let receipt = opened
        .channel
        .input
        .managed_context_receipt(1)
        .await
        .unwrap();
    // Reattachment can lose the native goal snapshot, but the admitted exact
    // runtime receipt allows this already-fenced operation to recover.
    *opened.bridge.context_goal.lock().await = None;
    let result = opened
        .channel
        .input
        .send_classified(ConversationInput::RefreshContext {
            service_origin: None,
            operation_sequence: 1,
            readiness_receipt: receipt,
            guidance: "New managed".into(),
            previous_guidance: vec!["Old managed".into()],
            model: "gpt-6".into(),
            reasoning_effort: crate::ReasoningEffort::High,
            permission: crate::PermissionLevel::Autonomous,
            working_directory: None,
        })
        .await;
    assert!(matches!(
        result,
        crate::ConversationSendOutcome::WriteConfirmed
    ));
    assert_eq!(files.writes.load(std::sync::atomic::Ordering::SeqCst), 1);
    stop(opened).await;
    serving.await.unwrap();
}

#[tokio::test]
async fn transport_confirmation_follows_exact_reload_and_settings_acknowledgements() {
    use crate::hosted::{HostedAccessToolBinding, HostedAccessToolSource};
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Source {
        old: HostedAccessToolBinding,
        new: HostedAccessToolBinding,
        confirmed: AtomicUsize,
    }
    #[async_trait::async_trait]
    impl HostedAccessToolSource for Source {
        async fn resolve(
            &self,
            _: &crate::RuntimeHandle,
            _: &crate::AgentOpenRequest,
            _: u64,
        ) -> crate::Result<Option<HostedAccessToolBinding>> {
            unreachable!()
        }
        async fn confirm_open(&self, _: &HostedAccessToolBinding, _: &str) -> crate::Result<()> {
            unreachable!()
        }
        async fn prepare_transport(
            &self,
            binding: &HostedAccessToolBinding,
            origin: &str,
            conversation: &str,
            sequence: u64,
        ) -> crate::Result<HostedAccessToolBinding> {
            assert_eq!(binding, &self.old);
            assert_eq!(origin, "https://new.example");
            assert_eq!(conversation, "retained-session");
            assert_eq!(sequence, 1);
            Ok(self.new.clone())
        }
        async fn confirm_transport(
            &self,
            binding: &HostedAccessToolBinding,
            conversation: &str,
            sequence: u64,
        ) -> crate::Result<()> {
            assert_eq!(binding, &self.new);
            assert_eq!(conversation, "retained-session");
            assert_eq!(sequence, 1);
            self.confirmed.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }
    let old = access_tool_tests::binding(Some("retained-session"));
    let request = crate::AgentOpenRequest::new(
        "open-access-fixture",
        crate::RuntimeOptions {
            slot: crate::AgentSlot::new("agent-1"),
            ..Default::default()
        },
        None,
    );
    let new = HostedAccessToolBinding::new(
        "https://new.example",
        crate::RuntimeCredential::new(format!("cma_at_{}", "a".repeat(64))),
        crate::RuntimeHandle("runtime-1".into()),
        &request,
        1,
        Some("retained-session".into()),
    )
    .unwrap();
    let source = Arc::new(Source {
        old: old.clone(),
        new: new.clone(),
        confirmed: AtomicUsize::new(0),
    });
    let unchanged_tool = serde_json::json!({"name":"owner_tool","type":"http","url":"https://owner.example/mcp","headers":[]});
    let (context, _) = crate::hosted::managed_context::writer_tests::binding_with_tools(
        0,
        vec![old.acp_server(), unchanged_tool.clone()],
    );
    let context = context.with_access(source.clone(), old);
    let retained_context = context.clone();
    let (client, mut server) = socket_pair().await;
    send_ready(&mut server, "agent-1").await;
    let confirmation = source.clone();
    let serving = tokio::spawn(async move {
        let initialize = receive_json(&mut server).await;
        respond(&mut server, &initialize, serde_json::json!({})).await;
        let open = receive_json(&mut server).await;
        respond(
            &mut server,
            &open,
            serde_json::json!({"sessionId":"retained-session"}),
        )
        .await;
        for method in [
            "session/close",
            "session/load",
            "session/set_model",
            "session/set_config_option",
            "session/set_mode",
        ] {
            let call = receive_json(&mut server).await;
            assert_eq!(call["method"], method);
            assert_eq!(confirmation.confirmed.load(Ordering::SeqCst), 0);
            if method == "session/load" {
                assert_eq!(call["params"]["sessionId"], "retained-session");
                assert_eq!(
                    call["params"]["mcpServers"],
                    serde_json::json!([new.acp_server(), unchanged_tool])
                );
            }
            let result = if method == "session/set_config_option" {
                serde_json::json!({"configOptions":[{"id":"reasoning_effort","currentValue":"high"}]})
            } else {
                serde_json::json!({})
            };
            respond(&mut server, &call, result).await;
        }
        while server.next().await.is_some() {}
    });
    let mut config = settings(None, false);
    config.context_refresh = Some(context);
    let (updates, _receiver) = tokio::sync::mpsc::channel(8);
    let opened = Bridge::open(
        client,
        "agent-1",
        "/workspace".into(),
        config,
        None,
        updates,
    )
    .await
    .unwrap();
    opened
        .bridge
        .observe_context_goal(
            "retained-session",
            &serde_json::json!({"sessionUpdate":"session_info_update","_meta":{"goal":null}}),
        )
        .await;
    let receipt = opened
        .channel
        .input
        .managed_context_receipt(1)
        .await
        .unwrap();
    opened
        .channel
        .input
        .send(ConversationInput::RefreshContext {
            service_origin: Some("https://new.example".into()),
            operation_sequence: 1,
            readiness_receipt: receipt,
            guidance: "New managed".into(),
            previous_guidance: vec!["Old managed".into()],
            model: "gpt-6".into(),
            reasoning_effort: crate::ReasoningEffort::High,
            permission: crate::PermissionLevel::Autonomous,
            working_directory: None,
        })
        .await
        .unwrap();
    assert_eq!(source.confirmed.load(Ordering::SeqCst), 1);
    assert_eq!(
        retained_context.tools()[0]["url"],
        "https://new.example/api/v1/agent-tools"
    );
    stop(opened).await;
    serving.await.unwrap();
}
