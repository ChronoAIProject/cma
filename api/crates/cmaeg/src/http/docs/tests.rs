use super::*;

#[tokio::test]
async fn configured_docs_rewrite_public_example_urls_in_every_format() {
    for (advertise, api_base) in [
        (false, "https://gateway.example"),
        (true, "https://bot.example/event-gateway"),
    ] {
        let app = router_with_base(api_base, advertise);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = reqwest::Client::new();
        for (path, example_path) in [
            ("/docs", "/v1"),
            ("/docs/download", "/v1"),
            ("/docs/integration.md", "/v1"),
            ("/docs/architecture.md", "/docs"),
            ("/docs/protocol.md", "/v1"),
        ] {
            let response = client
                .get(format!("{base}{path}"))
                .header(header::HOST, "untrusted.example")
                .header("x-forwarded-host", "untrusted.example")
                .query(&[("return_to", "https://foreign.example")])
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 200, "{path}");
            assert_eq!(
                response.headers()[header::CONTENT_TYPE],
                if path == "/docs" {
                    "text/html; charset=utf-8"
                } else {
                    "text/markdown; charset=utf-8"
                }
            );
            let body = response.text().await.unwrap();
            assert!(
                body.contains(&format!("{api_base}{example_path}")),
                "{path}"
            );
            for unexpected in [
                "https://gateway.example.com/event-gateway",
                "https://cma.nyxid.ai/event-gateway",
                "untrusted.example",
                "foreign.example",
            ] {
                assert!(!body.contains(unexpected), "{path}: {unexpected}");
            }
            if matches!(path, "/docs" | "/docs/download" | "/docs/integration.md") {
                let console_path = if advertise {
                    "/legacy-console"
                } else {
                    "/console"
                };
                let console = format!("{api_base}{console_path}");
                assert!(body.contains(&console), "{path}: {console}");
                if path == "/docs" {
                    assert!(body.contains(&format!("href=\"{console}\"")));
                }
            }
        }
        server.abort();
    }
}

#[test]
fn embedded_document_url_roots_preserve_console_advertisement_semantics() {
    let base = "https://bot.example/event-gateway";
    for root in [
        "https://gateway.example.com/event-gateway",
        "https://cma.nyxid.ai/event-gateway",
    ] {
        let source = format!(
            "{root}\n{root}/v1\n{root}?schema=1#latest\n{root}/legacy-console\n{root}/legacy-console/channels/record-123?tab=events#latest\n"
        );
        for advertise in [false, true] {
            let console_path = if advertise {
                "/legacy-console"
            } else {
                "/console"
            };
            let expected = format!(
                "{base}\n{base}/v1\n{base}?schema=1#latest\n{base}{console_path}\n{base}{console_path}/channels/record-123?tab=events#latest\n"
            );
            assert_eq!(advertised_document(&source, base, advertise), expected);
            assert_eq!(advertised_document(&source, "", advertise), source);
        }
    }
}

#[test]
fn document_url_rewriting_leaves_similar_hosts_and_path_prefixes_unchanged() {
    let source = [
        "https://gateway.example.com.evil.example/event-gateway/v1",
        "https://gateway.example.com@foreign.example/event-gateway/v1",
        "https://gateway.example.com/event-gateway-preview/v1",
        "https://cma.nyxid.ai.evil.example/event-gateway/v1",
        "https://cma.nyxid.ai/event-gateway-preview/v1",
    ]
    .join("\n");
    for advertise in [false, true] {
        assert_eq!(
            advertised_document(&source, "https://bot.example/event-gateway", advertise),
            source
        );
    }
}

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
fn document_links_headings_and_unsafe_markup_are_handled() {
    let (output, _) = render::render("# Heading\n\n## 8. Tools API and MCP\n\n## 8. Tools API and MCP\n\n[protocol](CMAEG_Protocol.md#8-tools-api-and-mcp)\n\n[internal](CMAEG_Operations.md)\n\n[bad](javascript:alert%281%29)\n\n<img src=x onerror=alert(1)>\n", "protocol");
    assert!(output.contains("id=\"protocol-8-tools-api-and-mcp\""));
    assert!(output.contains("id=\"protocol-8-tools-api-and-mcp-1\""));
    assert!(output.contains("href=\"#protocol-8-tools-api-and-mcp\""));
    assert!(!output.contains("CMAEG_Operations"));
    assert!(!output.contains("javascript:"));
    assert!(!output.contains("<img"));
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

#[test]
fn sanitized_guide_links_use_the_configured_gateway_origin() {
    let source = "https://gateway.example.com/event-gateway/legacy-console https://gateway.example.com/event-gateway/v1";
    assert_eq!(
        advertised_document(source, "https://gateway.example", false),
        "https://gateway.example/console https://gateway.example/v1"
    );
    assert_eq!(
        advertised_document(source, "https://public.example/event-gateway", true),
        "https://public.example/event-gateway/legacy-console https://public.example/event-gateway/v1"
    );
}
