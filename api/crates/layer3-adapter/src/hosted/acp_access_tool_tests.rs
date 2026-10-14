use super::*;
use crate::hosted::HostedAccessToolBinding;

fn binding(retained: Option<&str>) -> HostedAccessToolBinding {
    let request = crate::AgentOpenRequest::new(
        "open-access-fixture",
        crate::RuntimeOptions {
            slot: crate::AgentSlot::new("agent-1"),
            ..Default::default()
        },
        None,
    );
    HostedAccessToolBinding::new(
        "https://cma.example",
        crate::RuntimeCredential::new(format!("cma_at_{}", "a".repeat(64))),
        crate::RuntimeHandle("runtime-1".into()),
        &request,
        1,
        retained.map(str::to_owned),
    )
    .unwrap()
}

#[tokio::test]
async fn access_tool_is_negotiated_for_new_and_resumed_conversations() {
    for resume in [None, Some("access-conversation")] {
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
            assert_eq!(servers[0]["name"], "cma_access");
            assert_eq!(servers[0]["type"], "http");
            assert_eq!(servers[0]["url"], "https://cma.example/api/v1/agent-tools");
            assert_eq!(
                servers[0]["headers"][0]["value"],
                format!("Bearer cma_at_{}", "a".repeat(64))
            );
            respond(
                &mut server,
                &open,
                serde_json::json!({"sessionId":"access-conversation"}),
            )
            .await;
            // An access tool must not change Agent effort or send an extra turn.
            while let Some(Ok(frame)) = server.next().await {
                assert!(!matches!(frame, Message::Binary(_)));
            }
        });
        let (updates, _receiver) = tokio::sync::mpsc::channel(8);
        let mut configured = settings(None, false);
        configured.access_tool = Some(binding(None));
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
async fn existing_conversation_requires_exact_retained_access_tool_receipt() {
    for retained in [
        None,
        Some("different-conversation"),
        Some("access-conversation"),
    ] {
        let (client, mut server) = socket_pair().await;
        send_ready(&mut server, "agent-1").await;
        let serving = tokio::spawn(async move {
            let initialize = receive_json(&mut server).await;
            reject(&mut server, &initialize).await;
            while let Some(Ok(frame)) = server.next().await {
                assert!(!matches!(frame, Message::Binary(_)));
            }
        });
        let (updates, _receiver) = tokio::sync::mpsc::channel(8);
        let mut configured = settings(None, false);
        configured.access_tool = Some(binding(retained));
        let result = Bridge::open(
            client,
            "agent-1",
            "/workspace".into(),
            configured,
            Some("access-conversation".into()),
            updates,
        )
        .await;
        if retained == Some("access-conversation") {
            stop(result.unwrap()).await;
        } else {
            assert!(matches!(result, Err(crate::AdapterError::Unsupported(_))));
        }
        serving.await.unwrap();
    }
}

#[tokio::test]
async fn missing_http_capability_refuses_access_tool_before_open() {
    let (client, mut server) = socket_pair().await;
    send_ready(&mut server, "agent-1").await;
    let serving = tokio::spawn(async move {
        let initialize = receive_json(&mut server).await;
        respond(&mut server, &initialize, serde_json::json!({})).await;
        while let Some(Ok(frame)) = server.next().await {
            assert!(!matches!(frame, Message::Binary(_)));
        }
    });
    let (updates, _receiver) = tokio::sync::mpsc::channel(8);
    let mut configured = settings(None, false);
    configured.access_tool = Some(binding(None));
    let result = Bridge::open(
        client,
        "agent-1",
        "/workspace".into(),
        configured,
        None,
        updates,
    )
    .await;
    assert!(matches!(result, Err(crate::AdapterError::Unsupported(_))));
    serving.await.unwrap();
}

#[tokio::test]
async fn access_setup_error_does_not_disclose_echoed_capability() {
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
        server.send(Message::Binary(serde_json::to_vec(&serde_json::json!({
            "jsonrpc":"2.0", "id":open["id"],
            "error":{"code":-32603,"message":format!("echoed {}", open["params"]["mcpServers"])}
        })).unwrap())).await.unwrap();
        let _ = server.next().await;
    });
    let (updates, _receiver) = tokio::sync::mpsc::channel(8);
    let mut configured = settings(None, false);
    configured.access_tool = Some(binding(None));
    let result = Bridge::open(
        client,
        "agent-1",
        "/workspace".into(),
        configured,
        None,
        updates,
    )
    .await;
    let Err(error) = result else {
        panic!("setup must fail");
    };
    assert!(!error.to_string().contains("cma_at_"));
    assert!(!error.to_string().contains("cma.example"));
    serving.await.unwrap();
}
