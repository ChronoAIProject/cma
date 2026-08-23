//! End-to-end tests for recording a request.
//!
//! Driven over a real socket, against a real router carrying both middleware,
//! because the properties worth proving only exist once everything is wired
//! together: that the handler still receives a body this read on the way past,
//! that the caller is attributed, and — the one that matters most — that a
//! request full of a caller's secrets produces an entry containing none of
//! them.

use std::sync::Arc;

use axum::{
    response::IntoResponse,
    routing::{post, put},
    Router,
};
use layer2_core::{
    store::{ActivityStore, StoreError},
    ActivityAction, ActivityOutcome, ActivityQuery, ActivityRecord, ActivityService, DetailKey,
    SubjectId,
};
use tokio::sync::Mutex;

use super::ActivityRecorder;
use crate::auth::{AuthMode, AuthState};
use layer1_public::{ActedUpon, SettingsTouched};

/// A store held in memory, so what was recorded can be read back exactly.
#[derive(Default)]
struct FakeActivityStore {
    entries: Mutex<Vec<ActivityRecord>>,
}

#[async_trait::async_trait]
impl ActivityStore for FakeActivityStore {
    async fn append(&self, record: &ActivityRecord) -> Result<(), StoreError> {
        self.entries.lock().await.push(record.clone());
        Ok(())
    }

    async fn recent(&self, _query: &ActivityQuery) -> Result<Vec<ActivityRecord>, StoreError> {
        Ok(self.entries.lock().await.clone())
    }
}

/// What the handler was actually handed, so the body can be proved intact.
#[derive(Default)]
struct SeenBodies(Mutex<Vec<String>>);

/// Serve a router carrying both middleware, and answer one request against it.
///
/// A real socket rather than a service harness: the ordering of the two layers
/// is the thing being relied on, and a test that bypassed the server would not
/// exercise it.
async fn call(
    store: Arc<FakeActivityStore>,
    seen: Arc<SeenBodies>,
    path: &str,
    body: &'static str,
) -> reqwest::StatusCode {
    let activity = ActivityService::new(Arc::clone(&store) as Arc<dyn ActivityStore>);
    // Trusted mode attributes every request to one fixed subject, which is
    // exactly what a test needs: a caller with an identity and no provider.
    let auth = AuthState::new(
        AuthMode::Trusted {
            subject: "local-developer".into(),
        },
        None,
    );

    let app = Router::new()
        .route(
            "/api/v1/environment-profiles",
            post({
                let seen = Arc::clone(&seen);
                move |body: String| async move {
                    seen.0.lock().await.push(body);
                    axum::http::StatusCode::CREATED
                }
            }),
        )
        .layer(axum::middleware::from_fn_with_state(
            ActivityRecorder::new(activity),
            super::record,
        ))
        .layer(axum::middleware::from_fn_with_state(
            Arc::clone(&auth),
            crate::auth::attach_subject,
        ));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a port is available");
    let addr = listener.local_addr().expect("the socket has an address");
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let status = reqwest::Client::new()
        .post(format!("http://{addr}{path}"))
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body)
        .send()
        .await
        .expect("the request reaches the server")
        .status();
    server.abort();
    status
}

#[tokio::test]
async fn a_request_carrying_a_secret_produces_an_entry_that_does_not_contain_it() {
    // The rule the whole design exists for, proved against a real request. A
    // bundle's variable values are where a caller is invited to put a
    // credential, and none of it may reach the record.
    // A value with no entropy and no meaning, so this fixture cannot be
    // mistaken for a real credential by anything reading the repository — the
    // property under test is that it does not travel, not what it is.
    const PLANTED: &str = "not-a-real-value-at-all";
    const BODY: &str = r#"{"name":"deploy settings",
        "variables":[{"key":"PRIVATE_SETTING","value":"not-a-real-value-at-all"}],
        "packages":[],"skills":[]}"#;

    let store = Arc::new(FakeActivityStore::default());
    let seen = Arc::new(SeenBodies::default());
    let status = call(
        Arc::clone(&store),
        Arc::clone(&seen),
        "/api/v1/environment-profiles",
        BODY,
    )
    .await;
    assert_eq!(status, reqwest::StatusCode::CREATED);

    let entries = store.entries.lock().await.clone();
    assert_eq!(entries.len(), 1, "one request, one entry");
    let entry = &entries[0];

    // Checked against the serialised entry, since that is what is written down
    // and handed back: anything hiding in any field shows up here.
    let rendered = serde_json::to_string(entry).expect("serialises");
    assert!(
        !rendered.contains(PLANTED),
        "a value the caller may treat as a credential reached the record: {rendered}"
    );
    assert!(
        !rendered.contains("PRIVATE_SETTING"),
        "even the names of a caller's variables stay out: {rendered}"
    );

    // What was kept instead: enough to say what happened, and nothing more.
    assert_eq!(entry.action, ActivityAction::EnvironmentProfileCreate);
    assert_eq!(entry.outcome, ActivityOutcome::Succeeded);
    assert_eq!(
        entry.actor.as_ref().map(|s| s.as_str()),
        Some("local-developer"),
        "the caller must be attributed, or the entry answers nothing"
    );
    assert_eq!(
        entry.detail.get(DetailKey::ResourceName),
        Some("deploy settings")
    );
    assert_eq!(entry.detail.get(DetailKey::VariableCount), Some("1"));
}

