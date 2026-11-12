#[path = "launch_telemetry_tests.rs"]
mod telemetry_tests;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use axum::{
    body::{to_bytes, Body},
    http::{HeaderValue, Request, StatusCode},
    response::Response,
};
use futures_util::StreamExt;
use layer2_core::trigger_domain::{
    Launch, LaunchAdmission, LaunchAdmissionResult, LaunchAuthorityClass, LaunchCommit,
    LaunchEvent, LaunchEventKind, LaunchPage, LaunchPageRequest, LaunchPlacementStats,
    LaunchResolution, LaunchSeed, LaunchStore, LaunchStoreError, LaunchTrustManifest,
    PublicTriggerId, RequirementsSnapshot, ReviewRequirements, TriggerId, MAX_LAUNCH_EVENT_LIMIT,
};
use layer2_core::{LaunchId, SubjectId};
use tower::ServiceExt;

use super::{launch_companion_router, LaunchCompanionState};
use crate::{rest::LaunchEventView, AuthenticatedSubject};

const OWNER: &str = "owner";
const OTHER: &str = "other";
const LAUNCH_ID: &str = "lch_one";
const NOW: &str = "2026-09-05T00:00:00Z";

#[derive(Default)]
struct StoreState {
    launch: Option<Launch>,
    events: Vec<LaunchEvent>,
    launch_reads: usize,
    event_reads: Vec<(u64, u16)>,
    fail_launch: bool,
    fail_events: bool,
}

#[derive(Clone)]
struct TestLaunchStore {
    state: Arc<Mutex<StoreState>>,
}

impl TestLaunchStore {
    fn new(kinds: impl IntoIterator<Item = LaunchEventKind>) -> Self {
        let store = Self {
            state: Arc::new(Mutex::new(StoreState {
                launch: Some(test_launch()),
                ..StoreState::default()
            })),
        };
        for kind in kinds {
            store.append(kind);
        }
        store
    }

    fn append(&self, kind: LaunchEventKind) -> LaunchEvent {
        let mut state = self.state.lock().unwrap();
        let launch = state.launch.as_mut().unwrap();
        launch.event_head += 1;
        let event = LaunchEvent {
            launch_id: launch.id.clone(),
            seq: launch.event_head,
            appended_at: NOW.into(),
            kind,
        };
        state.events.push(event.clone());
        event
    }

    fn events(&self) -> Vec<LaunchEvent> {
        self.state.lock().unwrap().events.clone()
    }

    fn reads(&self) -> (usize, Vec<(u64, u16)>) {
        let state = self.state.lock().unwrap();
        (state.launch_reads, state.event_reads.clone())
    }

    fn hide(&self) {
        self.state.lock().unwrap().launch = None;
    }

    fn transfer(&self) {
        self.state.lock().unwrap().launch.as_mut().unwrap().owner = SubjectId::new(OTHER);
    }

    fn fail_launches(&self) {
        self.state.lock().unwrap().fail_launch = true;
    }

    fn fail_event_reads(&self) {
        self.state.lock().unwrap().fail_events = true;
    }
}

#[async_trait]
impl LaunchStore for TestLaunchStore {
    async fn admit_launch(
        &self,
        _: LaunchAdmission,
    ) -> Result<LaunchAdmissionResult, LaunchStoreError> {
        unreachable!("the companion does not admit Launches")
    }

    async fn launch_by_command(
        &self,
        _: &SubjectId,
        _: &PublicTriggerId,
        _: &str,
    ) -> Result<Option<Launch>, LaunchStoreError> {
        unreachable!("the companion does not replay commands")
    }

    async fn launch(
        &self,
        id: &LaunchId,
        owner: &SubjectId,
    ) -> Result<Option<Launch>, LaunchStoreError> {
        let mut state = self.state.lock().unwrap();
        state.launch_reads += 1;
        if state.fail_launch {
            return Err(LaunchStoreError::Unavailable("test".into()));
        }
        Ok(state
            .launch
            .as_ref()
            .filter(|launch| &launch.id == id && &launch.owner == owner)
            .cloned())
    }

    async fn list_launches(
        &self,
        _: &SubjectId,
        _: &LaunchPageRequest,
    ) -> Result<LaunchPage, LaunchStoreError> {
        unreachable!("the companion does not list Launches")
    }

    async fn placement_stats(
        &self,
        _: &TriggerId,
    ) -> Result<Vec<LaunchPlacementStats>, LaunchStoreError> {
        unreachable!("the companion does not read placement statistics")
    }

