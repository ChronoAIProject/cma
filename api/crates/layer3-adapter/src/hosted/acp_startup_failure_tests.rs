use super::*;

#[tokio::test]
async fn lost_new_session_identity_requires_explicit_recovery_instead_of_claiming_unsupported() {
    for retry in [false, true] {
        let (client, mut server) = socket_pair().await;
        send_ready(&mut server, "agent-1").await;
        let serving = tokio::spawn(async move {
            let initialize = receive_json(&mut server).await;
            if retry {
                reject(&mut server, &initialize).await;
                // The only frame allowed is the read-only adoption probe; this
                // runtime cannot list completed opens, so nothing is adopted.
                let probe = receive_json(&mut server).await;
                assert_eq!(probe["method"], "cma/open-completion");
                assert_eq!(probe["params"], serde_json::json!({"action": "support"}));
                respond(
                    &mut server,
                    &probe,
                    serde_json::json!({"supported": true, "version": 1}),
                )
                .await;
                while let Some(Ok(frame)) = server.next().await {
                    assert!(
                        !matches!(frame, Message::Binary(_)),
                        "no unproved session or prompt"
                    );
                }
            } else {
                respond(
                    &mut server,
                    &initialize,
                    serde_json::json!({
                        "agentCapabilities":{"mcpCapabilities":{"http":true}}
                    }),
                )
                .await;
                let new = receive_json(&mut server).await;
                assert_eq!(new["method"], "session/new");
                server.close(None).await.unwrap();
            }
        });
        let mut config = settings(None, false);
        config.access_tool = Some(access_tool_tests::binding(None));
        config.open_completion = Some(crate::hosted::HostedOpenCompletionProtocol::V1);
        let (updates, _receiver) = mpsc::channel(8);
        let result = Bridge::open(
            client,
            "agent-1",
            "/workspace".into(),
            config,
            None,
            updates,
        )
        .await;
        let detail = match result {
            Err(AdapterError::Unavailable(detail)) if !retry => detail,
            Err(AdapterError::Rejected(detail)) if retry => detail,
            _ => panic!(
                "lost identity must settle explicitly without claiming unsupported capability"
            ),
        };
        assert_eq!(
            detail,
            format!(
                "channel tool session setup did not complete (phase: {})",
                if retry {
                    "access_tool_confirmation"
                } else {
                    "session_new"
                }
            )
        );
        serving.await.unwrap();
    }
}

#[tokio::test]
async fn protected_session_and_permission_refusals_keep_safe_phase_without_echoed_descriptors() {
    for failure_phase in ["session_new", "permission_mode"] {
        let (client, mut server) = socket_pair().await;
        send_ready(&mut server, "agent-1").await;
        let serving = tokio::spawn(async move {
            let initialize = receive_json(&mut server).await;
            respond(
                &mut server,
                &initialize,
                serde_json::json!({
                    "agentCapabilities":{"mcpCapabilities":{"http":true}}
                }),
            )
            .await;
            let new = receive_json(&mut server).await;
            let descriptor = new["params"]["mcpServers"].clone();
            let refused = if failure_phase == "permission_mode" {
                respond(
                    &mut server,
                    &new,
                    serde_json::json!({"sessionId":"safe-fixture"}),
                )
                .await;
                let mode = receive_json(&mut server).await;
                assert_eq!(mode["method"], "session/set_mode");
                mode
            } else {
                new
            };
            send_json(
                &mut server,
                serde_json::json!({"jsonrpc":"2.0", "id":refused["id"],
                "error":{"code":-32603,"message":format!("private descriptor: {descriptor}")}}),
            )
            .await;
            while let Some(Ok(frame)) = server.next().await {
                assert!(!matches!(frame, Message::Binary(_)));
            }
        });
        let mut config = settings(Some("read-only"), false);
        config.access_tool = Some(access_tool_tests::binding(None));
        let (updates, _receiver) = mpsc::channel(8);
        let result = Bridge::open(
            client,
            "agent-1",
            "/workspace".into(),
            config,
            None,
            updates,
        )
        .await;
        let Err(AdapterError::Rejected(detail)) = result else {
            panic!("refusal must stay terminal")
        };
        assert_eq!(
            detail,
            format!("channel tool session setup did not complete (phase: {failure_phase})")
        );
        assert!(!detail.contains("cma_at_"));
        assert!(!detail.contains("cma.example"));
        serving.await.unwrap();
    }
}
