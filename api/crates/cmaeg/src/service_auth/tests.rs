use super::*;
use axum::{
    extract::State, http::StatusCode, response::IntoResponse, routing::post, Form, Json, Router,
};
use serde_json::json;
use std::{
    collections::HashMap,
    sync::atomic::{AtomicUsize, Ordering},
};

struct Fixture {
    calls: AtomicUsize,
    fail: AtomicUsize,
    secret: Secret,
    tokens: [Secret; 2],
}

async fn issue(
    State(state): State<Arc<Fixture>>,
    Form(form): Form<HashMap<String, String>>,
) -> axum::response::Response {
    assert_eq!(form["grant_type"], "client_credentials");
    assert_eq!(form["client_id"], "fixture-client");
    assert_eq!(form["client_secret"], state.secret.as_str());
    assert_eq!(form["scope"], "openid roles");
    let call = state.calls.fetch_add(1, Ordering::SeqCst);
    match state.fail.load(Ordering::SeqCst) {
        1 => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                state.secret.as_str().to_owned(),
            )
                .into_response()
        }
        2 => return (StatusCode::FOUND, [("location", "/credential-leak")]).into_response(),
        3 => {
            return Json(
                json!({"access_token":state.secret.as_str(),"token_type":"Bearer","expires_in":0}),
            )
            .into_response()
        }
        _ => {}
    }
    Json(json!({"access_token":state.tokens[call.min(1)].as_str(),"token_type":"Bearer","expires_in":3600})).into_response()
}

async fn fixture() -> (ServiceAuth, Arc<Fixture>, tokio::task::JoinHandle<()>) {
    let state = Arc::new(Fixture {
        calls: AtomicUsize::new(0),
        fail: AtomicUsize::new(0),
        secret: Secret::new(uuid::Uuid::new_v4().to_string()),
        tokens: std::array::from_fn(|_| Secret::new(uuid::Uuid::new_v4().to_string())),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new()
        .route("/oauth/token", post(issue))
        .with_state(state.clone());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let auth = ServiceAuth::account(
        &origin,
        crate::ServiceAccountConfig {
            client_id: "fixture-client".into(),
            client_secret: state.secret.clone(),
        },
    )
    .unwrap();
    (auth, state, task)
}

#[tokio::test]
async fn concurrent_readers_share_tokens_and_refresh_without_restarting() {
    let (auth, state, task) = fixture().await;
    let mut workers = tokio::task::JoinSet::new();
    for _ in 0..24 {
        let auth = auth.clone();
        workers.spawn(async move { auth.bearer().await.unwrap() });
    }
    while let Some(token) = workers.join_next().await {
        assert_eq!(token.unwrap().as_str(), state.tokens[0].as_str());
    }
    assert_eq!(state.calls.load(Ordering::SeqCst), 1);
    let ServiceAuth::Account(account) = &auth else {
        panic!()
    };
    account
        .cache
        .lock()
        .await
        .token
        .as_mut()
        .unwrap()
        .refresh_at = Instant::now();
    assert_eq!(
        auth.bearer().await.unwrap().as_str(),
        state.tokens[1].as_str()
    );
    assert_eq!(state.calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        auth.bearer().await.unwrap().as_str(),
        state.tokens[1].as_str()
    );
    assert_eq!(state.calls.load(Ordering::SeqCst), 2);
    task.abort();
}

#[tokio::test]
async fn refresh_failure_uses_valid_token_then_refuses_expired_token_without_storms_or_leaks() {
    let (auth, state, task) = fixture().await;
    auth.bearer().await.unwrap();
    state.fail.store(1, Ordering::SeqCst);
    let ServiceAuth::Account(account) = &auth else {
        panic!()
    };
    account
        .cache
        .lock()
        .await
        .token
        .as_mut()
        .unwrap()
        .refresh_at = Instant::now();
    assert_eq!(
        auth.bearer().await.unwrap().as_str(),
        state.tokens[0].as_str()
    );
    assert_eq!(state.calls.load(Ordering::SeqCst), 2);
    account
        .cache
        .lock()
        .await
        .token
        .as_mut()
        .unwrap()
        .expires_at = Instant::now();
    for _ in 0..10 {
        assert!(auth.bearer().await.is_err());
    }
    assert_eq!(state.calls.load(Ordering::SeqCst), 2);
    assert!(!format!("{auth:?}").contains(state.secret.as_str()));
    assert!(!format!("{auth:?}").contains(state.tokens[0].as_str()));
    assert!(!auth
        .bearer()
        .await
        .unwrap_err()
        .to_string()
        .contains(state.secret.as_str()));
    task.abort();
}

#[tokio::test]
async fn redirects_and_invalid_token_expiry_are_refused() {
    for failure in [2, 3] {
        let (auth, state, task) = fixture().await;
        state.fail.store(failure, Ordering::SeqCst);
        assert!(auth.bearer().await.is_err());
        assert_eq!(state.calls.load(Ordering::SeqCst), 1);
        task.abort();
    }
}
