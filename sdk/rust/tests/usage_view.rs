#[path = "support/wire.rs"]
mod wire;
use cma_sdk::{
    Error,
    usage_metrics::{ViewQuery, view_unavailable},
};
use serde_json::json;
use wire::WireServer;
fn pending() -> serde_json::Value {
    let id = "umr_00000000000000000000000000000001";
    json!({"schema_version":1,"window":"24h","requested_range":{"from":1700000000,"to":1700086400},
        "generated_at":null,"stale":false,"refresh":{"state":"pending","report_id":id,"retry_after_seconds":2},
        "report":{"schema_version":1,"report_id":id,"status":"pending","report_complete":false,
        "scope":{"principal":"owner","admin":true},"query":{"from":1700000000,"to":1700086400,"step_seconds":60,
        "filters":{"owner":null,"statuses":[],"runtime_version":null,"sandbox_id":null,"agent_id":null,"search":""}},
        "as_of":1700086400,"measurement_cutoff":1700086280,"effective_range":{"from":1700000000,"to":1700086280},
        "expires_at":1700172800,"allocation_watermark":"","mapping_revision":0,"available_from":null,
        "tracking_started_at":null,"warnings":[],"source_health":"pending","summary":null,
        "poll_url":format!("/api/v1/admin/usage/metrics/reports/{id}"),"retry_after_seconds":2}})
}
#[test]
fn resource_extensions_prevent_route_absence_fallback() {
    let error = Error::from_http_problem(
        404,
        None,
        None,
        br#"{"status":404,"title":"Not Found","reason":"concealed"}"#,
    );
    assert!(matches!(&error, Error::Api { code: None, extensions, .. } if extensions.len() == 1));
    assert!(!view_unavailable(&error));
    let absent =
        Error::from_http_problem(404, None, None, br#"{"status":404,"title":"Not Found"}"#);
    assert!(view_unavailable(&absent));
    let malformed = Error::from_http_problem(404, None, None, b"");
    assert!(matches!(malformed, Error::Protocol { .. }));
    assert!(!view_unavailable(&malformed));
}
#[tokio::test]
async fn latest_usage_uses_dedicated_route_and_refuses_mixed_scope_without_polling() {
    let server = WireServer::new().await;
    let path = "/api/v1/admin/usage/metrics/view?window=24h";
    server.expect(wire::json("GET", path, None, 202, pending(), true));
    let view = server
        .client
        .admin_usage_metrics()
        .latest(&ViewQuery::default())
        .await
        .unwrap();
    assert_eq!(view.report.status, "pending");
    assert_eq!(view.generated_at, None);
    let mut wrong = pending();
    wrong["report"]["scope"]["admin"] = json!(false);
    server.expect(wire::json("GET", path, None, 200, wrong, true));
    assert!(matches!(
        server
            .client
            .admin_usage_metrics()
            .latest(&ViewQuery::default())
            .await,
        Err(Error::Protocol { .. })
    ));
    for (status, code, allowed) in [
        (501, "report_view_disabled", true),
        (404, "not_found", false),
        (503, "usage_unavailable", false),
    ] {
        server.expect(wire::json(
            "GET",
            path,
            None,
            status,
            json!({"status":status,"code":code,"detail":"fixture"}),
            true,
        ));
        let error = server
            .client
            .admin_usage_metrics()
            .latest(&ViewQuery::default())
            .await
            .unwrap_err();
        assert_eq!(view_unavailable(&error), allowed);
    }
    assert_eq!(server.request_count(), 5);
    let _ = (&server.anonymous, &server.base_url);
    server.assert_drained();
}
