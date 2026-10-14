use super::*;

#[tokio::test]
async fn live_settings_wait_for_all_runtime_acknowledgements_without_prompting() {
    for confirms_effort in [true, false] {
        let (client, mut server) = socket_pair().await;
        send_ready(&mut server, "agent-1").await;
        let serving = tokio::spawn(async move {
            let initialize = receive_json(&mut server).await;
            respond(&mut server, &initialize, serde_json::json!({})).await;
            let open = receive_json(&mut server).await;
            respond(
                &mut server,
                &open,
                serde_json::json!({"sessionId":"settings-session"}),
            )
            .await;
            let model = receive_json(&mut server).await;
            assert_eq!(model["method"], "session/set_model");
            assert_eq!(
                model["params"],
                serde_json::json!({"sessionId":"settings-session","modelId":"gpt-6[xhigh]"})
            );
            respond(&mut server, &model, serde_json::json!({})).await;
            let effort = receive_json(&mut server).await;
            assert_eq!(effort["method"], "session/set_config_option");
            assert_eq!(effort["params"]["value"], "xhigh");
            let effort_result = if confirms_effort { "xhigh" } else { "medium" };
            respond(&mut server, &effort, serde_json::json!({"configOptions":[{"id":"reasoning_effort","currentValue":effort_result}]})).await;
            if confirms_effort {
                let permission = receive_json(&mut server).await;
                assert_eq!(permission["method"], "session/set_mode");
                assert_eq!(permission["params"]["modeId"], "agent-full-access");
                respond(&mut server, &permission, serde_json::json!({})).await;
            }
            while let Some(Ok(frame)) = server.next().await {
                assert!(
                    !matches!(frame, Message::Binary(_)),
                    "settings never create a prompt or silently retry"
                );
            }
        });
        let (updates, _receiver) = tokio::sync::mpsc::channel(8);
        let opened = Bridge::open(
            client,
            "agent-1",
            "/workspace".into(),
            settings(None, false),
            None,
            updates,
        )
        .await
        .unwrap();
        let result = opened
            .channel
            .input
            .send(ConversationInput::Configure {
                model: "gpt-6".into(),
                reasoning_effort: crate::ReasoningEffort::XHigh,
                permission: crate::PermissionLevel::Autonomous,
            })
            .await;
        assert_eq!(result.is_ok(), confirms_effort);
        stop(opened).await;
        serving.await.unwrap();
    }
}

#[tokio::test]
async fn rejected_model_has_no_configuration_effect_and_keeps_writer_usable() {
    let (client, mut server) = socket_pair().await;
    send_ready(&mut server, "agent-1").await;
    let serving = tokio::spawn(async move {
        let initialize = receive_json(&mut server).await;
        respond(&mut server, &initialize, serde_json::json!({})).await;
        let open = receive_json(&mut server).await;
        respond(
            &mut server,
            &open,
            serde_json::json!({"sessionId":"settings-session"}),
        )
        .await;
        let model = receive_json(&mut server).await;
        assert_eq!(model["method"], "session/set_model");
        reject(&mut server, &model).await;
        let cancel = receive_json(&mut server).await;
        assert_eq!(cancel["method"], "session/cancel");
        while server.next().await.is_some() {}
    });
    let (updates, _receiver) = tokio::sync::mpsc::channel(8);
    let opened = Bridge::open(
        client,
        "agent-1",
        "/workspace".into(),
        settings(None, false),
        None,
        updates,
    )
    .await
    .unwrap();
    let result = opened
        .channel
        .input
        .send_classified(ConversationInput::Configure {
            model: "unavailable-model".into(),
            reasoning_effort: crate::ReasoningEffort::High,
            permission: crate::PermissionLevel::Autonomous,
        })
        .await;
    assert!(matches!(
        result,
        crate::ConversationSendOutcome::PreWriteFailure(crate::AdapterError::Unsupported(_))
    ));
    opened
        .channel
        .input
        .send(ConversationInput::Cancel)
        .await
        .unwrap();
    stop(opened).await;
    serving.await.unwrap();
}
