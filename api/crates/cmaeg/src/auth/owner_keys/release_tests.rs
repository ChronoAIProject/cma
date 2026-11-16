//! Recovery authority is checked independently of ordinary POST/create rights.
use super::tests::OwnerFixture;
use crate::auth::{Creator, CreatorAuthState, CreatorAuthenticator, PartitionReleaseActor};
use axum::{
    extract::FromRequestParts,
    http::{Method, Request, StatusCode},
    response::IntoResponse,
};
use serde_json::json;

struct State(CreatorAuthenticator);
impl CreatorAuthState for State {
    fn creator_auth(&self) -> &CreatorAuthenticator {
        &self.0
    }
}

#[tokio::test]
async fn eg2171_release_requires_explicit_grant_and_rejects_forged_owner() {
    let fixture = OwnerFixture::start("owner").await;
    let state = State(fixture.auth.clone());
    for (permissions, expected) in [
        (vec!["cma-eg:channels:read"], StatusCode::FORBIDDEN),
        (vec!["cma-eg:channels:create"], StatusCode::FORBIDDEN),
        (vec!["cma-eg:channels:update"], StatusCode::FORBIDDEN),
        (vec!["cma-eg:partitions:release"], StatusCode::OK),
        (vec!["cma-eg:admin"], StatusCode::OK),
    ] {
        fixture.claims.lock().unwrap()["permissions"] = json!(permissions);
        let (mut parts, _) = Request::builder()
            .method(Method::POST)
            .uri("/v1/channels/channel/partitions/partition/release")
            .body(())
            .unwrap()
            .into_parts();
        parts.headers = fixture.headers.clone();
        let actual = match PartitionReleaseActor::from_request_parts(&mut parts, &state).await {
            Ok(actor) => {
                assert_eq!(actor.actor, "owner");
                assert_eq!(actor.creator.subject, "owner");
                StatusCode::OK
            }
            Err(error) => error.into_response().status(),
        };
        assert_eq!(actual, expected, "release grant: {permissions:?}");
    }
    let (mut parts, _) = Request::builder()
        .method(Method::POST)
        .uri("/release")
        .body(())
        .unwrap()
        .into_parts();
    parts.headers = fixture.headers.clone();
    parts
        .headers
        .insert("x-cmaeg-owner", "other-owner".parse().unwrap());
    assert!(
        PartitionReleaseActor::from_request_parts(&mut parts, &state)
            .await
            .is_err()
    );
    fixture.claims.lock().unwrap()["permissions"] = json!(["cma-eg:channels:read"]);
    let (mut parts, _) = Request::builder()
        .method(Method::GET)
        .uri("/v1/channels/channel/partitions/partition/head")
        .body(())
        .unwrap()
        .into_parts();
    parts.headers = fixture.headers.clone();
    assert!(Creator::from_request_parts(&mut parts, &state)
        .await
        .is_ok());
}

#[tokio::test]
async fn eg2171_release_does_not_fall_back_to_legacy_identity_only_authentication() {
    let state = State(CreatorAuthenticator::new("http://127.0.0.1:1").unwrap());
    let (mut parts, _) = Request::builder()
        .method(Method::POST)
        .uri("/release")
        .header("authorization", "Bearer local-test-not-a-credential")
        .body(())
        .unwrap()
        .into_parts();
    assert!(matches!(
        PartitionReleaseActor::from_request_parts(&mut parts, &state).await,
        Err(crate::auth::CreatorAuthError::Forbidden)
    ));
}
