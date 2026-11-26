use axum::http::header;

#[path = "../../../../cmaeg-http/src/http/docs/diagrams.rs"]
mod diagrams;

const GUIDE: &str = include_str!("../../../assets/docs/public/CMAEG_Integration.md");
const ARCHITECTURE: &str = include_str!("../../../assets/docs/public/CMAEG_Architecture.md");
const PROTOCOL: &str = include_str!("../../../assets/docs/public/CMAEG_Protocol.md");

#[tokio::test]
async fn public_docs_are_real_html_and_exact_markdown_without_authentication() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, crate::router()).await.unwrap();
    });
    let client = reqwest::Client::new();
    for (path, expected) in [
        ("/docs", "Implement a downstream agent provider"),
        (
            "/docs/architecture",
            "Full event context and restart recovery",
        ),
        ("/docs/protocol", "8. Tools API and MCP"),
    ] {
        let response = client.get(format!("{base}{path}")).send().await.unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/html; charset=utf-8"
        );
        assert!(response.headers()[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap()
            .contains("default-src 'none'"));
        let body = response.text().await.unwrap();
        assert!(body.contains(expected));
        assert!(body.contains("NyxID REST proxy base (example)"));
        assert!(body.contains("https://nyxid.example.com/api/v1/proxy/s/cmaeg"));
        assert!(body.contains("<strong>Retry creation</strong>"));
        assert!(body.contains("<strong>Resume creation</strong>"));
        assert!(!body.contains("Production address"));
        assert!(body.contains("<nav aria-label=\"Sections\">"));
        assert!(body.contains("<main id=\"content\"><h1"));
        assert!(body.contains("href=\"/openapi.json\""));
        assert!(!body.contains("<script"));
        assert!(!body.contains("language-mermaid"));
        assert_eq!(
            body.matches("class=\"diagram\"").count(),
            diagrams::DIAGRAMS.len()
        );
        assert!(body.contains("href=\"/docs/download\""));
        assert!(!body.contains("href=\"CMAEG_Operations.md"));
    }
    for (path, expected) in [
        ("/docs/integration.md", GUIDE),
        ("/docs/architecture.md", ARCHITECTURE),
        ("/docs/protocol.md", PROTOCOL),
    ] {
        let response = client.get(format!("{base}{path}")).send().await.unwrap();
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/markdown; charset=utf-8"
        );
        assert_eq!(response.text().await.unwrap(), expected);
    }
    let download = client
        .get(format!("{base}/docs/download"))
        .send()
        .await
        .unwrap();
    assert_eq!(download.status(), 200);
    assert_eq!(
        download.headers()[header::CONTENT_DISPOSITION],
        "attachment; filename=\"agent-event-gateway.md\""
    );
    let markdown = download.text().await.unwrap();
    for text in [
        "Implement a downstream agent provider",
        "Full event context and restart recovery",
        "Flow H — Replica restart",
        "8. Tools API and MCP",
        "NyxID REST proxy base (example)",
        "https://nyxid.example.com/api/v1/proxy/s/cmaeg",
        "**Retry creation**",
        "**Resume creation**",
    ] {
        assert!(markdown.contains(text));
    }
    for (_, name, _) in diagrams::DIAGRAMS {
        let response = client
            .get(format!("{base}/docs/diagrams/{name}.svg"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "image/svg+xml");
    }
    assert_eq!(
        client
            .get(format!("{base}/docs/operations.md"))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    let spec = client
        .get(format!("{base}/openapi.json"))
        .send()
        .await
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap();
    assert!(spec["paths"]["/channels"].is_object());
    server.abort();
}

#[test]
fn integration_channel_example_passes_the_production_validator() {
    use crate::channels::definition::{validate_create_request, ChannelCreateRequest};
    use base64::Engine as _;
    let raw = GUIDE
        .split("```json\n")
        .nth(1)
        .unwrap()
        .split("\n```")
        .next()
        .unwrap()
        .replace(
            "<new-UUID-v4-reserved-for-this-channel>",
            &uuid::Uuid::new_v4().to_string(),
        )
        .replace("<creator-subject>", "example-owner")
        .replace("<model-service-slug>", "model-service")
        .replace("<supported-model>", "example-model")
        .replace(
            "<dedicated-agent-key>",
            &format!("nyx_{}", uuid::Uuid::new_v4().simple()),
        )
        .replace(
            "<random-base64-string>",
            &base64::engine::general_purpose::STANDARD.encode(uuid::Uuid::new_v4().as_bytes()),
        );
    let request: ChannelCreateRequest = serde_json::from_str(&raw).unwrap();
    validate_create_request(&request).unwrap();
}
