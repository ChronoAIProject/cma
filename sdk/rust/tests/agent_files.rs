#[path = "support/wire.rs"]
mod wire;
use cma_sdk::{Client, Error};
use wire::{Expected, WireServer, headers};

#[tokio::test]
async fn file_download_has_its_own_bound_without_expanding_ordinary_api_responses() {
    let server = WireServer::new().await;
    assert!(matches!(
        server
            .anonymous
            .agent_files()
            .read("agent", "cma/report.bin", Some(0))
            .await,
        Err(Error::Validation { .. })
    ));
    assert_eq!(server.request_count(), 0);
    let content = vec![91; 9 * 1024 * 1024];
    server.expect(Expected {
        method: "GET".into(),
        path: "/api/v1/agents/agent/files/download?path=cma%2Freport.bin".into(),
        headers: headers(true, "application/json"),
        status: 200,
        response: content.clone(),
        ..Default::default()
    });
    assert_eq!(
        server
            .client
            .agent_files()
            .download("agent", "cma/report.bin")
            .await
            .unwrap(),
        content
    );
    server.expect(Expected {
        method: "GET".into(),
        path: "/api/v1/agents/agent/settings".into(),
        headers: headers(false, "application/json"),
        status: 200,
        response: content,
        ..Default::default()
    });
    let ordinary = Client::builder(&server.base_url).unwrap().build().unwrap();
    assert!(matches!(ordinary.agents().settings("agent").await,
        Err(Error::Protocol { message }) if message.contains("8388608-byte boundary")));
    server.assert_drained();
    assert_eq!(server.request_count(), 2);
}