    async fn commit_launch(&self, _: LaunchCommit) -> Result<Launch, LaunchStoreError> {
        unreachable!("the companion does not commit Launches")
    }

    async fn launch_events(
        &self,
        id: &LaunchId,
        owner: &SubjectId,
        after: u64,
        limit: u16,
    ) -> Result<Vec<LaunchEvent>, LaunchStoreError> {
        let mut state = self.state.lock().unwrap();
        state.event_reads.push((after, limit));
        if state.fail_events {
            return Err(LaunchStoreError::Unavailable("test".into()));
        }
        let launch = state
            .launch
            .as_ref()
            .filter(|launch| &launch.id == id && &launch.owner == owner)
            .ok_or(LaunchStoreError::NotFound)?;
        if after > launch.event_head || limit == 0 || limit > MAX_LAUNCH_EVENT_LIMIT {
            return Err(LaunchStoreError::InvalidCursor);
        }
        Ok(state
            .events
            .iter()
            .filter(|event| event.seq > after)
            .take(usize::from(limit))
            .cloned()
            .collect())
    }

    async fn due_launches(&self, _: &str, _: u16) -> Result<Vec<LaunchId>, LaunchStoreError> {
        unreachable!("the companion does not claim Launch work")
    }

    async fn claim_launch(
        &self,
        _: &LaunchId,
        _: &str,
        _: String,
        _: String,
    ) -> Result<Option<Launch>, LaunchStoreError> {
        unreachable!("the companion does not claim Launch work")
    }
}

fn test_launch() -> Launch {
    let mut launch = Launch::new(LaunchSeed {
        trigger_id: TriggerId("trg_one".into()),
        revision_digest: "a".repeat(64),
        public_id: PublicTriggerId("pub_one".into()),
        owner: SubjectId::new(OWNER),
        authority: LaunchAuthorityClass::SandboxCreator,
        placement_id: None,
        referrer_origin: None,
        requirements: RequirementsSnapshot {
            services: vec![],
            checked_at: NOW.into(),
        },
        review: ReviewRequirements {
            needs: vec![],
            can_start_now: true,
        },
        trust: LaunchTrustManifest::none(),
        resolution: LaunchResolution::pending(),
        has_first_message: false,
        command_key_hash: "b".repeat(64),
        command_fingerprint: "c".repeat(64),
        created_at: NOW.into(),
    })
    .unwrap();
    launch.id = LaunchId(LAUNCH_ID.into());
    launch
}

fn request(path: &str, owner: &str) -> Request<Body> {
    let mut request = Request::get(path).body(Body::empty()).unwrap();
    request
        .extensions_mut()
        .insert(AuthenticatedSubject(SubjectId::new(owner)));
    request
}

async fn response(store: &TestLaunchStore, request: Request<Body>) -> Response {
    launch_companion_router(LaunchCompanionState::new(Arc::new(store.clone())))
        .oneshot(request)
        .await
        .unwrap()
}

async fn finite_body(store: &TestLaunchStore, path: &str) -> (StatusCode, Vec<u8>) {
    let response = response(store, request(path, OWNER)).await;
    let status = response.status();
    let body = to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap()
        .to_vec();
    (status, body)
}

fn frames(bytes: &[u8]) -> Vec<(String, String)> {
    std::str::from_utf8(bytes)
        .unwrap()
        .split("\n\n")
        .filter(|frame| !frame.is_empty() && !frame.starts_with(':'))
        .map(|frame| {
            let mut id = None;
            let mut data = None;
            for line in frame.lines() {
                if let Some(value) = line.strip_prefix("id: ") {
                    id = Some(value.to_owned());
                }
                if let Some(value) = line.strip_prefix("data: ") {
                    data = Some(value.to_owned());
                }
            }
            (id.unwrap(), data.unwrap())
        })
        .collect()
}

