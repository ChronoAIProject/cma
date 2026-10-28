use super::*;
use crate::hosted::{HostedProviderToolBinding, HostedToolOpenReceipt};

pub(super) fn binding(retained: Option<(&str, &str)>, observed: bool) -> HostedProviderToolBinding {
    let request = crate::AgentOpenRequest::new(
        "open-tools",
        crate::RuntimeOptions {
            slot: crate::AgentSlot::new("agent-1"),
            ..Default::default()
        },
        None,
    );
    let proxy = crate::ToolProxyRequest {
        sandbox_id: "sandbox-test".into(),
        generation: 1,
        endpoint: "https://gateway.example.test/mcp".into(),
        credential_generation: 1,
    };
    let mut binding = HostedProviderToolBinding::new(
        "a".repeat(64),
        crate::RuntimeHandle("runtime-1".into()),
        request,
        1,
        proxy.clone(),
        retained.map(|(boot, conversation)| HostedToolOpenReceipt {
            boot_id: boot.repeat(32),
            conversation: conversation.into(),
        }),
    )
    .unwrap();
    if observed {
        binding
            .observe(crate::ToolProxyStatus {
                request: proxy,
                boot_id: "b".repeat(32),
            })
            .unwrap();
    }
    binding
}

#[tokio::test]
async fn provider_tools_use_only_the_verified_local_proxy_for_new_and_loaded_conversations() {
    for resume in [None, Some("provider-conversation")] {
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
            assert_eq!(servers[0]["name"], "cmaeg");
            assert_eq!(servers[0]["url"], crate::LOCAL_TOOL_PROXY_URL);
            assert!(servers[0]["headers"].as_array().is_none_or(Vec::is_empty));
            assert!(!open.to_string().contains("gateway.example.test"));
            respond(
                &mut server,
                &open,
                serde_json::json!({"sessionId":"provider-conversation"}),
            )
            .await;
            while let Some(Ok(frame)) = server.next().await {
                assert!(
                    !matches!(frame, Message::Binary(_)),
                    "tool setup must not alter profile effort or send a prompt"
                );
            }
        });
        let (updates, _receiver) = tokio::sync::mpsc::channel(8);
        let mut configured = settings(None, false);
        configured.provider_tool = Some(binding(None, true));
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
        stop(opened).await;
        serving.await.unwrap();
    }
}

#[tokio::test]
async fn rejoin_requires_an_exact_successful_tool_open_in_the_same_runtime_process() {
    for (retained, accepted) in [
        (None, false),
        (Some(("a", "provider-conversation")), false),
        (Some(("b", "other-conversation")), false),
        (Some(("b", "provider-conversation")), true),
    ] {
        let (client, mut server) = socket_pair().await;
        send_ready(&mut server, "agent-1").await;
        let serving = tokio::spawn(async move {
            let initialize = receive_json(&mut server).await;
            reject(&mut server, &initialize).await;
            while let Some(Ok(frame)) = server.next().await {
                assert!(
                    !matches!(frame, Message::Binary(_)),
                    "rejoin never replays setup or input"
                );
            }
        });
        let (updates, _receiver) = tokio::sync::mpsc::channel(8);
        let mut configured = settings(None, false);
        configured.provider_tool = Some(binding(retained, true));
        let opened = Bridge::open(
            client,
            "agent-1",
            "/workspace".into(),
            configured,
            Some("provider-conversation".into()),
            updates,
        )
        .await;
        if accepted {
            stop(opened.unwrap()).await;
        } else {
            assert!(matches!(opened, Err(crate::AdapterError::Unsupported(_))));
        }
        serving.await.unwrap();
    }
}

#[tokio::test]
async fn native_http_capability_and_runtime_observation_are_both_required() {
    for (http, observed) in [(false, true), (true, false)] {
        let (client, mut server) = socket_pair().await;
        send_ready(&mut server, "agent-1").await;
        let serving = tokio::spawn(async move {
            let initialize = receive_json(&mut server).await;
            respond(
                &mut server,
                &initialize,
                serde_json::json!({
                    "agentCapabilities":{"mcpCapabilities":{"http":http}}
                }),
            )
            .await;
            while let Some(Ok(frame)) = server.next().await {
                assert!(!matches!(frame, Message::Binary(_)));
            }
        });
        let (updates, _receiver) = tokio::sync::mpsc::channel(8);
        let mut configured = settings(None, false);
        configured.provider_tool = Some(binding(None, observed));
        let opened = Bridge::open(
            client,
            "agent-1",
            "/workspace".into(),
            configured,
            None,
            updates,
        )
        .await;
        assert!(matches!(opened, Err(crate::AdapterError::Unsupported(_))));
        serving.await.unwrap();
    }
}
