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
async fn authenticated_control_drains_real_legacy_and_owned_websockets() {
    let home = std::env::temp_dir().join(format!("cma-desktop-http-{}", uuid::Uuid::new_v4()));
    let config = crate::config::RuntimeConfig::for_test(home.clone());
    let state = config.state_root.clone();
    let checkpoint = config.checkpoint_root.clone();
    let sandbox = config.sandbox_id.clone();
    let supervisor =
        Supervisor::new(config, crate::writer_fence::SupervisorBoundary::default()).unwrap();
    let display = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let display_url = format!("ws://{}/display", display.local_addr().unwrap());
    let display_server = tokio::spawn(async move {
        let mut streams = tokio::task::JoinSet::new();
        loop {
            let (stream, _) = display.accept().await.unwrap();
            streams.spawn(async move {
                let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
                while let Some(Ok(message)) = socket.next().await {
                    if message.is_close() {
                        break;
                    }
                    if socket.send(message).await.is_err() {
                        break;
                    }
                }
            });
            while streams.try_join_next().is_some() {}
        }
    });
    let token = uuid::Uuid::new_v4().simple().to_string().repeat(2);
    let auth = crate::control_auth::ControlAuth::from_token(token.clone()).unwrap();
    let receiver = supervisor.desktop_ownership.clone();
    let app = Router::new()
        .route(
            "/runtime/desktop",
            get(
                move |ws: WebSocketUpgrade, headers: axum::http::HeaderMap| {
                    upgrade(
                        ws,
                        receiver.clone(),
                        "display-test".into(),
                        display_url.clone(),
                        headers,
                    )
                },
            ),
        )
        .route(
            "/runtime/desktop/ownership/challenge",
            post(crate::link::desktop_control::challenge),
        )
        .route(
            "/runtime/desktop/ownership/confirm",
            post(crate::link::desktop_control::confirm),
        )
        .route(
            "/runtime/desktop/ownership/revoke",
            post(crate::link::desktop_control::revoke),
        )
        .with_state(supervisor.clone())
        .layer(axum::middleware::from_fn_with_state(
            auth,
            crate::control_auth::authenticate,
        ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
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
    let (mut legacy, _) = tokio_tungstenite::connect_async(handshake.clone())
        .await
        .unwrap();
    legacy
        .send(TungsteniteMessage::Binary(vec![1]))
        .await
        .unwrap();
    assert_eq!(
        legacy.next().await.unwrap().unwrap(),
        TungsteniteMessage::Binary(vec![1])
    );
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
    let closed = tokio::time::timeout(Duration::from_secs(1), legacy.next())
        .await
        .expect("legacy drain must close the socket");
    assert!(matches!(
        closed,
        None | Some(Err(_)) | Some(Ok(TungsteniteMessage::Close(_)))
    ));
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
    server.abort();
    display_server.abort();
    let _ = server.await;
    let _ = display_server.await;
    drop(supervisor);
    for path in [home, state, checkpoint] {
        let _ = std::fs::remove_dir_all(path);
    }
}
