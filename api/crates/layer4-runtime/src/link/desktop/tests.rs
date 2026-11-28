use super::*;
use axum::{
    routing::{get, post},
    Router,
};
use layer3_adapter::desktop_ownership::wire::{
    DesktopChallengeRequest, DesktopConfirmation, DESKTOP_OWNER_HEADER,
};
use layer3_adapter::{DesktopOwnership, DesktopOwnershipChallenge};
use std::time::Duration;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

#[tokio::test]
async fn only_terminal_desktop_unavailability_has_a_failed_state_header() {
    for retryable in [true, false] {
        let response = unavailable(retryable);
        assert_eq!(
            response.status(),
            axum::http::StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            response
                .headers()
                .get(DESKTOP_STATE_HEADER)
                .map(|value| value.to_str().unwrap()),
            (!retryable).then_some(DESKTOP_STATE_FAILED),
        );
        assert_eq!(
            response
                .headers()
                .get(axum::http::header::RETRY_AFTER)
                .map(|value| value.to_str().unwrap()),
            retryable.then_some("1"),
        );
        let bytes = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["code"], "desktop_unavailable");
        assert_eq!(body["retryable"], retryable);
    }
}

#[test]
fn a_non_machine_never_owns_a_desktop_with_supervision_enabled() {
    let home = std::env::temp_dir().join(format!("cma-non-machine-{}", uuid::Uuid::new_v4()));
    let mut config = crate::config::RuntimeConfig::for_test(home.clone());
    config.desktop_supervision_enabled = true;
    let paths = [
        home,
        config.state_root.clone(),
        config.checkpoint_root.clone(),
    ];
    let supervisor =
        Supervisor::new(config, crate::writer_fence::SupervisorBoundary::default()).unwrap();
    assert!(supervisor.desktop_generation().is_none());
    drop(supervisor);
    for path in paths {
        if let Err(error) = std::fs::remove_dir_all(path) {
            assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
        }
    }
}

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

#[tokio::test]
async fn invalidated_generation_cancels_pending_upgrades_and_active_relays() {
    for pending in [false, true] {
        let home =
            std::env::temp_dir().join(format!("cma-desktop-rotation-{}", uuid::Uuid::new_v4()));
        let config = crate::config::RuntimeConfig::for_test(home.clone());
        let paths = [
            home,
            config.state_root.clone(),
            config.checkpoint_root.clone(),
        ];
        let sandbox_id = config.sandbox_id.clone();
        let access = uuid::Uuid::new_v4().simple().to_string().repeat(2);
        let (generation, lifetime) =
            DesktopGeneration::legacy("current".into()).with_invalidation();
        let (accepted, handshake) = tokio::sync::oneshot::channel();
        let fixture = test_support::Fixture::with_generation(
            config,
            access.clone(),
            generation,
            pending.then_some(accepted),
        )
        .await;
        let address = fixture.address;
        let client = reqwest::Client::new();
        let ownership = format!("http://{address}/runtime/desktop/ownership");
        let request = DesktopChallengeRequest {
            sandbox_id: sandbox_id.clone(),
            runtime_id: "fixture-runtime".into(),
            previous: None,
        };
        let challenge: DesktopOwnershipChallenge = client
            .post(format!("{ownership}/challenge"))
            .header("x-cma-runtime-token", &access)
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
            sandbox_id,
            runtime_id: request.runtime_id,
            boot_id: None,
            session_id: "fixture-viewer".into(),
            epoch: 1,
            issued_at_ms: 1000,
            expires_at_ms: 16000,
        };
        client
            .post(format!("{ownership}/confirm"))
            .header("x-cma-runtime-token", &access)
            .json(&DesktopConfirmation {
                ownership: owner.clone(),
                nonce: challenge.nonce,
            })
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
        let mut request = format!("ws://{address}/runtime/desktop")
            .into_client_request()
            .unwrap();
        request
            .headers_mut()
            .insert("x-cma-runtime-token", access.parse().unwrap());
        request.headers_mut().insert(
            DESKTOP_OWNER_HEADER,
            serde_json::to_string(&owner).unwrap().parse().unwrap(),
        );
        if pending {
            let upgrade = tokio::spawn(tokio_tungstenite::connect_async(request));
            tokio::time::timeout(Duration::from_secs(2), handshake)
                .await
                .unwrap()
                .unwrap();
            drop(lifetime);
            let result = tokio::time::timeout(Duration::from_secs(2), upgrade)
                .await
                .unwrap()
                .unwrap();
            assert!(matches!(result,
                Err(tokio_tungstenite::tungstenite::Error::Http(response)) if response.status() == 503));
        } else {
            let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
            let message = TungsteniteMessage::Binary(vec![1, 2, 3]);
            socket.send(message.clone()).await.unwrap();
            assert_eq!(socket.next().await.unwrap().unwrap(), message);
            drop(lifetime);
            let closed = tokio::time::timeout(Duration::from_secs(2), socket.next())
                .await
                .unwrap();
            assert!(matches!(
                closed,
                None | Some(Err(_)) | Some(Ok(TungsteniteMessage::Close(_)))
            ));
        }
        // The desktop generation ended while ownership remains independently revocable.
        client
            .post(format!("{ownership}/revoke"))
            .header("x-cma-runtime-token", &access)
            .json(&owner)
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
        fixture.stop().await;
        for path in paths {
            if let Err(error) = std::fs::remove_dir_all(path) {
                assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
            }
        }
    }
}

mod child_fixture;
mod child_process_tests;
mod test_support;
