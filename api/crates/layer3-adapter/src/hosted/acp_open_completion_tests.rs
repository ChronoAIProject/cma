use super::*;
use crate::hosted::HostedOpenCompletionProtocol;

fn configured(family: &str) -> HostedAgentOpenSettings {
    let mut config = settings(Some("read-only"), false);
    config.open_completion = Some(HostedOpenCompletionProtocol::V1);
    match family {
        "access" => config.access_tool = Some(access_tool_tests::binding(None)),
        "provider" => config.provider_tool = Some(provider_tool_tests::binding(None, true)),
        "channel" => config.channel_tool = Some(channel_tool_tests::binding(None)),
        _ => unreachable!(),
    }
    config
}

fn digest(config: &HostedAgentOpenSettings) -> String {
    let server = config
        .channel_tool
        .as_ref()
        .map(|b| b.acp_server())
        .or_else(|| config.provider_tool.as_ref().map(|b| b.acp_server()))
        .or_else(|| config.access_tool.as_ref().map(|b| b.acp_server()))
        .unwrap();
    super::super::super::acp_open_completion::binding_digest("/workspace", &[server]).unwrap()
}

async fn completion(server: &mut ServerSocket, expected: &str, mutation: &str) {
    let request = receive_json(server).await;
    assert_eq!(request["method"], "cma/open-completion");
    let params = &request["params"];
    assert_eq!(params["action"], "read");
    assert_eq!(params["sessionId"], "held-conversation");
    assert_eq!(params["bindingDigest"], expected);
    assert!(uuid::Uuid::parse_str(params["challenge"].as_str().unwrap()).is_ok());
    let mut result = serde_json::json!({
        "complete":true,"version":1,"sessionId":params["sessionId"],
        "bindingDigest":params["bindingDigest"],"challenge":params["challenge"],
        "processNonce":"cdd9d1db-5781-4ba0-9e27-df60e4f1cab2",
        "openGeneration":"0b6ef5af-53df-4beb-a34d-967b8a2b32bc","sessionGeneration":0,
    });
    match mutation {
        "complete" => result["complete"] = serde_json::json!(false),
        "version" => result["version"] = serde_json::json!(2),
        "sessionId" => result["sessionId"] = serde_json::json!("another-conversation"),
        "bindingDigest" => result["bindingDigest"] = serde_json::json!("v1:changed"),
        "challenge" => result["challenge"] = serde_json::json!("an_earlier_challenge"),
        "processNonce" => {
            result["processNonce"] = serde_json::json!("ba8a8a50-d1a2-4c98-9f6f-de15f778601d")
        }
        "openGeneration" => {
            result["openGeneration"] = serde_json::json!("a6686cad-992b-4901-9338-c77595cf2b69")
        }
        "sessionGeneration" => result["sessionGeneration"] = serde_json::json!(1),
        "invalidUuid" => {
            result["processNonce"] = serde_json::json!("00000000-0000-0000-0000-000000000000")
        }
        "unsupported" => {
            send_json(
                server,
                serde_json::json!({"jsonrpc":"2.0","id":request["id"],
                "error":{"code":-32601,"message":"Method not found"}}),
            )
            .await;
            return;
        }
        "" => {}
        _ => unreachable!(),
    }
    respond(server, &request, result).await;
}

async fn settings_only(server: &mut ServerSocket, family: &str) {
    let mode = receive_json(server).await;
    assert_eq!(mode["method"], "session/set_mode");
    assert_eq!(mode["params"]["sessionId"], "held-conversation");
    respond(server, &mode, serde_json::json!({})).await;
    if family == "channel" {
        let effort = receive_json(server).await;
        assert_eq!(effort["method"], "session/set_config_option");
        respond(
            server,
            &effort,
            serde_json::json!({"configOptions":[{
                "id":"reasoning_effort","currentValue":"low","options":[{"value":"low"}]
            }]}),
        )
        .await;
    }
}

async fn exercise(family: &'static str, first: &'static str, second: &'static str) {
    let config = configured(family);
    let expected = digest(&config);
    let (client, mut server) = socket_pair().await;
    send_ready(&mut server, "agent-1").await;
    let serving = tokio::spawn(async move {
        let initialize = receive_json(&mut server).await;
        reject(&mut server, &initialize).await;
        completion(&mut server, &expected, first).await;
        if first.is_empty() {
            settings_only(&mut server, family).await;
            completion(&mut server, &expected, second).await;
        }
        // In particular: no session/load, session/close or prompt can be sent
        // to obtain substitute evidence after a refused or successful read.
        while let Some(Ok(frame)) = server.next().await {
            assert!(!matches!(frame, Message::Binary(_)));
        }
    });
    let (updates, _receiver) = tokio::sync::mpsc::channel(8);
    let opened = Bridge::open(
        client,
        "agent-1",
        "/workspace".into(),
        config,
        Some("held-conversation".into()),
        updates,
    )
    .await;
    if first.is_empty() && second.is_empty() {
        assert_eq!(
            opened.as_ref().unwrap().channel.conversation_id.as_deref(),
            Some("held-conversation")
        );
        stop(opened.unwrap()).await;
    } else {
        assert!(matches!(opened, Err(crate::AdapterError::Unavailable(_))));
    }
    serving.await.unwrap();
}

#[tokio::test]
async fn exact_native_completion_recovers_missing_confirmation_for_each_tool_family() {
    for family in ["access", "provider", "channel"] {
        exercise(family, "", "").await;
    }
}

#[tokio::test]
async fn unavailable_or_uncorrelated_native_completion_cannot_bypass_confirmation() {
    for field in [
        "complete",
        "version",
        "sessionId",
        "bindingDigest",
        "challenge",
        "invalidUuid",
        "unsupported",
    ] {
        exercise("access", field, "").await;
    }
}

#[tokio::test]
async fn changed_native_incarnation_before_handshake_publication_is_refused() {
    for field in [
        "processNonce",
        "openGeneration",
        "sessionGeneration",
        "complete",
    ] {
        exercise("provider", "", field).await;
    }
}
