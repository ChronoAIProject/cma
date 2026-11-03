use axum::{
    body::to_bytes,
    http::{Method, StatusCode},
    Router,
};
use layer3_adapter::*;
use serde_json::Value;
use std::sync::Arc;
use tower::ServiceExt as _;

#[async_trait::async_trait]
impl BrowserAdapter for super::runtime::Files {
    async fn browser(
        &self,
        handle: &RuntimeHandle,
        request: &BrowserRequest,
    ) -> Result<BrowserReceipt> {
        assert_eq!(handle.as_str(), "runtime-route");
        assert_eq!(request.sandbox_id, "sbx_route");
        assert_eq!(request.runtime_generation, 1);
        assert_eq!(request.agent_id.as_str(), "agt_shared_read");
        assert_eq!(request.boot_id, "b".repeat(32));
        if let Some(observer) = &request.observer {
            assert_eq!(observer.len(), 64);
            assert_ne!(observer, "observer");
        }
        Ok(BrowserReceipt {
            agent_id: request.agent_id.clone(),
            boot_id: request.boot_id.clone(),
            observation: self.browser_reply.lock().unwrap().clone(),
        })
    }
}

async fn read(router: &Router, path: &str, subject: &str) -> (StatusCode, Value) {
    let response = router
        .clone()
        .oneshot(crate::rest::agents_tests::request(
            Method::GET,
            path,
            subject,
            None,
            None,
        ))
        .await
        .unwrap();
    let status = response.status();
    assert_eq!(response.headers()["cache-control"], "private, no-store");
    let bytes = to_bytes(response.into_body(), 400000).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

fn controlled_frame() -> BrowserObservation {
    let mut observed = BrowserObservation::idle(BrowserState::Live);
    observed.context_id = Some("e9ed9ab9-02a9-4c1e-91f5-7e13ca191b24".into());
    observed.location = "Local page".into();
    observed.frame = Some(BrowserFrame {
        sequence: 1,
        captured_at: "2026-11-03T05:02:55.559Z".into(),
        media_type: "image/jpeg".into(),
        width: 1280,
        height: 720,
        data: "/9j/AA==".into(),
    });
    observed
}

#[tokio::test]
async fn public_browser_bounds_frames_and_withdraws_disabled_context_without_transferring_a_grant()
{
    let runtime = Arc::new(super::runtime::Files::new("browser"));
    let owner = super::scenario_with_runtime("filesystem_owner", runtime.clone())
        .await
        .unwrap();
    let (_, environment) = read(
        &owner.router,
        "/api/v1/agents/agt_shared_read/environment",
        "owner-a",
    )
    .await;
    assert_eq!(environment["browser"]["state"], "not_enabled");
    let incarnation = environment["runtime_incarnation"].as_str().unwrap();
    let path = format!("/api/v1/agents/agt_shared_read/browser?runtime_incarnation={incarnation}&observer=observer&context_id=e9ed9ab9-02a9-4c1e-91f5-7e13ca191b24");
    *runtime.browser_reply.lock().unwrap() = controlled_frame();
    let (status, frame) = read(&owner.router, &path, "owner-a").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(frame["state"], "live");
    assert_eq!(frame["frame"]["data"], "/9j/AA==");
    assert_eq!(frame["runtime_incarnation"], incarnation);
    for bad in ["address", "oversized", "context"] {
        let mut changed = controlled_frame();
        match bad {
            "address" => changed.location = "http://127.0.0.1/private?token=secret".into(),
            "oversized" => changed.frame.as_mut().unwrap().data = "A".repeat(349529),
            _ => changed.context_id = Some("79a0e773-7691-401e-89e8-7487345c221a".into()),
        }
        *runtime.browser_reply.lock().unwrap() = changed;
        let (status, body) = read(&owner.router, &path, "owner-a").await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert!(body.get("frame").is_none());
        assert!(!body.to_string().contains("secret"));
    }
    *runtime.browser_reply.lock().unwrap() = BrowserObservation::idle(BrowserState::NotEnabled);
    let (status, stopped) = read(&owner.router, &path, "owner-a").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(stopped["state"], "not_enabled");
    assert_eq!(stopped["frame"], Value::Null);
    *runtime.browser_reply.lock().unwrap() = controlled_frame();
    let viewer = super::scenario_with_runtime("filesystem_viewer", runtime.clone())
        .await
        .unwrap();
    assert_eq!(
        read(&viewer.router, &path, "link-recipient").await.0,
        StatusCode::OK
    );
    let revoked = super::scenario_with_runtime("filesystem_revoked", runtime)
        .await
        .unwrap();
    let (status, body) = read(&revoked.router, &path, "link-recipient").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body.get("frame").is_none());
}
