use super::*;

#[tokio::test]
async fn external_native_bot_capability_is_omitted_until_writes_enabled() {
    let fixture = Fixture::new().await;
    for enabled in [false, true] {
        let mut service = fixture.service();
        service.management.native_bot_mentions_writes_enabled = enabled;
        let response = crate::http::connections::capabilities(
            State(service),
            Creator {
                subject: "alice".into(),
                token_hash: "capability-test".into(),
                organization: false,
            },
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        if enabled {
            assert_eq!(value["source_features"], json!(["native_bot_mentions_v1"]));
        } else {
            assert!(value.get("source_features").is_none());
        }
    }
}

#[tokio::test]
async fn public_discovery_uses_canonical_namespace_and_console_redirect_keeps_fixed_authority() {
    public_discovery(false, None).await;
    public_discovery(true, None).await;
    public_discovery(true, Some("https://cma.nyxid.ai")).await;
}

async fn public_discovery(advertise: bool, console_origin: Option<&str>) {
    use axum::http::header;
    use utoipa::OpenApi;
    let fixture = Fixture::new().await;
    let mut service = fixture.service_with_console_origin(console_origin.map(str::to_owned));
    let expected_console = console_origin.unwrap_or("https://gateway.example");
    assert_eq!(service.management.console_public_url, expected_console);
    assert_eq!(service.management.public_url, "https://gateway.example");
    assert!(Arc::ptr_eq(&service.management.master_keys, &fixture.keys));
    service.management.public_api_url = "https://bot.example/event-gateway".into();
    service.management.advertise_public_api_url = advertise;
    let advertised = if advertise {
        "https://bot.example/event-gateway"
    } else {
        "https://gateway.example"
    };
    let record = connection("alice", false);
    assert!(service
        .view(&record)
        .unwrap()
        .setup_url
        .unwrap()
        .starts_with(&format!("{advertised}/connect/start/")));
    let app = crate::router_with_state(service.management);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let document: Value = client
        .get(format!("{base}/openapi.json"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        document["servers"],
        json!([{"url":format!("{advertised}/v1")}])
    );
    let static_document = serde_json::to_value(crate::ApiDoc::openapi()).unwrap();
    assert_eq!(document["paths"], static_document["paths"]);
    assert_eq!(document["components"], static_document["components"]);
    let docs = client.get(format!("{base}/docs")).send().await.unwrap();
    assert!(docs.headers()[header::CONTENT_SECURITY_POLICY]
        .to_str()
        .unwrap()
        .contains(if advertise {
            "img-src 'self' https://bot.example"
        } else {
            "img-src 'self' https://gateway.example"
        }));
    let html = docs.text().await.unwrap();
    for path in [
        "/docs/style.css",
        "/docs/download",
        "/openapi.json",
        "/docs/diagrams/",
    ] {
        assert!(html.contains(&format!("{advertised}{path}")), "{path}");
    }
    assert!(!html.contains("href=\"/docs"));
    assert!(!html.contains("https://gateway.example.com/event-gateway"));
    if !advertise {
        assert!(!html.contains("https://bot.chrono-ai.fun/event-gateway"));
        assert!(!html.contains("https://cma.nyxid.ai/event-gateway"));
        assert!(html.contains("href=\"https://gateway.example/console\""));
    }
    for path in ["/docs/download", "/docs/integration.md"] {
        let response = client.get(format!("{base}{path}")).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let markdown = response.text().await.unwrap();
        assert!(markdown.contains(&format!("{advertised}/v1")));
        assert!(markdown.contains(&format!("{advertised}/openapi.json")));
        assert!(!markdown.contains("https://gateway.example.com/event-gateway"));
        let console = if advertise {
            format!("{advertised}/legacy-console")
        } else {
            format!("{advertised}/console")
        };
        assert!(markdown.contains(&format!("<{console}>")));
    }
    let protocol_destination = format!("{advertised}/docs#protocol");
    let console_destination = format!("{expected_console}/console");
    let channel_destination = format!("{console_destination}/channels/record-123");
    for (path, destination) in [
        ("/docs/protocol", protocol_destination.as_str()),
        (
            "/event-gateway/legacy-console",
            console_destination.as_str(),
        ),
        (
            "/event-gateway/legacy-console/channels/record-123?return_to=https://foreign.example",
            channel_destination.as_str(),
        ),
    ] {
        let response = client.get(format!("{base}{path}")).send().await.unwrap();
        assert!(response.status().is_redirection());
        assert_eq!(response.headers()[header::LOCATION], destination);
    }
    for path in [
        "/event-gateway/legacy-console/channels/evil%2Fpath",
        "/event-gateway/legacy-console/channels/evil%5Cpath",
    ] {
        assert_eq!(
            client
                .get(format!("{base}{path}"))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(
        client
            .post(format!("{base}/event-gateway/legacy-console"))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::METHOD_NOT_ALLOWED
    );
    server.abort();
}
