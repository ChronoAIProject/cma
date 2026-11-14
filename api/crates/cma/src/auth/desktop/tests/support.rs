//! The reusable fixture must exercise middleware and fresh current checks.
use super::*;
use axum::{
    body::Body,
    extract::Extension,
    http::{Request, StatusCode},
    middleware::from_fn_with_state,
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use layer2_core::resource_access::CallerContext;
use tower::ServiceExt;

const PATH: &str = "/api/v1/agents/agt_auth_fixture/desktop/ws";

async fn captured(
    context: Option<Extension<CallerContext>>,
    authority: Option<Extension<Arc<dyn DesktopAuthorization>>>,
) -> Response {
    let (Some(Extension(context)), Some(Extension(authority))) = (context, authority) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let mut response = StatusCode::OK.into_response();
    response.extensions_mut().insert(context.caller);
    response.extensions_mut().insert(authority);
    response
}

fn router(auth: Arc<AuthState>) -> Router {
    Router::new()
        .route(PATH, get(captured))
        .layer(from_fn_with_state(auth, crate::auth::attach_subject))
}

async fn request(app: Router, headers: &HeaderMap) -> Response {
    let mut request = Request::get(PATH).body(Body::empty()).unwrap();
    *request.headers_mut() = headers.clone();
    app.oneshot(request).await.unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn reusable_provider_fixture_preserves_ingress_and_current_verification() {
    let f = Fixture::new(SubjectId::new("resource-fixture-owner")).await;
    assert_eq!(f.provider_snapshot(), Default::default());
    assert!(!Arc::ptr_eq(&f.left, &f.right));
    let left = router(f.left.clone());
    let right = router(f.right.clone());
    let response = request(left, &f.headers).await;
    assert_eq!(response.status(), StatusCode::OK);
    let caller = response.extensions().get::<Caller>().unwrap();
    assert_eq!(caller.subject, f.caller.subject);
    let authority = response
        .extensions()
        .get::<Arc<dyn DesktopAuthorization>>()
        .unwrap();
    let ingress = f.provider_snapshot();
    assert_eq!(ingress.discovery.requests, 1);
    assert_eq!(ingress.userinfo.requests, 1);
    assert_eq!(ingress.introspection.requests, 1);
    assert_eq!(ingress.userinfo.timed, 0);
    assert_eq!(ingress.userinfo.elapsed_nanos, 0);
    authority.current(caller).await.unwrap();
    authority.current(caller).await.unwrap();
    let current = f.provider_snapshot();
    assert_eq!(current.userinfo.requests, ingress.userinfo.requests + 2);
    assert_eq!(
        current.introspection.requests,
        ingress.introspection.requests + 2
    );

    f.set_provider_timing(true);
    let other = request(right.clone(), &f.headers).await;
    assert_eq!(other.status(), StatusCode::OK);
    assert_eq!(
        other.extensions().get::<Caller>().unwrap().subject,
        caller.subject
    );
    let peer = other
        .extensions()
        .get::<Arc<dyn DesktopAuthorization>>()
        .unwrap();
    peer.current(caller).await.unwrap();
    let timed = f.provider_snapshot();
    assert_eq!(timed.discovery.requests, 2);
    assert_eq!(timed.discovery.timed, 1);
    assert_eq!(timed.userinfo.requests, current.userinfo.requests + 2);
    assert_eq!(
        timed.introspection.requests,
        current.introspection.requests + 2
    );
    assert_eq!(timed.userinfo.timed, 2);
    assert_eq!(timed.introspection.timed, 2);
    assert_eq!(timed.userinfo.completed, timed.userinfo.requests);
    assert_eq!(timed.introspection.completed, timed.introspection.requests);

    f.deny_provider(true);
    assert!(authority.current(caller).await.is_err());
    assert!(peer.current(caller).await.is_err());
    f.revoke_login().await;
    let revoked = f.provider_snapshot();
    assert!(authority.current(caller).await.is_err());
    assert!(peer.current(caller).await.is_err());
    assert_eq!(
        request(right, &f.headers).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(f.provider_snapshot(), revoked);
    f.cleanup().await;
}
