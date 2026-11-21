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
        let status_reads = files.status_reads.load(std::sync::atomic::Ordering::SeqCst);
        assert!(!opened.channel.input.managed_context_may_be_ready().await);
        assert_eq!(
            files.status_reads.load(std::sync::atomic::Ordering::SeqCst),
            status_reads
        );
        assert!(opened
            .channel
            .input
            .managed_context_receipt(1)
            .await
            .is_none());
        assert_eq!(
            files.status_reads.load(std::sync::atomic::Ordering::SeqCst),
            status_reads
        );
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
    refresh_context(false).await;
}

#[tokio::test]
async fn managed_context_load_survives_the_ordinary_rpc_deadline() {
    refresh_context(true).await;
}

async fn refresh_context(slow_load: bool) {
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
                if slow_load {
                    recovery_deadline_tests::elapse_silent_recovery().await;
                }
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
    assert!(opened.channel.input.managed_context_may_be_ready().await);
    assert_eq!(
        files.status_reads.load(std::sync::atomic::Ordering::SeqCst),
        0
    );
    *opened.bridge.context_goal.lock().await = None;
    assert!(!opened.channel.input.managed_context_may_be_ready().await);
    opened
        .bridge
        .observe_context_goal(
            "retained-session",
            &serde_json::json!({
                "sessionUpdate":"session_info_update", "_meta":{"goal":null}
            }),
        )
        .await;
    assert!(opened.channel.input.managed_context_may_be_ready().await);
    opened
        .bridge
        .owned_prompt_waiters
        .store(1, std::sync::atomic::Ordering::SeqCst);
    assert!(!opened.channel.input.managed_context_may_be_ready().await);
    assert!(opened
        .channel
        .input
        .managed_context_receipt(1)
        .await
        .is_none());
    opened
        .bridge
        .owned_prompt_waiters
        .store(0, std::sync::atomic::Ordering::SeqCst);
    assert!(opened.channel.input.managed_context_may_be_ready().await);
    assert_eq!(
        files.status_reads.load(std::sync::atomic::Ordering::SeqCst),
        0
    );
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
