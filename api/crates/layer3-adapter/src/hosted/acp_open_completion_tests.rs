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

/// Serve an interrupted first open: the bridge is already initialized and the
/// caller holds no conversation. Returns the frames sent after `current`.
async fn adoption(server: &mut ServerSocket, expected: &str, support: &str, current: &str) {
    let probe = receive_json(server).await;
    assert_eq!(probe["method"], "cma/open-completion");
    assert_eq!(probe["params"], serde_json::json!({"action": "support"}));
    match support {
        "unsupported" => {
            send_json(
                server,
                serde_json::json!({"jsonrpc":"2.0","id":probe["id"],
                "error":{"code":-32601,"message":"Method not found"}}),
            )
            .await;
            return;
        }
        "legacy" => {
            respond(
                server,
                &probe,
                serde_json::json!({"supported":true,"version":1}),
            )
            .await;
            return;
        }
        _ => {
            respond(
                server,
                &probe,
                serde_json::json!({"supported":true,"version":1,"current":true}),
            )
            .await
        }
    }
    let request = receive_json(server).await;
    assert_eq!(request["method"], "cma/open-completion");
    assert_eq!(request["params"]["action"], "current");
    let challenge = request["params"]["challenge"].clone();
    assert!(uuid::Uuid::parse_str(challenge.as_str().unwrap()).is_ok());
    let held = serde_json::json!({"sessionId":"held-conversation","bindingDigest":expected});
    let mut result = serde_json::json!({
        "version":1,"challenge":challenge,
        "processNonce":"cdd9d1db-5781-4ba0-9e27-df60e4f1cab2","opening":false,
        "sessions":[held.clone()],
    });
    match current {
        "" => {}
        "none" => result["sessions"] = serde_json::json!([]),
        "several" => {
            result["sessions"] = serde_json::json!([held,
                {"sessionId":"another-conversation","bindingDigest":expected}])
        }
        "binding" => {
            result["sessions"][0]["bindingDigest"] =
                serde_json::json!(format!("v1:{}", "0".repeat(64)))
        }
        "opening" => result["opening"] = serde_json::json!(true),
        "challenge" => result["challenge"] = serde_json::json!("an-earlier-challenge-value"),
        "version" => result["version"] = serde_json::json!(2),
        _ => unreachable!(),
    }
    respond(server, &request, result).await;
}

async fn interrupted(
    family: &'static str,
    support: &'static str,
    current: &'static str,
) -> (crate::Result<OpenedBridge>, tokio::task::JoinHandle<()>) {
    let config = configured(family);
    let expected = digest(&config);
    let (client, mut server) = socket_pair().await;
    send_ready(&mut server, "agent-1").await;
    let adopted = support.is_empty() && current.is_empty();
    let serving = tokio::spawn(async move {
        let initialize = receive_json(&mut server).await;
        reject(&mut server, &initialize).await;
        adoption(&mut server, &expected, support, current).await;
        if adopted {
            // The adopted open is verified through its exact receipt, then rejoined.
            completion(&mut server, &expected, "").await;
            settings_only(&mut server, family).await;
            completion(&mut server, &expected, "").await;
        }
        // Never a second session/new, session/load or prompt to obtain evidence.
        while let Some(Ok(frame)) = server.next().await {
            if let Message::Text(text) = &frame {
                let frame: serde_json::Value = serde_json::from_str(text).unwrap();
                assert!(
                    !frame["method"]
                        .as_str()
                        .is_some_and(|m| m.starts_with("session/")),
                    "unexpected {frame}"
                );
            }
            assert!(!matches!(frame, Message::Binary(_)));
        }
    });
    let (updates, _receiver) = tokio::sync::mpsc::channel(8);
    let opened = Bridge::open(
        client,
        "agent-1",
        "/workspace".into(),
        configured(family),
        None,
        updates,
    )
    .await;
    (opened, serving)
}

async fn refused(support: &'static str, current: &'static str) -> crate::AdapterError {
    let (opened, serving) = interrupted("access", support, current).await;
    serving.await.unwrap();
    opened
        .err()
        .expect("an unproven interrupted open must not be adopted")
}

