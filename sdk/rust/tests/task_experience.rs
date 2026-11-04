#[path = "support/wire.rs"]
mod wire;
use cma_sdk::Error;
use wire::{Expected, WireServer, headers};

#[tokio::test]
async fn task_experience_reads_fresh_authenticated_policy_and_rejects_unknown_schema() {
    let server = WireServer::new().await;
    for version in [1, 2] {
        server.expect(Expected {
            method: "GET".into(), path: "/api/v2/ui/task-experience".into(),
            headers: headers(true, "application/json"), status: 200,
            response: serde_json::to_vec(&serde_json::json!({"object":"cma.ui.task_experience", "schema_version":version, "account_id":"owner-a",
                "capabilities":{"task_home_admission":true,"suggested_followup_admission":false,
                    "desktop_enable":false,"browser_takeover":false,"replay_presentation":true}})).unwrap(),
            ..Default::default()
        });
    }
    let policy = server.client.read_task_experience().await.unwrap();
    assert!(policy.capabilities.task_home_admission);
    assert!(!policy.capabilities.desktop_enable);
    assert!(matches!(
        server.client.read_task_experience().await,
        Err(Error::Protocol { .. })
    ));
    server.assert_drained();
    assert_eq!(server.request_count(), 2);
    let rebuilt = cma_sdk::Client::builder(&server.base_url)
        .unwrap()
        .build()
        .unwrap();
    for client in [&server.anonymous, &rebuilt] {
        server.expect(Expected {
            method: "GET".into(),
            path: "/api/v2/ui/task-experience".into(),
            headers: headers(false, "application/json"),
            status: 401,
            response_headers: [("content-type".into(), "application/problem+json".into())].into(),
            response: serde_json::to_vec(&serde_json::json!({"type":"urn:cma:unauthenticated",
                "title":"Authentication required", "status":401}))
            .unwrap(),
            ..Default::default()
        });
        assert!(matches!(
            client.read_task_experience().await,
            Err(Error::Api { status: 401, .. })
        ));
    }
    server.assert_drained();
    assert_eq!(server.request_count(), 4);
}
