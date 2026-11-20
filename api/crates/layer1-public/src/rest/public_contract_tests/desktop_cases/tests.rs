use super::*;
use axum::body::to_bytes;
use layer2_core::sandbox::{store::SandboxSessionStore, DesktopError, DesktopFrame};
use std::sync::atomic::Ordering;
use tower::ServiceExt;

fn websocket() -> Request<Body> {
    Request::builder()
        .uri("/api/v1/agents/agt_contract/desktop/ws")
        .header("connection", "upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", "YWFhYWFhYWFhYWFhYWFhYQ==")
        .header("origin", "https://cma.example")
        .body(Body::empty())
        .unwrap()
}

async fn unavailable(response: Response) {
    assert_eq!(response.status(), 503);
    assert_eq!(response.headers()["retry-after"], "1");
    assert_eq!(response.headers()["cache-control"], "no-store, private");
    assert_eq!(
        response.headers()["content-type"],
        "application/problem+json"
    );
    assert!(!response.headers().contains_key("set-cookie"));
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(
        body,
        serde_json::json!({
            "type": "https://spec.chrono-ai.fun/cma/1.0/problems/desktop_authority_unavailable",
            "title": "desktop_authority_unavailable", "status": 503,
            "detail": "desktop session authority is temporarily unavailable",
            "code": "desktop_authority_unavailable"
        })
    );
}

fn no_effects(ownership: &ownership::Ownership) {
    let state = ownership.state.lock().unwrap();
    assert!(state.claim.is_none() && state.receiver.is_none());
    assert_eq!(
        (state.writes, state.opens, state.frames, state.releases),
        (0, 0, 0, 0)
    );
}

#[tokio::test]
async fn absent_and_disabled_public_desktop_refuse_discovery_and_upgrade_without_effects() {
    for mode in ["desktop_default", "desktop_disabled"] {
        let f = fixture(mode).await;
        let request = Request::builder()
            .uri("/api/v1/agents/agt_contract/desktop")
            .extension(AuthenticatedSubject(SubjectId::new("owner-a")))
            .extension(f.scenario.authorization.clone())
            .body(Body::empty())
            .unwrap();
        unavailable(f.scenario.router.clone().oneshot(request).await.unwrap()).await;
        unavailable(upgrade_response(&f.scenario, websocket()).await).await;
        no_effects(&f.ownership);
        assert_eq!(f.ownership.checks.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn protected_public_desktop_requires_present_and_current_credential_authority() {
    for mode in ["desktop_no_authority", "desktop_owner"] {
        let f = fixture(mode).await;
        f.ownership.revoked.store(true, Ordering::SeqCst);
        unavailable(upgrade_response(&f.scenario, websocket()).await).await;
        no_effects(&f.ownership);
        assert_eq!(
            f.ownership.checks.load(Ordering::SeqCst),
            usize::from(mode == "desktop_owner")
        );
    }
}

#[tokio::test]
async fn protected_public_relay_revalidates_authority_and_releases_after_receiver_revoke() {
    let f = fixture("desktop_owner").await;
    assert_eq!(
        upgrade_response(&f.scenario, websocket()).await.status(),
        101
    );
    assert!(f.ownership.checks.load(Ordering::SeqCst) >= 2);
    let state = f.ownership.state.lock().unwrap();
    assert!(state.claim.is_none() && state.receiver.is_none());
    assert_eq!((state.writes, state.opens, state.releases), (1, 1, 1));
    assert!(state.frames >= 3);
}

#[tokio::test]
async fn protected_busy_holder_closes_exactly_and_delayed_cleanup_cannot_release_successor() {
    let f = fixture("desktop_busy").await;
    let caller = Caller::new(SubjectId::new("owner-a"), f.scenario.authorization.clone());
    let id = AgentId("agt_contract".into());
    assert!(matches!(
        f.service.available(&caller, &id).await,
        Err(DesktopError::Busy)
    ));
    let old = f.ownership.state.lock().unwrap().claim.clone().unwrap();
    let mut held = f.held.lock().await.take().unwrap();
    f.service.close_connection(&mut held).await;
    assert!(f.service.available(&caller, &id).await.is_ok());
    let mut next = f
        .service
        .open_authorized(&caller, &id, f.ownership.clone())
        .await
        .unwrap();
    assert!(
        f.ownership
            .state
            .lock()
            .unwrap()
            .claim
            .as_ref()
            .unwrap()
            .epoch
            > old.epoch
    );
    f.service.close_connection(&mut held).await;
    f.ownership
        .release_exclusive_sandbox_session(&old)
        .await
        .unwrap();
    next.send(DesktopFrame::Binary(vec![1])).await.unwrap();
    assert!(matches!(
        f.service.available(&caller, &id).await,
        Err(DesktopError::Busy)
    ));
    f.service.close_connection(&mut next).await;
    assert!(f.service.available(&caller, &id).await.is_ok());
    assert_eq!(f.ownership.state.lock().unwrap().releases, 2);
}
