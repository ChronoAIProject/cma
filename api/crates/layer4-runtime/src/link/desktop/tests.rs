use super::*;
use axum::{
    routing::{get, post},
    Router,
};
use layer3_adapter::desktop_ownership::wire::{
    DesktopChallengeRequest, DesktopConfirmation, DESKTOP_OWNER_HEADER,
};
use layer3_adapter::{DesktopOwnership, DesktopOwnershipChallenge};

#[tokio::test]
async fn authenticated_control_refuses_ownerless_and_drains_owned_websockets() {
    let home = std::env::temp_dir().join(format!("cma-desktop-http-{}", uuid::Uuid::new_v4()));
    let config = crate::config::RuntimeConfig::for_test(home.clone());
    let state = config.state_root.clone();
    let checkpoint = config.checkpoint_root.clone();
    let sandbox = config.sandbox_id.clone();
    let token = uuid::Uuid::new_v4().simple().to_string().repeat(2);
    let fixture = test_support::Fixture::start(config, token.clone()).await;
    let address = fixture.address;
    let http = reqwest::Client::new();
    let url = format!("http://{address}/runtime/desktop/ownership");
    let request = DesktopChallengeRequest {
        sandbox_id: sandbox.clone(),
        runtime_id: "external-handle".into(),
        previous: None,
    };
    assert_eq!(
        http.post(format!("{url}/challenge"))
            .json(&request)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let mut handshake = format!("ws://{address}/runtime/desktop")
        .into_client_request()
        .unwrap();
    handshake
        .headers_mut()
        .insert("x-cma-runtime-token", token.parse().unwrap());
    let refused = tokio_tungstenite::connect_async(handshake.clone()).await;
    assert!(matches!(refused,
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) if response.status() == 400));
    let challenge: DesktopOwnershipChallenge = http
        .post(format!("{url}/challenge"))
        .header("x-cma-runtime-token", &token)
        .json(&request)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let owner = DesktopOwnership {
        sandbox_id: sandbox,
        runtime_id: "external-handle".into(),
        boot_id: None,
        session_id: "exact-session".into(),
        epoch: 1,
        issued_at_ms: 1000,
        expires_at_ms: 16000,
    };
    let confirm = DesktopConfirmation {
        ownership: owner.clone(),
        nonce: challenge.nonce,
    };
    let response = http
        .post(format!("{url}/confirm"))
        .header("x-cma-runtime-token", &token)
        .json(&confirm)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert!(tokio_tungstenite::connect_async(handshake.clone())
        .await
        .is_err());
    handshake.headers_mut().insert(
        DESKTOP_OWNER_HEADER,
        serde_json::to_string(&owner).unwrap().parse().unwrap(),
    );
    let (mut owned, _) = tokio_tungstenite::connect_async(handshake).await.unwrap();
    owned
        .send(TungsteniteMessage::Binary(vec![2]))
        .await
        .unwrap();
    assert_eq!(
        owned.next().await.unwrap().unwrap(),
        TungsteniteMessage::Binary(vec![2])
    );
    assert_eq!(
        http.post(format!("{url}/revoke"))
            .header("x-cma-runtime-token", &token)
            .json(&owner)
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let closed = tokio::time::timeout(Duration::from_secs(1), owned.next())
        .await
        .expect("owned revoke must close the socket");
    assert!(matches!(
        closed,
        None | Some(Err(_)) | Some(Ok(TungsteniteMessage::Close(_)))
    ));
    let renewal = DesktopChallengeRequest {
        previous: Some(owner),
        ..request
    };
    assert_eq!(
        http.post(format!("{url}/challenge"))
            .header("x-cma-runtime-token", &token)
            .json(&renewal)
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    fixture.stop().await;
    for path in [home, state, checkpoint] {
        let _ = std::fs::remove_dir_all(path);
    }
}

mod child_fixture;
mod child_process_tests;
mod test_support;