#[tokio::test]
async fn owner_replay_emits_one_exact_shared_dto_per_frame() {
    let store = TestLaunchStore::new([
        LaunchEventKind::Created,
        LaunchEventKind::ConfirmationRecorded,
        LaunchEventKind::Completed,
    ]);
    let expected = store
        .events()
        .into_iter()
        .map(|event| serde_json::to_string(&LaunchEventView::from(event)).unwrap())
        .collect::<Vec<_>>();
    let response = response(
        &store,
        request("/api/v1/agui/launches/lch_one/events?follow=false", OWNER),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let frames = frames(&body);
    assert_eq!(frames.len(), 3);
    assert_eq!(
        frames
            .iter()
            .map(|frame| frame.0.as_str())
            .collect::<Vec<_>>(),
        ["1", "2", "3"]
    );
    assert_eq!(
        frames.into_iter().map(|frame| frame.1).collect::<Vec<_>>(),
        expected
    );
}

#[tokio::test]
async fn query_cursors_resume_and_after_is_an_alias() {
    for cursor in ["starting_after", "after"] {
        let store = TestLaunchStore::new([
            LaunchEventKind::Created,
            LaunchEventKind::ConfirmationRecorded,
            LaunchEventKind::Completed,
        ]);
        let path = format!("/api/v1/agui/launches/lch_one/events?{cursor}=1&follow=false");
        let (status, body) = finite_body(&store, &path).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            frames(&body)
                .iter()
                .map(|frame| frame.0.as_str())
                .collect::<Vec<_>>(),
            ["2", "3"]
        );
        assert_eq!(store.reads().1, [(1, 100)]);
    }
}

#[tokio::test]
async fn valid_last_event_id_takes_precedence_over_query_cursor() {
    let store = TestLaunchStore::new([
        LaunchEventKind::Created,
        LaunchEventKind::ConfirmationRecorded,
        LaunchEventKind::Completed,
    ]);
    let mut request = request(
        "/api/v1/agui/launches/lch_one/events?starting_after=0&follow=false",
        OWNER,
    );
    request
        .headers_mut()
        .insert("last-event-id", HeaderValue::from_static("2"));
    let response = response(&store, request).await;
    let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    assert_eq!(frames(&body)[0].0, "3");
    assert_eq!(store.reads().1, [(2, 100)]);
}

#[tokio::test]
async fn duplicate_malformed_and_unknown_inputs_are_rejected_after_owner_read() {
    let invalid_queries = [
        "starting_after=0&after=1",
        "after=1&after=2",
        "starting_after=01",
        "starting_after=-1",
        "starting_after=18446744073709551616",
        "starting_after=%",
        "limit=0",
        "limit=201",
        "limit=1&limit=2",
        "follow=True",
        "follow=false&follow=true",
        "unknown=1",
    ];
    for query in invalid_queries {
        let store = TestLaunchStore::new([]);
        let path = format!("/api/v1/agui/launches/lch_one/events?{query}");
        let (status, body) = finite_body(&store, &path).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{query}");
        let problem: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(problem["code"], "invalid_launch_event_query", "{query}");
        assert_eq!(store.reads(), (1, vec![]), "{query}");
    }
}