#[tokio::test]
async fn the_handler_still_receives_the_body_this_read_on_the_way_past() {
    // Buffering the body to record its shape must give it back untouched, or
    // recording a request would break serving it.
    const BODY: &str = r#"{"name":"build tools","variables":[],"packages":[],"skills":[]}"#;

    let store = Arc::new(FakeActivityStore::default());
    let seen = Arc::new(SeenBodies::default());
    call(
        Arc::clone(&store),
        Arc::clone(&seen),
        "/api/v1/environment-profiles",
        BODY,
    )
    .await;

    let bodies = seen.0.lock().await.clone();
    assert_eq!(bodies.len(), 1);
    assert_eq!(bodies[0], BODY, "the handler was handed a different body");
}

#[tokio::test]
async fn a_request_to_a_path_this_service_does_not_serve_records_nothing() {
    // A path that serves nothing did nothing. Recording every scan of the
    // internet would bury the entries that matter.
    let store = Arc::new(FakeActivityStore::default());
    let seen = Arc::new(SeenBodies::default());
    let status = call(
        Arc::clone(&store),
        Arc::clone(&seen),
        "/not-a-route-this-service-serves",
        "{}",
    )
    .await;

    assert_eq!(status, reqwest::StatusCode::NOT_FOUND);
    assert!(store.entries.lock().await.is_empty());
}

/// Drive an operator control route whose handler names the session's owner.
///
/// Stands in for the real one at exactly the point that matters: the owner is
/// known inside the handler and nowhere else, so the entry can only carry it
/// if it survives the trip out on the response.
async fn call_control(store: Arc<FakeActivityStore>, owner: Option<&'static str>) {
    let activity = ActivityService::new(Arc::clone(&store) as Arc<dyn ActivityStore>);
    let auth = AuthState::new(
        AuthMode::Trusted {
            subject: "local-developer".into(),
        },
        None,
    );

    let app = Router::new()
        .route(
            "/api/v1/admin/sessions/{id}/terminate",
            post(move || async move {
                let mut response = axum::http::StatusCode::NO_CONTENT.into_response();
                if let Some(owner) = owner {
                    response
                        .extensions_mut()
                        .insert(ActedUpon(SubjectId(owner.into())));
                }
                response
            }),
        )
        .layer(axum::middleware::from_fn_with_state(
            ActivityRecorder::new(activity),
            super::record,
        ))
        .layer(axum::middleware::from_fn_with_state(
            Arc::clone(&auth),
            crate::auth::attach_subject,
        ));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a port is available");
    let addr = listener.local_addr().expect("the socket has an address");
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    reqwest::Client::new()
        .post(format!(
            "http://{addr}/api/v1/admin/sessions/sesn_1/terminate"
        ))
        .send()
        .await
        .expect("the request reaches the server");
    server.abort();
}

#[tokio::test]
async fn ending_somebody_elses_session_is_recorded_as_somebody_elses() {
    // The entry an operator's action has to leave behind. Action and target
    // are identical whoever asked, so without the subject the record cannot
    // answer the one question it is read for — and recovering it afterwards
    // would mean joining against a session that terminating has just removed.
    let store = Arc::new(FakeActivityStore::default());
    call_control(Arc::clone(&store), Some("alice")).await;

    let entries = store.entries.lock().await.clone();
    assert_eq!(entries.len(), 1, "one request, one entry");
    let entry = &entries[0];
    assert_eq!(entry.action, ActivityAction::SessionTerminate);
    assert_eq!(
        entry.actor.as_ref().map(|s| s.as_str()),
        Some("local-developer")
    );
    assert_eq!(
        entry.detail.get(DetailKey::Subject),
        Some("alice"),
        "an operator's action must name whose work it ended"
    );
}

