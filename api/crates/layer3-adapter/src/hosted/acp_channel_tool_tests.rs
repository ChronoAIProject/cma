use super::*;
use crate::hosted::HostedChannelToolBinding;

pub(super) fn binding(retained: Option<&str>) -> HostedChannelToolBinding {
    let request = crate::AgentOpenRequest::new(
        "open-tool-fixture",
        crate::RuntimeOptions {
            slot: crate::AgentSlot::new("agent-1"),
            ..Default::default()
        },
        None,
    );
    HostedChannelToolBinding::new(
        "https://cma.example",
        crate::RuntimeCredential::new(
            "cma_ct_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        ),
        crate::RuntimeHandle("runtime-1".into()),
        &request,
        1,
        retained.map(str::to_owned),
    )
    .unwrap()
}

async fn confirm_low_effort(server: &mut ServerSocket) {
    let configure = receive_json(server).await;
    assert_eq!(configure["method"], "session/set_config_option");
    assert_eq!(
        configure["params"],
        serde_json::json!({
            "sessionId":"channel-conversation", "configId":"reasoning_effort", "value":"low"
        })
    );
    respond(
        server,
        &configure,
        serde_json::json!({"configOptions":[{
            "id":"reasoning_effort", "type":"select", "currentValue":"low",
            "options":[{"value":"low", "name":"Low"},{"value":"xhigh", "name":"Extra high"}]
        }]}),
    )
    .await;
}

#[path = "acp_channel_effort_tests.rs"]
mod effort_tests;

#[tokio::test]
async fn fresh_and_resumed_sessions_receive_only_explicit_negotiated_channel_tool() {
    for resume in [None, Some("channel-conversation")] {
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
            let open = receive_json(&mut server).await;
            assert_eq!(
                open["method"],
                if resume.is_some() {
                    "session/load"
                } else {
                    "session/new"
                }
            );
            let servers = open["params"]["mcpServers"].as_array().unwrap();
            assert_eq!(servers.len(), 1);
            assert_eq!(servers[0]["name"], "cma_channel");
            assert_eq!(servers[0]["type"], "http");
            assert_eq!(
                servers[0]["url"],
                "https://cma.example/api/v1/channel-tools"
            );
            assert_eq!(
                servers[0]["headers"],
                serde_json::json!([{
                    "name":"Authorization", "value":"Bearer cma_ct_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                }])
            );
            // Tool setup travels only in ACP session settings. It is not a
            // prompt, runtime home, provider credential, or model-selected URL.
            assert!(open["params"].get("prompt").is_none());
            respond(
                &mut server,
                &open,
                serde_json::json!({"sessionId":"channel-conversation"}),
            )
            .await;
            confirm_low_effort(&mut server).await;
            // Hold the link until the client has observed the completed handshake.
            let _ = server.next().await;
        });
        let (updates, _receiver) = tokio::sync::mpsc::channel(8);
        let mut configured = settings(None, false);
        configured.channel_tool = Some(binding(None));
        let opened = Bridge::open(
            client,
            "agent-1",
            "/workspace".into(),
            configured,
            resume.map(str::to_owned),
            updates,
        )
        .await
        .unwrap();
        assert_eq!(
            opened.channel.conversation_id.as_deref(),
            Some("channel-conversation")
        );
        stop(opened).await;
        serving.abort();
    }
}

#[tokio::test]
async fn unsupported_or_unproved_existing_runtime_cannot_silently_omit_required_tool() {
    for existing in [false, true] {
        let (client, mut server) = socket_pair().await;
        send_ready(&mut server, "agent-1").await;
        let serving = tokio::spawn(async move {
            let initialize = receive_json(&mut server).await;
            if existing {
                reject(&mut server, &initialize).await;
            } else {
                respond(&mut server, &initialize, serde_json::json!({})).await;
            }
            while let Some(Ok(frame)) = server.next().await {
                assert!(
                    !matches!(frame, Message::Binary(_)),
                    "no session open or prompt after tool refusal"
                );
            }
        });
        let (updates, _receiver) = tokio::sync::mpsc::channel(8);
        let mut configured = settings(None, false);
        configured.channel_tool = Some(binding(None));
        let result = Bridge::open(
            client,
            "agent-1",
            "/workspace".into(),
            configured,
            Some("channel-conversation".into()),
            updates,
        )
        .await;
        if existing {
            assert!(matches!(result, Err(crate::AdapterError::Rejected(_))));
        } else {
            assert!(matches!(result, Err(crate::AdapterError::Unsupported(_))));
        }
        serving.await.unwrap();
    }
}

#[tokio::test]
async fn retained_exact_tool_receipt_rejoins_with_low_effort_without_replaying_input() {
    let (client, mut server) = socket_pair().await;
    send_ready(&mut server, "agent-1").await;
    let serving = tokio::spawn(async move {
        let initialize = receive_json(&mut server).await;
        reject(&mut server, &initialize).await;
        confirm_low_effort(&mut server).await;
        // Rejoining a proven tool session must not load/reset it or send input.
        while let Some(Ok(frame)) = server.next().await {
            assert!(!matches!(frame, Message::Binary(_)));
        }
    });
    let (updates, _receiver) = tokio::sync::mpsc::channel(8);
    let mut configured = settings(None, false);
    configured.channel_tool = Some(binding(Some("channel-conversation")));
    let opened = Bridge::open(
        client,
        "agent-1",
        "/workspace".into(),
        configured,
        Some("channel-conversation".into()),
        updates,
    )
    .await
    .unwrap();
    stop(opened).await;
    serving.await.unwrap();
}

#[test]
fn tool_binding_is_exact_redacted_and_not_carried_by_retained_open_wire() {
    let bound = binding(None);
    assert!(!format!("{bound:?}").contains("cma_ct_"));
    assert!(!format!("{bound:?}").contains("https://"));
    let mut request = crate::AgentOpenRequest::new(
        "open-tool-fixture",
        crate::RuntimeOptions {
            slot: crate::AgentSlot::new("agent-1"),
            ..Default::default()
        },
        None,
    );
    assert!(bound
        .require_request(&crate::RuntimeHandle("runtime-1".into()), &request, 1)
        .is_ok());
    assert!(bound
        .require_request(&crate::RuntimeHandle("runtime-2".into()), &request, 1)
        .is_err());
    assert!(bound
        .require_request(&crate::RuntimeHandle("runtime-1".into()), &request, 2)
        .is_err());
    let wire = serde_json::to_string(&request).unwrap();
    assert!(!wire.contains("channel_tool") && !wire.contains("cma_ct_"));
    request.options.slot = crate::AgentSlot::new("other-agent");
    assert!(bound
        .require_request(&crate::RuntimeHandle("runtime-1".into()), &request, 1)
        .is_err());
    request.options.slot = crate::AgentSlot::new("agent-1");
    assert!(request.has_valid_sha256());
    for url in [
        "http://cma.example",
        "https://user:secret@cma.example",
        "https://cma.example/?token=a",
        "https://cma.example/other",
    ] {
        assert!(HostedChannelToolBinding::new(
            url,
            crate::RuntimeCredential::new(
                "cma_ct_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
            ),
            crate::RuntimeHandle("runtime-1".into()),
            &request,
            1,
            None
        )
        .is_err());
    }
}

#[test]
fn tool_binding_rejects_untyped_or_provider_credentials() {
    let request = crate::AgentOpenRequest::new("open", Default::default(), None);
    for credential in [
        "nyxid_ag_fixture_only_not_a_real_key_1234567890",
        "fixture-only-opaque-credential-0000000000000000",
        "cma_ct_short",
        "cma_ct_0123456789ABCDEF0123456789abcdef0123456789abcdef0123456789abcdef",
    ] {
        assert!(HostedChannelToolBinding::new(
            "https://cma.example",
            crate::RuntimeCredential::new(credential),
            crate::RuntimeHandle("runtime-1".into()),
            &request,
            1,
            None
        )
        .is_err());
    }
}

#[tokio::test]
async fn channel_setup_error_echoes_never_expose_bearer_in_worker_failure() {
    for (resume, mode_failure) in [
        (None, false),
        (Some("channel-conversation"), false),
        (None, true),
    ] {
        let (client, mut server) = socket_pair().await;
        send_ready(&mut server, "agent-1").await;
        let serving = tokio::spawn(async move {
            let initialize = receive_json(&mut server).await;
            respond(
                &mut server,
                &initialize,
                serde_json::json!({"agentCapabilities":{"mcpCapabilities":{"http":true}}}),
            )
            .await;
            let open = receive_json(&mut server).await;
            let descriptor = open["params"]["mcpServers"].clone();
            let rejected = if mode_failure {
                respond(
                    &mut server,
                    &open,
                    serde_json::json!({"sessionId":"channel-conversation"}),
                )
                .await;
                let mode = receive_json(&mut server).await;
                assert_eq!(mode["method"], "session/set_mode");
                mode
            } else {
                open
            };
            send_json(
                &mut server,
                serde_json::json!({"jsonrpc":"2.0","id":rejected["id"],"error":{
                    "code":-32000,"message":"MCP setup refused","data":{"mcpServers":descriptor}
                }}),
            )
            .await;
            let _ = server.next().await;
        });
        let (updates, _receiver) = tokio::sync::mpsc::channel(8);
        let mut configured = settings(mode_failure.then_some("guarded"), false);
        configured.channel_tool = Some(binding(None));
        let error = match Bridge::open(
            client,
            "agent-1",
            "/workspace".into(),
            configured,
            resume.map(str::to_owned),
            updates,
        )
        .await
        {
            Ok(opened) => {
                stop(opened).await;
                panic!("echoed setup refusal must fail");
            }
            Err(error) => error,
        };
        assert!(matches!(error, crate::AdapterError::Rejected(_)));
        let rendered = format!("{error} {error:?}");
        assert!(
            !rendered.contains("cma_ct_")
                && !rendered.contains("Authorization")
                && !rendered.contains("mcpServers")
        );
        serving.await.unwrap();
    }
}
