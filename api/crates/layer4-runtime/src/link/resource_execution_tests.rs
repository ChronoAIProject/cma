use super::*;
use axum::{body::Body, http::Request};
use layer3_adapter::owner_protocol::{OwnerConfirmation, TransportOwner, OWNER_HEADER};
use tower::ServiceExt;

#[tokio::test]
async fn resource_routes_require_real_authentication_and_an_explicit_current_owner() {
    let home = std::env::temp_dir().join(format!(
        "cma-resource-http-{}",
        uuid::Uuid::new_v4().simple()
    ));
    let config = crate::config::RuntimeConfig::for_test(home.clone());
    let state_root = config.state_root.clone();
    let supervisor =
        Supervisor::new(config, crate::writer_fence::SupervisorBoundary::default()).unwrap();
    let owner = TransportOwner {
        protocol: 1,
        agent_id: "agent".into(),
        instance_id: "manager".into(),
        epoch: 1,
    };
    let challenge = supervisor
        .manager_ownership
        .challenge(owner.clone())
        .await
        .unwrap();
    supervisor
        .manager_ownership
        .confirm(OwnerConfirmation {
            owner: owner.clone(),
            nonce: challenge.nonce,
        })
        .await
        .unwrap();
    let bare = crate::link::router(supervisor.clone());
    let token = uuid::Uuid::new_v4().simple().to_string().repeat(2);
    let authenticated = bare.clone().layer(axum::middleware::from_fn_with_state(
        crate::control_auth::ControlAuth::from_token(token.clone()).unwrap(),
        crate::control_auth::authenticate,
    ));
    let path = "/runtime/resource-execution/v1/agent/capabilities";
    for (app, authenticated_header, owner_header, expected) in [
        (bare.clone(), true, true, StatusCode::UNAUTHORIZED),
        (authenticated.clone(), false, true, StatusCode::UNAUTHORIZED),
        (authenticated.clone(), true, false, StatusCode::FORBIDDEN),
        (authenticated.clone(), true, true, StatusCode::OK),
    ] {
        let mut request = Request::post(path).header("content-type", "application/json");
        if authenticated_header {
            request = request.header("x-cma-runtime-token", &token);
        }
        if owner_header {
            request = request.header(OWNER_HEADER, owner.header().unwrap());
        }
        let response = app
            .oneshot(request.body(Body::from(r#"{"protocol":1}"#)).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
        if expected == StatusCode::OK {
            let body = to_bytes(response.into_body(), MAX_MESSAGE_BYTES)
                .await
                .unwrap();
            let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert!(value["capabilities"].is_null());
        }
    }
    let mut stale = owner.clone();
    stale.epoch += 1;
    let response = authenticated
        .oneshot(
            Request::post(path)
                .header("content-type", "application/json")
                .header("x-cma-runtime-token", token)
                .header(OWNER_HEADER, stale.header().unwrap())
                .body(Body::from(r#"{"protocol":1}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(!state_root.join("resource-execution-v1").exists());
    drop(supervisor);
    let _ = std::fs::remove_dir_all(home);
    let _ = std::fs::remove_dir_all(state_root);
}

#[tokio::test]
async fn bounded_stalled_start_bodies_leave_control_admission_available() {
    let home = std::env::temp_dir().join(format!("cma-resource-pressure-{}", uuid::Uuid::new_v4()));
    let config = crate::config::RuntimeConfig::for_test(home.clone());
    let state_root = config.state_root.clone();
    let supervisor =
        Supervisor::new(config, crate::writer_fence::SupervisorBoundary::default()).unwrap();
    let owner = TransportOwner {
        protocol: 1,
        agent_id: "agent".into(),
        instance_id: "manager".into(),
        epoch: 1,
    };
    let challenge = supervisor
        .manager_ownership
        .challenge(owner.clone())
        .await
        .unwrap();
    supervisor
        .manager_ownership
        .confirm(OwnerConfirmation {
            owner: owner.clone(),
            nonce: challenge.nonce,
        })
        .await
        .unwrap();
    let token = uuid::Uuid::new_v4().simple().to_string().repeat(2);
    let app = crate::link::router(supervisor.clone()).layer(axum::middleware::from_fn_with_state(
        crate::control_auth::ControlAuth::from_token(token.clone()).unwrap(),
        crate::control_auth::authenticate,
    ));
    let request = |action: &str, body: Body| {
        Request::post(format!("/runtime/resource-execution/v1/agent/{action}"))
            .header("x-cma-runtime-token", &token)
            .header(OWNER_HEADER, owner.header().unwrap())
            .body(body)
            .unwrap()
    };
    let mut waiters = Vec::new();
    for _ in 0..4 {
        let (polled, observed) = tokio::sync::oneshot::channel();
        let body = Body::from_stream(futures_util::stream::once(async move {
            let _ = polled.send(());
            std::future::pending::<Result<axum::body::Bytes, std::io::Error>>().await
        }));
        let app = app.clone();
        let request = request("start", body);
        waiters.push(tokio::spawn(
            async move { app.oneshot(request).await.unwrap() },
        ));
        observed.await.unwrap();
    }
    let response = app
        .clone()
        .oneshot(request("start", Body::from("{}")))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    let response = app
        .clone()
        .oneshot(request("capabilities", Body::from(r#"{"protocol":1}"#)))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    for waiter in waiters {
        waiter.abort();
        let _ = waiter.await;
    }
    assert_eq!(
        supervisor.resource_execution.admissions.available_permits(),
        4
    );
    let response = app
        .oneshot(request(
            "start",
            Body::from(vec![b' '; MAX_MESSAGE_BYTES + 1]),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        supervisor.resource_execution.admissions.available_permits(),
        4
    );
    assert!(!state_root.join("resource-execution-v1").exists());
    drop(supervisor);
    let _ = std::fs::remove_dir_all(home);
    let _ = std::fs::remove_dir_all(state_root);
}
