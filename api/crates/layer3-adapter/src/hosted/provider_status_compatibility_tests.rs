use super::*;

#[tokio::test]
async fn status_accept_extension_keeps_retained_runtime_request_and_response_compatible() {
    let request = crate::RuntimeStatusRequest {
        sandbox_id: "sandbox".into(),
        generation: 1,
        home_identity: "a".repeat(43),
    };
    let original = serde_json::json!({"t":"runtime_status", "sandbox_id":"sandbox", "generation":1,
        "home_identity":"a".repeat(43), "boot_id":"a".repeat(32), "installation_ready":true,
        "credential_ready":true, "restore_pending":false, "agent_processes":0});
    let (transport, provider, runtime) = transport_fixture(json_response(&original)).await;
    let response = transport
        .exchange(
            &RuntimeHandle::new("runtime-1"),
            RequestFrame::RuntimeStatus(request.clone()),
        )
        .await
        .unwrap();
    assert_eq!(serde_json::to_value(response).unwrap(), original);
    provider.await.unwrap();
    let received = request_text(runtime.await.unwrap());
    assert!(received.to_ascii_lowercase().contains(&format!(
        "accept: {}, {}, {}, {}, {}, {}",
        crate::wire::AGENT_RESOURCES_STATUS_MEDIA_TYPE,
        crate::wire::AGENT_COMPUTER_STATUS_MEDIA_TYPE,
        crate::wire::AGENT_PROCESS_RESTART_STATUS_MEDIA_TYPE,
        crate::wire::AGENT_SKILL_UPGRADE_STATUS_MEDIA_TYPE,
        crate::wire::GITHUB_CREDENTIAL_ROUTE_STATUS_MEDIA_TYPE,
        crate::wire::GITHUB_REPOSITORY_SCOPE_STATUS_MEDIA_TYPE,
    )));
    let frame: RequestFrame =
        serde_json::from_str(received.split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(frame, RequestFrame::RuntimeStatus(request));
}
