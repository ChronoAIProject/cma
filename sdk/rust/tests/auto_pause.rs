#[path = "support/wire.rs"]
mod wire;
use cma_sdk::{PageOptions, admin::PauseIdleNowRequest};
use serde_json::json;
use wire::{WireServer, json as response};

#[tokio::test]
async fn auto_pause_pages_preserve_old_replies_and_explicit_retry_cursors() {
    let server = WireServer::new().await;
    server.expect(response(
        "GET",
        "/api/v1/admin/auto-pause/preview",
        None,
        200,
        json!({"candidates": 2}),
        true,
    ));
    let old = server.client.admin().preview_auto_pause().await.unwrap();
    assert_eq!((old.candidates, old.next, old.failed), (2, None, None));
    server.expect(response("POST", "/api/v1/admin/auto-pause/pause-idle-now", Some(json!({"confirmed":true,"cursor":"","limit":25})), 200,
        json!({"scheduled":1,"already_satisfied":0,"skipped":0,"failed":1,"next":"","enabled":true,"policy_revision":9}), true));
    let result = server
        .client
        .admin()
        .pause_idle_now_page(
            &PauseIdleNowRequest { confirmed: true },
            &PageOptions {
                cursor: Some("".into()),
                limit: Some(25),
            },
        )
        .await
        .unwrap();
    assert_eq!(result.next.as_deref(), Some(""));
    assert_eq!(result.policy_revision, Some(9));
    assert!(
        server
            .client
            .admin()
            .preview_auto_pause_page(&PageOptions {
                cursor: None,
                limit: Some(101)
            })
            .await
            .is_err()
    );
    server.assert_drained();
    assert_eq!(server.request_count(), 2);
    let _ = (&server.anonymous, &server.base_url);
}
