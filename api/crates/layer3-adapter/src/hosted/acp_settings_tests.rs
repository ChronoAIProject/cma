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
                system_prompt: None,
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
            system_prompt: None,
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

#[tokio::test]
async fn retained_runtime_without_instruction_extension_refuses_before_any_setting_write() {
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
        let support = receive_json(&mut server).await;
        assert_eq!(support["method"], "cma/agent-instructions");
        assert_eq!(support["params"]["action"], "support");
        reject(&mut server, &support).await;
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
    assert!(!opened.channel.input.system_prompt_supported().await);
    let result = opened
        .channel
        .input
        .send_classified(ConversationInput::Configure {
            model: "gpt-6".into(),
            reasoning_effort: crate::ReasoningEffort::High,
            permission: crate::PermissionLevel::Autonomous,
            system_prompt: Some("new instructions".into()),
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

#[tokio::test]
async fn instruction_update_requires_exact_native_thread_and_content_acknowledgement() {
    for correct_hash in [true, false] {
        let (client, mut server) = socket_pair().await;
        send_ready(&mut server, "agent-1").await;
        let serving = tokio::spawn(async move {
            let initialize = receive_json(&mut server).await;
            respond(&mut server, &initialize, serde_json::json!({})).await;
            let open = receive_json(&mut server).await;
            respond(
                &mut server,
                &open,
                serde_json::json!({"sessionId":"instructions-session"}),
            )
            .await;
            let support = receive_json(&mut server).await;
            assert_eq!(support["params"]["action"], "support");
            respond(
                &mut server,
                &support,
                serde_json::json!({"supported":true,"version":1}),
            )
            .await;
            let model = receive_json(&mut server).await;
            assert_eq!(model["method"], "session/set_model");
            respond(&mut server, &model, serde_json::json!({})).await;
            let effort = receive_json(&mut server).await;
            respond(&mut server,&effort,serde_json::json!({"configOptions":[{"id":"reasoning_effort","currentValue":"high"}]})).await;
            let mode = receive_json(&mut server).await;
            respond(&mut server, &mode, serde_json::json!({})).await;
            let apply = receive_json(&mut server).await;
            assert_eq!(apply["method"], "cma/agent-instructions");
            assert_eq!(apply["params"]["systemPrompt"], "new instructions");
            use sha2::{Digest, Sha256};
            let hash = if correct_hash {
                format!("{:x}", Sha256::digest(b"new instructions"))
            } else {
                "wrong-content".into()
            };
            respond(&mut server,&apply,serde_json::json!({"applied":true,"sessionId":"instructions-session","sha256":hash})).await;
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
                model: "gpt-6".into(),
                reasoning_effort: crate::ReasoningEffort::High,
                permission: crate::PermissionLevel::Autonomous,
                system_prompt: Some("new instructions".into()),
            })
            .await;
        assert_eq!(
            matches!(result, crate::ConversationSendOutcome::WriteConfirmed),
            correct_hash
        );
        if !correct_hash {
            assert!(matches!(
                result,
                crate::ConversationSendOutcome::Ambiguous(_)
            ));
        }
        stop(opened).await;
        serving.await.unwrap();
    }
}