#[tokio::test]
async fn duplicate_and_malformed_last_event_ids_are_rejected() {
    let store = TestLaunchStore::new([]);
    let mut duplicate = request("/api/v1/agui/launches/lch_one/events", OWNER);
    duplicate
        .headers_mut()
        .append("last-event-id", HeaderValue::from_static("0"));
    duplicate
        .headers_mut()
        .append("last-event-id", HeaderValue::from_static("1"));
    assert_eq!(
        response(&store, duplicate).await.status(),
        StatusCode::BAD_REQUEST
    );

    let mut malformed = request("/api/v1/agui/launches/lch_one/events", OWNER);
    malformed
        .headers_mut()
        .insert("last-event-id", HeaderValue::from_bytes(&[0xff]).unwrap());
    assert_eq!(
        response(&store, malformed).await.status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(store.reads(), (2, vec![]));
}

#[tokio::test]
async fn cursor_beyond_end_reports_the_current_head() {
    let store = TestLaunchStore::new([LaunchEventKind::Created, LaunchEventKind::Completed]);
    let (status, body) = finite_body(
        &store,
        "/api/v1/agui/launches/lch_one/events?starting_after=3",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let problem: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(problem["code"], "cursor_beyond_end");
    assert_eq!(problem["head"], 2);
    assert!(store.reads().1.is_empty());
}

#[tokio::test]
async fn follow_false_reads_one_bounded_page_and_closes() {
    let store = TestLaunchStore::new([
        LaunchEventKind::Created,
        LaunchEventKind::ConfirmationRecorded,
        LaunchEventKind::Completed,
    ]);
    let (status, body) = finite_body(
        &store,
        "/api/v1/agui/launches/lch_one/events?limit=2&follow=false",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(frames(&body).len(), 2);
    assert_eq!(store.reads(), (1, vec![(0, 2)]));
}

#[tokio::test]
async fn follow_defaults_true_and_observes_a_later_durable_event() {
    let store = TestLaunchStore::new([]);
    let response = response(
        &store,
        request("/api/v1/agui/launches/lch_one/events", OWNER),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body().into_data_stream();
    let event = store.append(LaunchEventKind::Created);
    let bytes = tokio::time::timeout(Duration::from_secs(1), body.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let frames = frames(&bytes);
    assert_eq!(frames[0].0, "1");
    assert_eq!(
        frames[0].1,
        serde_json::to_string(&LaunchEventView::from(event)).unwrap()
    );
}

enum LaterFailure {
    Missing,
    OwnershipLost,
    LaunchRead,
    EventRead,
}

#[tokio::test]
async fn failures_after_streaming_starts_terminate_silently() {
    for failure in [
        LaterFailure::Missing,
        LaterFailure::OwnershipLost,
        LaterFailure::LaunchRead,
        LaterFailure::EventRead,
    ] {
        let store = TestLaunchStore::new([LaunchEventKind::Created]);
        let response = response(
            &store,
            request("/api/v1/agui/launches/lch_one/events", OWNER),
        )
        .await;
        let mut body = response.into_body().into_data_stream();
        let first = body.next().await.unwrap().unwrap();
        assert_eq!(frames(&first)[0].0, "1");
        match failure {
            LaterFailure::Missing => store.hide(),
            LaterFailure::OwnershipLost => store.transfer(),
            LaterFailure::LaunchRead => store.fail_launches(),
            LaterFailure::EventRead => store.fail_event_reads(),
        }
        let end = tokio::time::timeout(Duration::from_secs(1), body.next())
            .await
            .unwrap();
        assert!(end.is_none());
    }
}

#[tokio::test]
async fn foreign_and_missing_launches_are_concealed_identically() {
    let store = TestLaunchStore::new([LaunchEventKind::Created]);
    let foreign = response(
        &store,
        request("/api/v1/agui/launches/lch_one/events?after=bad", OTHER),
    )
    .await;
    let missing = response(
        &store,
        request("/api/v1/agui/launches/lch_missing/events?after=bad", OTHER),
    )
    .await;
    assert_eq!(foreign.status(), StatusCode::NOT_FOUND);
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    assert_eq!(foreign.headers(), missing.headers());
    let foreign = to_bytes(foreign.into_body(), 64 * 1024).await.unwrap();
    let missing = to_bytes(missing.into_body(), 64 * 1024).await.unwrap();
    assert_eq!(foreign, missing);
}

#[test]
fn shared_view_is_wire_owned_but_byte_equivalent_to_the_domain_event() {
    let domain = LaunchEvent {
        launch_id: LaunchId(LAUNCH_ID.into()),
        seq: 7,
        appended_at: NOW.into(),
        kind: LaunchEventKind::RequirementChanged {
            service: layer2_core::ServiceId::new("github"),
            state: layer2_core::trigger_domain::ServiceRequirementState::Missing,
        },
    };
    let domain_json = serde_json::to_vec(&domain).unwrap();
    let view = LaunchEventView::from(domain);
    assert_eq!(view.launch_id, LAUNCH_ID);
    assert_eq!(serde_json::to_vec(&view).unwrap(), domain_json);
}

#[tokio::test]
async fn launch_sse_rechecks_standing_before_each_buffered_frame() {
    use crate::rest::shared_stream_tests::ChangingStanding;
    for deleting in [OWNER, "unrelated", "outage"] {
        let store = TestLaunchStore::new([LaunchEventKind::Created, LaunchEventKind::Completed]);
        let standing = Arc::new(ChangingStanding::default());
        let router = launch_companion_router(
            LaunchCompanionState::new(Arc::new(store)).with_account_standing(standing.clone()),
        );
        let response = router
            .oneshot(request(
                "/api/v1/agui/launches/lch_one/events?follow=false",
                OWNER,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let mut body = response.into_body().into_data_stream();
        assert_eq!(frames(&body.next().await.unwrap().unwrap())[0].0, "1");
        *standing.0.lock().unwrap() = Some(deleting);
        let next = body.next().await;
        if deleting == "unrelated" {
            assert_eq!(frames(&next.unwrap().unwrap())[0].0, "2");
        } else {
            assert!(
                next.is_none(),
                "a buffered launch event escaped after {deleting}"
            );
        }
    }
}
