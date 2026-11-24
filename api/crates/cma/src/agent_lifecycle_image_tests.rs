use super::*;
use axum::{
    http::StatusCode,
    routing::{get, post},
    Router,
};

#[tokio::test]
async fn retirement_refuses_fresh_and_unversioned_selections() {
    let resolver = ImageManagerResolver::new("http://127.0.0.1:1".into(), true).unwrap();
    let image = AgentLifecycleImageReference {
        id: "image".into(),
        version: Some("published-version".into()),
    };
    assert!(matches!(
        resolver.resolve(None, &image).await,
        Err(AgentLifecycleProvisioningError::ImageSelectionUnsupported)
    ));
    let pinned = "registry.example/runtime@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    assert!(matches!(
        resolver.resolve_retained(None, &image, pinned).await,
        Err(AgentLifecycleProvisioningError::SignInRequired)
    ));
    let unversioned = AgentLifecycleImageReference {
        version: None,
        ..image
    };
    assert!(matches!(
        resolver
            .resolve_retained(Some("fixture-credential"), &unversioned, pinned)
            .await,
        Err(AgentLifecycleProvisioningError::ImageSelectionUnsupported)
    ));
}

#[tokio::test]
async fn image_authentication_and_authorization_never_invent_service_connection_targets() {
    for status in [StatusCode::UNAUTHORIZED, StatusCode::FORBIDDEN] {
        let app = Router::new()
            .route(
                "/v1/images/image/versions",
                get(move || async move { status }),
            )
            .route(
                "/v1/images/image/resolve",
                post(move || async move { status }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let resolver =
            ImageManagerResolver::new(format!("http://{}", listener.local_addr().unwrap()), false)
                .unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        for version in [None, Some("published-version".into())] {
            let image = AgentLifecycleImageReference {
                id: "image".into(),
                version,
            };
            assert!(matches!(
                resolver.resolve(None, &image).await,
                Err(AgentLifecycleProvisioningError::SignInRequired)
            ));
            let error = resolver
                .resolve(Some("fixture-credential"), &image)
                .await
                .unwrap_err();
            match status {
                StatusCode::UNAUTHORIZED => assert!(matches!(
                    error,
                    AgentLifecycleProvisioningError::SignInRequired
                )),
                StatusCode::FORBIDDEN => assert!(matches!(
                    error,
                    AgentLifecycleProvisioningError::ImageAccessDenied
                )),
                _ => unreachable!(),
            }
        }
        server.abort();
    }
}
