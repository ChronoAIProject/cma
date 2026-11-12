#[path = "support/wire.rs"]
mod wire;

use cma_sdk::{
    Error,
    usage::{DocumentUsage, DocumentUsageEnforcement, DocumentUsageQuota},
};
use serde_json::json;
use wire::WireServer;

#[tokio::test]
async fn document_usage_reads_authenticated_owner_quotas() {
    let server = WireServer::new().await;
    server.expect(wire::json(
        "GET",
        "/api/v1/usage/documents",
        None,
        200,
        json!({
            "enforcement":"enforced", "documents":{"used":2,"limit":100},
            "bytes":{"used":1024,"limit":1073741824},
            "objects":{"used":3,"limit":32768}
        }),
        true,
    ));
    let result: DocumentUsage = server.client.usage().documents().await.unwrap();
    assert_eq!(result.enforcement, DocumentUsageEnforcement::Enforced);
    assert_eq!(
        result.documents,
        Some(DocumentUsageQuota {
            used: 2,
            limit: 100
        })
    );
    assert_eq!(result.bytes.unwrap().used, 1024);
    assert_eq!(result.objects.unwrap().limit, 32768);
    assert_eq!(server.request_count(), 1);
    let _ = (&server.anonymous, &server.base_url);
    server.assert_drained();
}

#[tokio::test]
async fn document_usage_preserves_unknown_counters_when_not_enforced() {
    let server = WireServer::new().await;
    server.expect(wire::json(
        "GET",
        "/api/v1/usage/documents",
        None,
        200,
        json!({"enforcement":"not_enforced","documents":null,"bytes":null,"objects":null}),
        true,
    ));
    let result = server.client.usage().documents().await.unwrap();
    assert_eq!(result.enforcement, DocumentUsageEnforcement::NotEnforced);
    assert_eq!(result.documents, None);
    assert_eq!(result.bytes, None);
    assert_eq!(result.objects, None);
    let _ = (&server.anonymous, &server.base_url);
    server.assert_drained();
}

#[tokio::test]
async fn document_usage_rejects_malformed_counters_and_enforcement() {
    let server = WireServer::new().await;
    for invalid in [
        json!({"enforcement":"unknown","documents":null,"bytes":null,"objects":null}),
        json!({"enforcement":"enforced","documents":{"used":-1,"limit":100},"bytes":null,"objects":null}),
        json!({"enforcement":"enforced","documents":null,"bytes":{"used":1.5,"limit":100},"objects":null}),
        json!({"enforcement":"enforced","documents":null,"bytes":null,"objects":{"used":1,"limit":"100"}}),
    ] {
        server.expect(wire::json(
            "GET",
            "/api/v1/usage/documents",
            None,
            200,
            invalid,
            true,
        ));
        assert!(matches!(
            server.client.usage().documents().await,
            Err(Error::Protocol { .. })
        ));
    }
    let _ = (&server.anonymous, &server.base_url);
    server.assert_drained();
}

#[tokio::test]
async fn document_usage_preserves_named_authorization_failures() {
    let server = WireServer::new().await;
    server.expect(wire::json(
        "GET",
        "/api/v1/usage/documents",
        None,
        401,
        json!({"status":401,"code":"unauthenticated","detail":"Authentication required."}),
        true,
    ));
    assert!(matches!(
        server.client.usage().documents().await,
        Err(Error::Api { status: 401, code: Some(code), .. }) if code == "unauthenticated"
    ));
    let _ = (&server.anonymous, &server.base_url);
    server.assert_drained();
}
