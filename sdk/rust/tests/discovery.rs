#[path = "support/wire.rs"]
mod wire;

use cma_sdk::{Client, Error, Result};
use serde_json::json;
use std::{collections::BTreeMap, time::Duration};
use wire::WireServer;

#[tokio::test]
async fn openapi_is_anonymous_at_the_deployment_root_and_preserves_the_document() {
    let server = WireServer::new().await;
    let document = json!({
        "openapi": "3.1.0",
        "info": {"title": "CMA", "version": "1.0"},
        "paths": {"/api/v1/conformance": {"get": {"operationId": "readConformance"}}},
        "x-deployment": {"name": "fixture"}
    });
    for client in [&server.client, &server.anonymous] {
        server.expect(wire::json(
            "GET",
            "/openapi.json",
            None,
            200,
            document.clone(),
            false,
        ));
        assert_eq!(client.read_openapi().await.unwrap(), document);
    }
    assert!(server.base_url.ends_with("/gateway"));
    assert_eq!(server.request_count(), 2);
    server.assert_drained();
}

#[tokio::test]
async fn openapi_rejects_invalid_schema_declarations_and_unexpected_success_statuses() {
    let server = WireServer::new().await;
    for document in [
        json!(null),
        json!({"paths": {}}),
        json!({"openapi": 3, "paths": {}}),
        json!({"openapi": "2.0", "paths": {}}),
        json!({"openapi": "3.1.0"}),
        json!({"openapi": "3.1.0", "paths": []}),
    ] {
        server.expect(wire::json(
            "GET",
            "/openapi.json",
            None,
            200,
            document,
            false,
        ));
        assert!(matches!(
            server.client.read_openapi().await,
            Err(Error::Protocol { message })
                if message == "discovery did not return an OpenAPI 3 document"
        ));
    }
    server.expect(wire::json(
        "GET",
        "/openapi.json",
        None,
        201,
        json!({"openapi": "3.1.0", "paths": {}}),
        false,
    ));
    assert!(matches!(
        server.client.read_openapi().await,
        Err(Error::Protocol { message }) if message.contains("unexpected HTTP 201")
    ));
    assert_eq!(server.request_count(), 7);
    server.assert_drained();
}

const PROBES: [(&str, &str, &str, bool); 3] = [
    ("POST", "/api/v1/mcp", "mcp", true),
    ("POST", "/api/v1/a2a", "a2a", true),
    ("GET", "/.well-known/agent-card.json", "a2a", false),
];

async fn probe(client: &Client, path: &str) -> Result<()> {
    match path {
        "/api/v1/mcp" => client.unsupported().mcp().await,
        "/api/v1/a2a" => client.unsupported().a2a().await,
        "/.well-known/agent-card.json" => client.unsupported().agent_card().await.map(|_| ()),
        _ => panic!("unexpected discovery path"),
    }
}

#[tokio::test]
async fn unsupported_probes_preserve_credentials_policy_and_named_typed_problems() {
    let server = WireServer::new().await;
    for (method, path, interface, authenticated) in PROBES {
        let detail = format!("The {interface} interface is not yet available.");
        let mut expected = wire::json(
            method,
            path,
            None,
            501,
            json!({
                "type": "https://spec.chrono-ai.fun/cma/1.0/problems/interface-not-available",
                "title": "interface not available",
                "status": 501,
                "detail": detail,
                "code": "interface_not_available",
                "interface": interface
            }),
            authenticated,
        );
        expected.response_headers = BTreeMap::from([
            ("content-type".into(), "application/problem+json".into()),
            ("cache-control".into(), "no-store".into()),
            ("x-request-id".into(), "request-discovery".into()),
            ("retry-after".into(), "7".into()),
        ]);
        server.expect(expected);
        assert_eq!(
            probe(&server.client, path).await.unwrap_err(),
            Error::Api {
                status: 501,
                code: Some("interface_not_available".into()),
                message: detail,
                request_id: Some("request-discovery".into()),
                retry_after: Some(Duration::from_secs(7)),
                extensions: BTreeMap::from([("interface".into(), json!(interface))]),
            }
        );
    }
    assert_eq!(server.request_count(), 3);
    server.assert_drained();
}

#[tokio::test]
async fn unsupported_probes_refuse_unexpected_success() {
    let server = WireServer::new().await;
    for (method, path, _, authenticated) in PROBES
        .into_iter()
        .filter(|(_, path, _, _)| *path != "/.well-known/agent-card.json")
    {
        server.expect(wire::json(
            method,
            path,
            None,
            200,
            json!({}),
            authenticated,
        ));
        assert_eq!(
            probe(&server.client, path).await.unwrap_err(),
            Error::Protocol {
                message: "unsupported operation unexpectedly returned success".into(),
            }
        );
    }
    assert_eq!(server.request_count(), 2);
    server.assert_drained();
}

#[tokio::test]
async fn provider_card_is_typed_and_anonymous_through_both_entry_points() {
    let server = WireServer::new().await;
    let document = json!({"name":"CMA Codex provider","version":"0.1.0","url":"https://cma.example",
        "capabilities":{"streaming":true,"activity_items":true,"client_tools":false,"cancel":true,"rich_input":false},
        "provider_contract":{"binding":"/api/v1/cmaeg/bindings/{binding_id}","conversation":"/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}","responses":"/api/v2/responses"},
        "authentication":{"schemes":["bearer-agent-key"]}});
    for alias in [false, true] {
        server.expect(wire::json(
            "GET",
            "/.well-known/agent-card.json",
            None,
            200,
            document.clone(),
            false,
        ));
        let card = if alias {
            server.client.unsupported().agent_card().await.unwrap()
        } else {
            server.client.read_agent_card().await.unwrap()
        };
        assert_eq!(card.name, "CMA Codex provider");
        assert!(card.capabilities.activity_items && card.capabilities.cancel);
        assert!(!card.capabilities.client_tools);
        assert_eq!(card.provider_contract.responses, "/api/v2/responses");
    }
    server.assert_drained();
}
