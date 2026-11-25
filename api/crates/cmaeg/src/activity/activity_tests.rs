use super::*;
use crate::{
    activity::{Change, ConsumerState, ObservedActivity},
    auth::{Creator, CreatorAuthenticator},
    channels::management::ManagementState,
    ingress::{
        launch::{LaunchOutcome, LaunchRequest, TurnLauncher},
        routing_plan::fixture,
        routing_store::test_server::{Row, Server},
    },
    providers::{ProviderClient, ProviderRegistry},
    turns::observer::{NoopObserver, ObservedLauncher},
};
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
};
use cmaeg_core::seal::{seal_blob, ChannelPointer};
use futures_util::{future::BoxFuture, StreamExt};
use std::{
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    time::Duration,
};

#[path = "execution_tests.rs"]
mod execution;
#[path = "expiry_reporting_tests.rs"]
mod expiry_reporting;
#[path = "history_http_tests.rs"]
mod history_http;
#[path = "owner_keys_tests.rs"]
mod owner_keys;
#[path = "privacy_tests.rs"]
mod privacy;

struct AuthFixture {
    task: tokio::task::JoinHandle<()>,
    allowed: Arc<AtomicBool>,
}
impl Drop for AuthFixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
fn auth_headers() -> HeaderMap {
    HeaderMap::from_iter([(
        "authorization".parse().unwrap(),
        "Bearer activity-fixture".parse().unwrap(),
    )])
}
async fn live_auth(state: &mut ManagementState, creator: &mut Creator) -> AuthFixture {
    use axum::{response::IntoResponse, routing::get, Json, Router};
    let allowed = Arc::new(AtomicBool::new(true));
    let checked = allowed.clone();
    let subject = creator.subject.clone();
    let app = Router::new().route(
        "/api/v1/users/me",
        get(move || {
            let allowed = checked.clone();
            let subject = subject.clone();
            async move {
                if !allowed.load(Ordering::SeqCst) {
                    return StatusCode::UNAUTHORIZED.into_response();
                }
                Json(serde_json::json!({"subject":subject,"user_type":"person"})).into_response()
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    state.creator_auth = Arc::new(
        CreatorAuthenticator::new(&format!("http://{}", listener.local_addr().unwrap())).unwrap(),
    );
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    *creator = state
        .creator_auth
        .authenticate(&auth_headers())
        .await
        .unwrap();
    AuthFixture { task, allowed }
}

async fn fixture_app() -> (Server, ManagementState, LaunchRequest, Creator) {
    let server = Server::start_realtime().await;
    let mut request = fixture::request(
        |value| value["record_id"] = uuid::Uuid::new_v4().to_string().into(),
        fixture::message("not copied into activity"),
    );
    let mut channel = (*request.channel).clone();
    let url = server
        .records
        .record_url(channel.definition.record_id.as_str())
        .unwrap();
    channel.pointer = ChannelPointer {
        v: 1,
        record_id: channel.definition.record_id.clone(),
        record_url: url.to_string(),
    };
    channel.id = channel.pointer.seal_id(&fixture::keys()).unwrap();
    let creator = Creator {
        subject: channel.definition.owner.subject.clone(),
        token_hash: "fixture".into(),
        organization: false,
    };
    server.memory.channels.lock().unwrap().insert(
        channel.definition.record_id.to_string(),
        Row {
            owner: creator.subject.clone(),
            blob: seal_blob(
                &fixture::keys(),
                &serde_json::to_vec(&channel.definition.to_blob_json()).unwrap(),
            )
            .unwrap(),
            expiry: time::OffsetDateTime::now_utc().unix_timestamp() + 3600,
            version: 1,
        },
    );
    request.channel = Arc::new(channel);
    let registry=ProviderRegistry::parse(r#"[{"slug":"cma_codex","base_url":"http://127.0.0.1:9","kind":"responses_http","paths":{"agent_card":"/card","binding":"/bindings/{binding_id}","conversation":"/bindings/{binding_id}/conversations/{conversation_id}","responses":"/responses"},"max_inflight":8}]"#).unwrap();
    let state = ManagementState::new(
        Arc::new(
            ProviderClient::new(registry, Duration::from_secs(1), Duration::from_secs(1), 8)
                .unwrap(),
        ),
        Arc::new(server.records.clone()),
        Arc::new(CreatorAuthenticator::new("http://127.0.0.1:9").unwrap()),
        crate::ManagementConfig {
            console_public_url: None,
            master_keys: Arc::new(fixture::keys()),
            public_url: "http://127.0.0.1:8090".into(),
            public_api_url: "http://127.0.0.1:8090".into(),
            public_api_aliases: vec![],
            advertise_public_api_url: true,
            cma_base_url: url.origin().ascii_serialization(),
            tools_nyxid_slug: "cmaeg".into(),
            cma_service_token: cmaeg_core::channel::Secret::new("activity-fixture"),
            nyxid_base_url: "http://127.0.0.1:9".into(),
        },
        crate::ChannelCacheConfig::default(),
    );
    (server, state, request, creator)
}

#[tokio::test]
async fn receive_is_visible_durably_before_provider_launch_and_survives_restart() {
    let (server, state, request, creator) = fixture_app().await;
    struct Launcher {
        store: Arc<ActivityStore>,
        calls: Arc<AtomicUsize>,
    }
    impl TurnLauncher for Launcher {
        fn launch(&self, request: LaunchRequest) -> BoxFuture<'_, LaunchOutcome> {
            Box::pin(async move {
                let page = self
                    .store
                    .latest(
                        &request.channel.definition.owner.subject,
                        request.channel.definition.record_id.as_str(),
                    )
                    .await
                    .unwrap();
                assert_eq!(
                    page.items.len(),
                    1,
                    "receive must commit before provider launch"
                );
                self.calls.fetch_add(1, Ordering::SeqCst);
                LaunchOutcome::Accepted {
                    response_id: "response-activity".into(),
                }
            })
        }
    }
    let record = request.channel.definition.record_id.to_string();
    let calls = Arc::new(AtomicUsize::new(0));
    let launcher = ObservedLauncher::new(
        Arc::new(Launcher {
            store: state.activity.clone(),
            calls: calls.clone(),
        }),
        Arc::new(ObservedActivity::new(
            state.activity.clone(),
            Arc::new(NoopObserver),
        )),
    );
    assert!(matches!(
        launcher.launch(request).await,
        LaunchOutcome::Accepted { .. }
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let restarted = ActivityStore::new(server.records.clone(), Arc::new(fixture::keys()));
    let page = restarted.latest(&creator.subject, &record).await.unwrap();
    assert_eq!(page.items[0].consumers[0].state, ConsumerState::Accepted);
    assert_eq!(
        page.items[0].consumers[0].response_id.as_deref(),
        Some("response-activity")
    );
    assert!(!serde_json::to_string(&page)
        .unwrap()
        .contains("not copied into activity"));
    let recovery = server.store().recover_all_page(None, 32).await.unwrap();
    assert!(
        recovery.plans.is_empty() && recovery.issues.is_empty(),
        "activity records must be ignored by retained route recovery"
    );
}

#[tokio::test]
async fn failed_receive_persistence_never_calls_provider() {
    let (server, state, request, creator) = fixture_app().await;
    struct Never;
    impl TurnLauncher for Never {
        fn launch(&self, _: LaunchRequest) -> BoxFuture<'_, LaunchOutcome> {
            panic!("failed persistence must not launch")
        }
    }
    let id = identifier(
        &creator.subject,
        request.channel.definition.record_id.as_str(),
        today(),
    );
    server.fault(id, false, false);
    let launcher = ObservedLauncher::new(
        Arc::new(Never),
        Arc::new(ObservedActivity::new(
            state.activity.clone(),
            Arc::new(NoopObserver),
        )),
    );
    assert!(matches!(
        launcher.launch(request).await,
        LaunchOutcome::Refused(_)
    ));
}

#[tokio::test]
async fn previous_day_duplicate_keeps_first_receipt_and_one_row() {
    let (_server, state, request, creator) = fixture_app().await;
    let record = request.channel.definition.record_id.as_str();
    state.activity.received(&request).await.unwrap();
    let (mut bucket, _) = state
        .activity
        .read(&creator.subject, record, today())
        .await
        .unwrap()
        .unwrap();
    let mut original = bucket.items.remove(0);
    original.received_at = "2026-10-26T23:59:59Z".into();
    state
        .activity
        .modify(&creator.subject, record, today(), |b| b.items.clear())
        .await
        .unwrap();
    state
        .activity
        .modify(&creator.subject, record, today() - 1, |b| {
            b.items = vec![original.clone()]
        })
        .await
        .unwrap();
    state.activity.received(&request).await.unwrap();
    let page = state
        .activity
        .latest(&creator.subject, record)
        .await
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].received_at, original.received_at);
}

#[tokio::test]
async fn public_read_and_stream_enforce_owner_and_push_progress_without_client_polling() {
    let (_server, mut state, request, mut creator) = fixture_app().await;
    let _auth = live_auth(&mut state, &mut creator).await;
    let raw = request.channel.id.to_string();
    let mut other = creator.clone();
    other.subject = "another-owner".into();
    assert_eq!(
        crate::http::activity::latest(State(state.clone()), other.clone(), Path(raw.clone()))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        crate::http::activity::stream(
            State(state.clone()),
            other,
            Path(raw.clone()),
            Query(Default::default()),
            HeaderMap::new()
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    state.activity.received(&request).await.unwrap();
    let response = crate::http::activity::stream(
        State(state.clone()),
        creator.clone(),
        Path(raw),
        Query(Default::default()),
        auth_headers(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let mut stream = response.into_body().into_data_stream();
    let first = tokio::time::timeout(Duration::from_secs(1), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let first = std::str::from_utf8(&first).unwrap();
    assert!(first.contains("event: activity") && first.contains("event-1"));
    state
        .activity
        .change(
            request.channel.definition.record_id.as_str(),
            request.activity.event_id.as_str(),
            &Change::state(
                "default",
                ConsumerState::Produced,
                Some("response-live".into()),
            ),
        )
        .await
        .unwrap();
    let next = tokio::time::timeout(Duration::from_secs(1), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(std::str::from_utf8(&next)
        .unwrap()
        .contains("response-live"));
}

#[tokio::test]
async fn open_stream_observes_another_gateway_without_a_local_notification() {
    let (server, mut state, request, mut creator) = fixture_app().await;
    let _auth = live_auth(&mut state, &mut creator).await;
    let record = request.channel.definition.record_id.as_str();
    let other_gateway = ActivityStore::new(server.records.clone(), Arc::new(fixture::keys()));
    let response = crate::http::activity::stream(
        State(state.clone()),
        creator,
        Path(request.channel.id.to_string()),
        Query(Default::default()),
        auth_headers(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let mut stream = response.into_body().into_data_stream();
    stream.next().await.unwrap().unwrap();
    // The independent store models a replacement pod. Its notification channel
    // has no connection to the retained stream's original Gateway process.
    other_gateway.received(&request).await.unwrap();
    let received = tokio::time::timeout(Duration::from_secs(2), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(std::str::from_utf8(&received).unwrap().contains("event-1"));
    other_gateway
        .change(
            record,
            request.activity.event_id.as_str(),
            &Change::state(
                "default",
                ConsumerState::Produced,
                Some("remote-response".into()),
            ),
        )
        .await
        .unwrap();
    let produced = tokio::time::timeout(Duration::from_secs(2), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(std::str::from_utf8(&produced)
        .unwrap()
        .contains("remote-response"));
    // Consistency checks do not emit duplicate Activity frames when unchanged.
    assert!(
        tokio::time::timeout(Duration::from_millis(1100), stream.next())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn cursor_is_channel_bound_and_stale_reconnect_requests_reset() {
    let (_server, state, request, creator) = fixture_app().await;
    let record = request.channel.definition.record_id.as_str();
    let page = state
        .activity
        .latest(&creator.subject, record)
        .await
        .unwrap();
    assert!(!state.activity.cursor_reset(record, &page.cursor).unwrap());
    assert!(state
        .activity
        .cursor_reset("other-channel", &page.cursor)
        .is_err());
    let mut parsed: Cursor =
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(&page.cursor).unwrap()).unwrap();
    parsed.day -= 8;
    let stale = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&parsed).unwrap());
    let response = crate::http::activity::stream(
        State(state),
        creator,
        Path(request.channel.id.to_string()),
        Query(crate::http::activity::ActivityQuery {
            cursor: Some(stale),
        }),
        HeaderMap::new(),
    )
    .await;
    let mut stream = response.into_body().into_data_stream();
    let frame = stream.next().await.unwrap().unwrap();
    assert!(std::str::from_utf8(&frame)
        .unwrap()
        .contains("event: reset"));
}

#[tokio::test]
async fn revoked_identity_ends_open_stream_before_next_snapshot() {
    let (_server, mut state, request, mut creator) = fixture_app().await;
    let auth = live_auth(&mut state, &mut creator).await;
    state.activity.received(&request).await.unwrap();
    let response = crate::http::activity::stream(
        State(state.clone()),
        creator,
        Path(request.channel.id.to_string()),
        Query(Default::default()),
        auth_headers(),
    )
    .await;
    let mut stream = response.into_body().into_data_stream();
    stream.next().await.unwrap().unwrap();
    auth.allowed.store(false, Ordering::SeqCst);
    // The public authenticator has an existing 60-second credential cache.
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(61)).await;
    tokio::time::resume();
    state
        .activity
        .change(
            request.channel.definition.record_id.as_str(),
            request.activity.event_id.as_str(),
            &Change::state("default", ConsumerState::Accepted, None),
        )
        .await
        .unwrap();
    // A keep-alive due during clock advancement carries no activity data.
    let ended = tokio::time::timeout(Duration::from_secs(1), async {
        while let Some(frame) = stream.next().await {
            let frame = frame.unwrap();
            assert!(!String::from_utf8_lossy(&frame).contains("event: activity"));
        }
    })
    .await;
    assert!(ended.is_ok());
}

#[tokio::test]
async fn activity_is_bounded_and_explicit_about_truncation() {
    let (server, state, mut request, creator) = fixture_app().await;
    let record = request.channel.definition.record_id.to_string();
    for index in 0..55 {
        request.activity.event_id = fixture::event_id(&format!("event-{index:03}"));
        state.activity.received(&request).await.unwrap();
    }
    let page = state
        .activity
        .latest(&creator.subject, &record)
        .await
        .unwrap();
    assert_eq!(page.items.len(), 50);
    assert!(page.has_more);
    assert_eq!(page.retention_days, 7);
    assert!(server
        .memory
        .rows
        .lock()
        .unwrap()
        .values()
        .all(|r| r.blob.len() <= 65536));
    assert!(state
        .activity
        .latest("other-owner", &record)
        .await
        .unwrap()
        .items
        .is_empty());
}
