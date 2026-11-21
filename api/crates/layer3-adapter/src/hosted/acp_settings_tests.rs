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

async fn acknowledge_instruction_choices(server: &mut ServerSocket) {
    for method in [
        "session/set_model",
        "session/set_config_option",
        "session/set_mode",
    ] {
        let request = receive_json(server).await;
        assert_eq!(request["method"], method);
        let result = if method == "session/set_config_option" {
            serde_json::json!({"configOptions":[{"id":"reasoning_effort","currentValue":"high"}]})
        } else {
            serde_json::json!({})
        };
        respond(server, &request, result).await;
    }
}

#[tokio::test]
async fn legacy_instruction_recovery_loads_exact_thread_once_without_interrupting_or_writing_files()
{
    instruction_recovery(0..10, false).await;
}

#[tokio::test]
async fn instructions_recovery_load_survives_the_ordinary_rpc_deadline() {
    instruction_recovery(0..1, true).await;
}

async fn instruction_recovery(scenarios: std::ops::Range<u8>, slow_load: bool) {
    // Success, another error, changed file, active prompt, native goal,
    // wrong generation, wrong native thread, second failure, missing binding,
    // and native work observed while reading the retained session.
    for scenario in scenarios {
        let (client, mut server) = socket_pair().await;
        send_ready(&mut server, "agent-1").await;
        let tools = vec![serde_json::json!({"name":"cma_access","type":"http",
            "url":"https://cma.invalid/access","headers":[{"name":"Authorization","value":"Bearer fixture"}]})];
        let expected_tools = tools.clone();
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
            respond(
                &mut server,
                &support,
                serde_json::json!({"supported":true,"version":1}),
            )
            .await;
            acknowledge_instruction_choices(&mut server).await;
            let apply = receive_json(&mut server).await;
            assert_eq!(apply["method"], "cma/agent-instructions");
            let error = serde_json::json!({"code":-32603,"message":"Internal error","data":{
                "details":if scenario == 1 {"Another internal failure"} else {"Cannot read properties of undefined (reading 'replace')"}
            }});
            send_json(
                &mut server,
                serde_json::json!({"jsonrpc":"2.0","id":apply["id"],"error":error}),
            )
            .await;
            if matches!(scenario, 0 | 6 | 7 | 9) {
                let list = receive_json(&mut server).await;
                assert_eq!(list["method"], "session/list");
                if scenario == 9 {
                    send_json(&mut server, serde_json::json!({"jsonrpc":"2.0","method":"session/update","params":{
                        "sessionId":"retained-session","update":{"sessionUpdate":"session_info_update","_meta":{"goal":{"status":"active"}}}
                    }})).await;
                }
                respond(&mut server, &list, serde_json::json!({"sessions":[{
                    "sessionId": if scenario == 6 {"other-session"} else {"retained-session"},
                    "cwd":"/home/cma/user-current-directory","additionalDirectories":["/home/cma/extra"]
                }]})).await;
                if !matches!(scenario, 6 | 9) {
                    let load = receive_json(&mut server).await;
                    assert_eq!(load["method"], "session/load");
                    assert_eq!(
                        load["params"],
                        serde_json::json!({"sessionId":"retained-session",
                        "cwd":"/home/cma/user-current-directory","additionalDirectories":["/home/cma/extra"],
                        "mcpServers": expected_tools})
                    );
                    if slow_load {
                        recovery_deadline_tests::elapse_silent_recovery().await;
                    }
                    respond(&mut server, &load, serde_json::json!({})).await;
                    acknowledge_instruction_choices(&mut server).await;
                    let replay = receive_json(&mut server).await;
                    assert_eq!(replay["method"], apply["method"]);
                    assert_eq!(replay["params"], apply["params"]);
                    if scenario == 7 {
                        send_json(
                            &mut server,
                            serde_json::json!({"jsonrpc":"2.0","id":replay["id"],"error":error}),
                        )
                        .await;
                    } else {
                        use sha2::{Digest, Sha256};
                        let hash = format!(
                            "{:x}",
                            Sha256::digest(
                                apply["params"]["systemPrompt"].as_str().unwrap().as_bytes()
                            )
                        );
                        respond(&mut server, &replay, serde_json::json!({"applied":true,"sessionId":"retained-session","sha256":hash})).await;
                    }
                }
            }
            while let Some(Ok(frame)) = server.next().await {
                assert!(
                    !matches!(frame, Message::Binary(_)),
                    "recovery must not close, cancel, prompt, or retry again"
                );
            }
        });
        let (binding, files) = crate::hosted::managed_context::writer_tests::binding_with_tools(
            if scenario == 5 { 5 } else { 0 },
            tools,
        );
        let mut config = settings(None, false);
        config.context_refresh = (scenario != 8).then_some(binding);
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
        if scenario == 3 {
            opened
                .bridge
                .owned_prompt_waiters
                .store(1, std::sync::atomic::Ordering::SeqCst);
        }
        if scenario == 4 {
            opened
                .bridge
                .observe_context_goal(
                    "retained-session",
                    &serde_json::json!({
                        "sessionUpdate":"session_info_update","_meta":{"goal":{"status":"active"}}
                    }),
                )
                .await;
        }
        let result = opened
            .channel
            .input
            .send_classified(ConversationInput::Configure {
                model: "gpt-6".into(),
                reasoning_effort: crate::ReasoningEffort::High,
                permission: crate::PermissionLevel::Autonomous,
                system_prompt: Some(
                    if scenario == 2 {
                        "Changed after original failure"
                    } else {
                        "Old managed\n\nOwner exact text  \n"
                    }
                    .into(),
                ),
            })
            .await;
        assert_eq!(
            matches!(result, crate::ConversationSendOutcome::WriteConfirmed),
            scenario == 0,
            "scenario {scenario}"
        );
        if scenario != 0 {
            assert!(
                matches!(result, crate::ConversationSendOutcome::Ambiguous(_)),
                "already-persisted update must remain recoverable: scenario {scenario}"
            );
        }
        assert_eq!(files.writes.load(std::sync::atomic::Ordering::SeqCst), 0);
        stop(opened).await;
        serving.await.unwrap();
    }
}
