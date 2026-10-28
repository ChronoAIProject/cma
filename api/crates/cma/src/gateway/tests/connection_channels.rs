use super::*;
use axum::{
    extract::State,
    http::{HeaderMap, Method, StatusCode},
    response::IntoResponse,
    routing::any,
    Json, Router,
};
use std::sync::Mutex;

struct Peer {
    channel: Mutex<Value>,
    requests: Mutex<Vec<Value>>,
}
async fn response(
    State(peer): State<Arc<Peer>>,
    method: Method,
    headers: HeaderMap,
    body: String,
) -> axum::response::Response {
    assert_eq!(headers["authorization"], "Bearer owner-token");
    let mut channel = peer.channel.lock().unwrap();
    if method != Method::GET {
        let body: Value = serde_json::from_str(&body).unwrap();
        if method == Method::PUT {
            assert_eq!(body["expected_version"], channel["version"]);
            channel["version"] = (channel["version"].as_u64().unwrap() + 1).into();
        }
        if let Some(key) = body["credentials"]["agent_key"].as_str() {
            channel["definition"]["secrets"]["agent_key_hash"] = format!(
                "sha256:{}",
                ProvidedCredential::new(key.into()).unwrap().sha256()
            )
            .into();
        }
        peer.requests.lock().unwrap().push(body);
    }
    (
        if method == Method::POST {
            StatusCode::CREATED
        } else {
            StatusCode::OK
        },
        [("cmaeg-protocol-version", "1")],
        Json(channel.clone()),
    )
        .into_response()
}

#[tokio::test]
async fn generic_channel_creation_and_updates_never_send_native_platform_or_route_credentials() {
    let mut bot = bot().await;
    bot.request.external_bot_id.clear();
    bot.request.connection_ref = Some("opaque-connection".into());
    let original = BotSecrets {
        agent: ProvidedCredential::new("agent-fixture".into()).unwrap(),
        route: None,
    };
    bot.resources.credentials = BotCredentialSet {
        agent_key_id: Some("agent-key".into()),
        agent_key_hash: Some(original.agent.sha256()),
        ..Default::default()
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let peer = Arc::new(Peer {
        channel: Mutex::new(json!({
            "channel_id":"native-channel", "record_id":bot.record_id, "version":1,
            "provider":{"slug":bot.provider,"binding_id":"binding-native","level":"l2","refused":[]},
            "endpoints":{"nyxid_callback_url":format!("{base}/v1/callbacks/native-channel"),"custom_event_url":format!("{base}/v1/channels/native-channel/events")},
            "definition":{"record_id":bot.record_id,"owner":{"kind":"human","subject":bot.owner},"profile":profile::snapshot(&bot).unwrap(),
                "connection_ref":"opaque-connection", "sources":[{"type":"future-native-source","platform":"future-platform","native_id":"123"}],
                "secrets":{"agent_key_hash":format!("sha256:{}",original.agent.sha256())},"creation":{"ready":true}}
        })),
        requests: Mutex::new(vec![]),
    });
    let app = Router::new()
        .route("/v1/channels", any(response))
        .route("/v1/channels/{id}", any(response))
        .route("/v1/channel-records/{id}", any(response))
        .with_state(peer.clone());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let client = client::GatewayClient::new(
        reqwest::Client::new(),
        &GatewayConfig {
            binding_preparation_enabled: false,
            base_url: base.clone(),
            public_url: base.clone(),
            public_api_url: "https://bot.example/event-gateway".into(),
        },
        "https://identity.example".into(),
    )
    .unwrap();
    let created = client
        .create("owner-token", &bot, &original, None)
        .await
        .unwrap();
    assert!(created.uses(&bot.resources.credentials));
    assert!(created.route_key_id.is_empty() && created.route_ids.is_empty());
    let disabled = client
        .configure(
            "owner-token",
            &bot,
            &created,
            &bot.resources.credentials,
            None,
            false,
        )
        .await
        .unwrap();
    assert_eq!(disabled.channel.version, 2);
    {
        let requests = peer.requests.lock().unwrap();
        for request in requests.iter() {
            assert_eq!(request["connection_ref"], "opaque-connection");
            assert_eq!(request["sources"], json!([]));
            assert!(request.get("platform").is_none());
            assert!(request["credentials"].get("channel_key").is_none());
        }
        assert_eq!(
            requests[0]["credentials"],
            json!({"agent_key":"agent-fixture"})
        );
        assert!(requests[1].get("credentials").is_none());
        assert_eq!(
            requests[1]["connection_admission"]["senders"]["type"],
            "deny"
        );
    }
    for allowed in [base.as_str(), "https://bot.example/event-gateway"] {
        let callback = format!("{allowed}/v1/callbacks/native-channel");
        let events = format!("{allowed}/v1/channels/native-channel/events");
        peer.channel.lock().unwrap()["endpoints"] =
            json!({"nyxid_callback_url":callback,"custom_event_url":events});
        assert!(client.channel("owner-token", &bot).await.unwrap().is_some());
        assert!(client.valid_callback("native-channel", &callback));
        assert!(!client.valid_callback("other-channel", &callback));
    }
    for forbidden in [
        "https://foreign.example/v1/callbacks/native-channel",
        "https://bot.example/event-gateway/v1/callbacks/native-channel?extra=1",
        "https://bot.example/event-gateway/v1/callbacks/native-channel/suffix",
    ] {
        assert!(!client.valid_callback("native-channel", forbidden));
    }
    peer.channel.lock().unwrap()["endpoints"]["nyxid_callback_url"] =
        format!("{base}/v1/callbacks/native-channel").into();
    assert!(matches!(
        client.channel("owner-token", &bot).await,
        Err(BotError::Unavailable)
    ));
    peer.channel.lock().unwrap()["definition"]["connection_ref"] = "foreign-connection".into();
    assert!(matches!(
        client.channel("owner-token", &bot).await,
        Err(BotError::Unavailable)
    ));
    task.abort();
}
