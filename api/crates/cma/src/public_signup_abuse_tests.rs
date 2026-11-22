//! Synthetic bursts qualify existing admission and Default-tier enforcement.
//! This does not establish a dedicated signup quota or identity-provider protection.

mod support;

use std::sync::{atomic::Ordering, Arc};

use axum::http::{header, StatusCode};
use layer2_core::SubjectId;
use mongodb::bson::{doc, Document};

use crate::auth::persistence::{AuthStore as _, SecretDigest};
use support::*;

const AUTH_PATHS: [&str; 3] = [
    "/auth/login?screen=register",
    "/auth/login",
    "/auth/callback",
];

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn hundred_auth_requests_share_the_configured_default_budget_across_replicas() {
    let fixture = Fixture::new().await;
    let apps = [
        fixture.replica(config(1000, 20), CLOCK).await,
        fixture.replica(config(1000, 20), CLOCK).await,
    ];
    let barrier = Arc::new(tokio::sync::Barrier::new(100));
    let mut tasks = Vec::new();
    for index in 0..100 {
        let app = apps[index % 2].clone();
        let barrier = barrier.clone();
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            (index % 3, call(&app, AUTH_PATHS[index % 3], "", 1).await)
        }));
    }
    let mut accepted = 0;
    let mut refused = 0;
    for task in tasks {
        let (path, response) = task.await.unwrap();
        if response.status() == StatusCode::TOO_MANY_REQUESTS {
            assert_limited(response).await;
            refused += 1;
        } else {
            assert_eq!(
                response.status(),
                if path == 2 {
                    StatusCode::BAD_REQUEST
                } else {
                    StatusCode::SEE_OTHER
                }
            );
            accepted += 1;
        }
    }
    assert_eq!((accepted, refused), (20, 80));
    fixture.assert_no_sign_in_effects();
    assert!(fixture
        .store
        .subject(&SubjectId::new(SUBJECT))
        .await
        .unwrap()
        .is_none());
    let rows = fixture
        .database
        .collection::<Document>(crate::store::request_limits::COLLECTION);
    assert_eq!(
        rows.count_documents(doc! { "_id": { "$regex": "^default:" } })
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        rows.count_documents(doc! { "_id": { "$regex": "^create:" } })
            .await
            .unwrap(),
        0
    );

    let restarted = fixture.replica(config(1000, 20), CLOCK).await;
    for path in AUTH_PATHS {
        assert_limited(call(&restarted, path, "", 1).await).await;
    }
    assert_eq!(
        call(&restarted, AUTH_PATHS[0], "", 2).await.status(),
        StatusCode::SEE_OTHER
    );
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn admission_exhaustion_refuses_the_remaining_burst_before_auth_store_access() {
    let fixture = Fixture::new().await;
    let app = fixture.replica(config(2, 600), CLOCK).await;
    for path in &AUTH_PATHS[..2] {
        assert_eq!(
            call(&app, path, "", 1).await.status(),
            StatusCode::SEE_OTHER
        );
    }
    fixture.store.set_unavailable(true);
    for index in 2..100 {
        assert_limited(call(&app, AUTH_PATHS[index % 3], "", 1).await).await;
    }
    fixture.store.set_unavailable(false);
    fixture.assert_no_sign_in_effects();
    assert!(fixture
        .store
        .subject(&SubjectId::new(SUBJECT))
        .await
        .unwrap()
        .is_none());
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn refused_valid_callback_preserves_attempt_and_creates_no_partial_owner() {
    let fixture = Fixture::new().await;
    // Prepare a valid browser-bound attempt before exhausting its network budget.
    let login = call(
        &crate::auth::router(fixture.auth.clone()),
        "/auth/login",
        "",
        1,
    )
    .await;
    assert_eq!(login.status(), StatusCode::SEE_OTHER);
    let authorize =
        reqwest::Url::parse(login.headers()[header::LOCATION].to_str().unwrap()).unwrap();
    let state = authorize
        .query_pairs()
        .find(|(key, _)| key == "state")
        .unwrap()
        .1
        .into_owned();
    let binding = login.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let callback = format!("/auth/callback?code=synthetic-code&state={state}");
    let app = fixture.replica(config(1000, 2), CLOCK).await;
    for path in &AUTH_PATHS[..2] {
        assert_eq!(
            call(&app, path, "", 1).await.status(),
            StatusCode::SEE_OTHER
        );
    }
    let other_replica = fixture.replica(config(1000, 2), CLOCK).await;
    assert_limited(call(&other_replica, &callback, &binding, 1).await).await;
    fixture.assert_no_sign_in_effects();
    assert!(fixture
        .store
        .subject(&SubjectId::new(SUBJECT))
        .await
        .unwrap()
        .is_none());
    let binding_id = SecretDigest::identify("browser-login", binding.split_once('=').unwrap().1);
    assert!(fixture
        .store
        .browser_login(&binding_id)
        .await
        .unwrap()
        .is_none());

    // Advance the existing database clock and recreate local limiters. The same
    // callback must still complete: refusal cannot consume its one-use state.
    let recovered = fixture.replica(config(1000, 2), CLOCK + 60_000).await;
    let response = call(&recovered, &callback, &binding, 1).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(fixture.observed.exchanges.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.observed.users.lock().unwrap().len(), 1);
    assert_eq!(fixture.observed.activities.lock().unwrap().len(), 1);
    let subject = fixture
        .store
        .subject(&SubjectId::new(SUBJECT))
        .await
        .unwrap()
        .unwrap();
    assert_eq!((subject.revision, subject.token_generation), (1, 1));
    assert!(subject.tokens.is_some());
    let cookie = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|value| value.to_str().unwrap())
        .find(|value| value.starts_with("cma_session="))
        .unwrap();
    let session = cookie.split(';').next().unwrap().split_once('=').unwrap().1;
    assert_eq!(
        fixture
            .store
            .browser_login(&SecretDigest::identify("browser-login", session))
            .await
            .unwrap()
            .unwrap()
            .subject,
        subject.subject
    );
    fixture.close().await;
}
