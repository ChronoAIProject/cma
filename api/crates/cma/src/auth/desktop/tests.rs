//! Two independent AuthState/client instances prove shared revocation.
use super::*;
use crate::auth::{
    desktop_test_support::DesktopAuthFixture as Fixture,
    persistence::{AuthStore, SecretValue},
    profile::Profile,
};
use axum::http::header;
use layer2_core::Authorization;
use std::time::Duration;

mod boundaries;
mod support;

async fn authority(f: &Fixture, auth: &Arc<AuthState>) -> Arc<dyn DesktopAuthorization> {
    let resolved = auth.resolve_request(&f.headers).await.unwrap().unwrap();
    capture(auth.clone(), &f.headers, &resolved).unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn browser_stream_authority_revokes_across_instances_and_restart() {
    let f = Fixture::new(SubjectId::new("desktop-owner")).await;
    let left = authority(&f, &f.left).await;
    let right = authority(&f, &f.right).await;
    left.current(&f.caller).await.unwrap();
    right.current(&f.caller).await.unwrap();
    f.revoke_login().await;
    assert!(matches!(
        left.current(&f.caller).await,
        Err(DesktopError::Unavailable)
    ));
    assert!(matches!(
        right.current(&f.caller).await,
        Err(DesktopError::Unavailable)
    ));
    let restarted = AuthState::with_store(f.left.fixture_mode().clone(), None, f.stores[0].clone());
    assert!(restarted
        .resolve_request(&f.headers)
        .await
        .unwrap()
        .is_none());
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn logout_during_provider_verification_cannot_renew_stream_authority() {
    let f = Fixture::new(SubjectId::new("desktop-owner")).await;
    let authority = authority(&f, &f.left).await;
    authority.current(&f.caller).await.unwrap();
    f.block_userinfo();
    let caller = f.caller.clone();
    let pending = tokio::spawn(async move { authority.current(&caller).await });
    f.wait_for_userinfo().await;
    f.revoke_login().await;
    f.release_userinfo();
    assert!(matches!(
        pending.await.unwrap(),
        Err(DesktopError::Unavailable)
    ));
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn replaced_browser_credentials_and_live_bearer_refusal_fail_closed() {
    let f = Fixture::new(SubjectId::new("desktop-owner")).await;
    let authority = authority(&f, &f.left).await;
    authority.current(&f.caller).await.unwrap();
    let old = f.stores[1]
        .subject(&f.caller.subject)
        .await
        .unwrap()
        .unwrap();
    let mut next = old.clone();
    next.revision += 1;
    next.token_generation += 1;
    next.tokens.as_mut().unwrap().access_token = SecretValue::new(uuid::Uuid::new_v4().to_string());
    assert!(f.stores[1]
        .replace_subject(&old.incarnation, old.revision, next)
        .await
        .unwrap());
    assert!(authority.current(&f.caller).await.is_err());
    let mut headers = HeaderMap::new();
    headers.insert(
        header::AUTHORIZATION,
        format!("Bearer {}", uuid::Uuid::new_v4()).parse().unwrap(),
    );
    let left = f.left.resolve_request(&headers).await.unwrap().unwrap();
    let right = f.right.resolve_request(&headers).await.unwrap().unwrap();
    let left = capture(f.left.clone(), &headers, &left).unwrap();
    let right = capture(f.right.clone(), &headers, &right).unwrap();
    left.current(&f.caller).await.unwrap();
    right.current(&f.caller).await.unwrap();
    f.deny_provider(true);
    assert!(left.current(&f.caller).await.is_err());
    assert!(right.current(&f.caller).await.is_err());
    f.cleanup().await;
}