#[tokio::test]
async fn interrupted_first_open_adopts_the_single_proven_completed_open() {
    for family in ["access", "provider", "channel"] {
        let (opened, serving) = interrupted(family, "", "").await;
        let opened = opened.unwrap();
        assert_eq!(
            opened.channel.conversation_id.as_deref(),
            Some("held-conversation")
        );
        stop(opened).await;
        serving.await.unwrap();
    }
}

#[tokio::test]
async fn interrupted_first_open_without_proof_keeps_refusing() {
    for (support, current) in [
        ("unsupported", ""),
        ("legacy", ""),
        ("", "none"),
        ("", "several"),
        ("", "binding"),
    ] {
        assert!(
            matches!(
                refused(support, current).await,
                crate::AdapterError::Rejected(_)
            ),
            "{support}/{current}"
        );
    }
    for current in ["challenge", "version"] {
        assert!(matches!(
            refused("", current).await,
            crate::AdapterError::Unavailable(_)
        ));
    }
}

#[tokio::test]
async fn an_open_still_in_flight_is_retried_never_duplicated() {
    let (opened, serving) = interrupted("provider", "", "opening").await;
    serving.await.unwrap();
    assert!(matches!(opened, Err(crate::AdapterError::Unavailable(_))));
}

#[derive(Default)]
struct OneShot(std::sync::atomic::AtomicUsize);

#[async_trait::async_trait]
impl crate::hosted::HostedOpenReplyLoss for OneShot {
    async fn claim(&self) -> bool {
        self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed) == 0
    }
}

async fn fresh_with_reply_loss(
    claim: Option<&dyn crate::hosted::HostedOpenReplyLoss>,
    proof: &'static str,
    drop_reply: bool,
) {
    let config = configured("access");
    let expected = digest(&config);
    let (client, mut server) = socket_pair().await;
    send_ready(&mut server, "agent-1").await;
    let should_probe = claim.is_some();
    let serving = tokio::spawn(async move {
        let initialize = receive_json(&mut server).await;
        assert_eq!(initialize["method"], "initialize");
        respond(
            &mut server,
            &initialize,
            serde_json::json!({"agentCapabilities":{"mcpCapabilities":{"http":true}}}),
        )
        .await;
        let open = receive_json(&mut server).await;
        assert_eq!(open["method"], "session/new");
        respond(
            &mut server,
            &open,
            serde_json::json!({"sessionId":"held-conversation"}),
        )
        .await;
        if should_probe {
            completion(&mut server, &expected, proof).await;
        }
        if !drop_reply {
            settings_only(&mut server, "access").await;
        }
        while let Some(Ok(frame)) = server.next().await {
            assert!(!matches!(frame, Message::Binary(_)), "unexpected {frame:?}");
        }
    });
    let (updates, _receiver) = mpsc::channel(8);
    let opened = Bridge::open_with_relay_policy(
        client,
        "agent-1",
        "/workspace".into(),
        (config, None, (false, claim)),
        None,
        None,
        updates,
    )
    .await;
    if drop_reply {
        assert!(matches!(opened, Err(crate::AdapterError::Unavailable(_))));
    } else {
        let opened = opened.unwrap();
        assert_eq!(
            opened.channel.conversation_id.as_deref(),
            Some("held-conversation")
        );
        stop(opened).await;
    }
    serving.await.unwrap();
}

#[tokio::test]
async fn open_reply_loss_requires_proof_and_preserves_adoption_and_other_opens() {
    let claim = OneShot::default();
    fresh_with_reply_loss(None, "", false).await;
    fresh_with_reply_loss(Some(&claim), "complete", false).await;
    assert_eq!(claim.0.load(std::sync::atomic::Ordering::Relaxed), 0);
    fresh_with_reply_loss(Some(&claim), "", true).await;
    let (recovered, serving) = interrupted("access", "", "").await;
    let recovered = recovered.unwrap();
    assert_eq!(
        recovered.channel.conversation_id.as_deref(),
        Some("held-conversation")
    );
    stop(recovered).await;
    serving.await.unwrap();
    fresh_with_reply_loss(Some(&claim), "", false).await;
    assert_eq!(claim.0.load(std::sync::atomic::Ordering::Relaxed), 2);
}