#[tokio::test]
async fn ending_your_own_session_names_nobody_else() {
    // The other half: the subject is present only when it says something. A
    // key that always appeared, holding the actor again, would make the two
    // cases look alike from the other direction.
    let store = Arc::new(FakeActivityStore::default());
    call_control(Arc::clone(&store), None).await;

    let entries = store.entries.lock().await.clone();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].detail.get(DetailKey::Subject), None);
}

/// Drive a settings write and carry only its handler-derived audit facts out.
async fn call_settings(
    store: Arc<FakeActivityStore>,
    touched: Option<SettingsTouched>,
    status: axum::http::StatusCode,
    body: &'static str,
) -> String {
    let activity = ActivityService::new(Arc::clone(&store) as Arc<dyn ActivityStore>);
    let auth = AuthState::new(
        AuthMode::Trusted {
            subject: "local-developer".into(),
        },
        None,
    );
    let seen = Arc::new(SeenBodies::default());

    let app = Router::new()
        .route(
            "/api/v1/admin/settings",
            put({
                let seen = Arc::clone(&seen);
                move |body: String| {
                    let seen = Arc::clone(&seen);
                    let touched = touched.clone();
                    async move {
                        seen.0.lock().await.push(body);
                        let mut response = status.into_response();
                        if let Some(touched) = touched {
                            response.extensions_mut().insert(touched);
                        }
                        response
                    }
                }
            }),
        )
        .layer(axum::middleware::from_fn_with_state(
            ActivityRecorder::new(activity),
            super::record,
        ))
        .layer(axum::middleware::from_fn_with_state(
            Arc::clone(&auth),
            crate::auth::attach_subject,
        ));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a port is available");
    let addr = listener.local_addr().expect("the socket has an address");
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    reqwest::Client::new()
        .put(format!("http://{addr}/api/v1/admin/settings"))
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body)
        .send()
        .await
        .expect("the request reaches the server");
    server.abort();

    let received = seen.0.lock().await.first().cloned().unwrap_or_default();
    received
}

#[tokio::test]
async fn a_settings_write_records_names_and_card_but_no_values_or_prose() {
    const BODY: &str = r#"{"expected_revision":4,"settings":{"briefing":{"before":["private administrator wording"]},"pricing":{"cards":[{"micros":45000}]}}}"#;
    let store = Arc::new(FakeActivityStore::default());
    let seen = call_settings(
        Arc::clone(&store),
        Some(SettingsTouched {
            changed: vec!["briefing", "pricing"],
            rate_card: Some("rate_new".into()),
        }),
        axum::http::StatusCode::OK,
        BODY,
    )
    .await;

    assert_eq!(seen, BODY, "the handler must receive the body untouched");
    let entries = store.entries.lock().await.clone();
    assert_eq!(entries.len(), 1);
    let entry = &entries[0];
    assert_eq!(entry.action, ActivityAction::PlatformSettingsWrite);
    assert_eq!(entry.outcome, ActivityOutcome::Succeeded);
    assert_eq!(
        entry.detail.get(DetailKey::Setting),
        Some("briefing,pricing")
    );
    assert_eq!(entry.detail.get(DetailKey::RateCard), Some("rate_new"));
    let rendered = serde_json::to_string(entry).expect("serialises");
    assert!(!rendered.contains("private administrator wording"));
    assert!(
        !rendered.contains("45000"),
        "a price reached the record: {rendered}"
    );
}

#[tokio::test]
async fn a_noop_settings_write_records_no_invented_detail() {
    let store = Arc::new(FakeActivityStore::default());
    call_settings(
        Arc::clone(&store),
        Some(SettingsTouched::default()),
        axum::http::StatusCode::OK,
        "{}",
    )
    .await;

    let entries = store.entries.lock().await.clone();
    assert_eq!(entries.len(), 1);
    assert!(entries[0].detail.is_empty());
}

#[tokio::test]
async fn a_refused_settings_write_is_recorded_as_rejected() {
    let store = Arc::new(FakeActivityStore::default());
    call_settings(
        Arc::clone(&store),
        None,
        axum::http::StatusCode::CONFLICT,
        "{}",
    )
    .await;

    let entries = store.entries.lock().await.clone();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].action, ActivityAction::PlatformSettingsWrite);
    assert_eq!(entries[0].outcome, ActivityOutcome::Rejected);
}
