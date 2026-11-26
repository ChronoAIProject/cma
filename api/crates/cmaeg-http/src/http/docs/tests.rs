use super::*;

#[test]
fn published_guide_explains_channels_and_the_optional_consumer_route() {
    let html = page();
    assert!(html.contains("id=\"integration-create-a-channel-in-cma\""));
    assert!(html.contains("href=\"#integration-create-a-channel-in-cma\""));
    let (_, channel) = html
        .split_once("id=\"integration-create-a-channel-in-cma\"")
        .unwrap();
    let (channel, consumer) = channel
        .split_once("id=\"integration-create-a-cma-consumer\"")
        .unwrap();
    for label in [
        "New channel",
        "Create channel",
        "Check original channel",
        "Retry same request",
    ] {
        assert!(channel.contains(label), "missing channel action {label}");
    }
    assert!(!channel.contains("New consumer"));
    assert!(consumer.contains("has a <strong>Consumers</strong> tab"));
    assert!(download_markdown().contains("](#integration-create-a-channel-in-cma)"));
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
