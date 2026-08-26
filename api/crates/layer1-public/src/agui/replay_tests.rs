//! What a client reading history is handed, and what it must be able to read.
//!
//! The defect these exist for was not in either half. The projection was
//! correct and the client's parser was correct; nothing checked that the
//! frames one produced were the frames the other reads, and the client was
//! pointed at a route that answers in the core's vocabulary. Every chat test
//! drove a mock whose frames were already AG-UI-shaped, so the suite proved
//! the surface renders AG-UI and never that the wire delivers it.
//!
//! So the tests here are about the seam: the query a client actually sends,
//! and the exact field names the frames carry.

use super::*;
use crate::agui::project::project;
use layer2_core::{Event, EventKind, SessionId};

#[test]
fn keepalive_interval_matches_the_documented_contract() {
    // `API_AGUI.md` §2 promises clients a comment at least every 15 seconds.
    assert_eq!(
        super::super::KEEPALIVE_INTERVAL,
        std::time::Duration::from_secs(15)
    );
}

fn logged(seq: u64, kind: EventKind) -> Event {
    Event {
        session_id: SessionId("sesn_1".into()),
        seq,
        kind,
        appended_at: String::new(),
    }
}

#[test]
fn the_cursor_is_read_under_the_name_a_client_sends() {
    // `wire.ts` sends `starting_after`; the REST route it used to be pointed
    // at reads `after`, so serde ignored it and every reconnect replayed the
    // whole log from the beginning. Both names are accepted here, and the one
    // the document names is the one that must work.
    let sent: Replay =
        serde_urlencoded::from_str("thread_id=sesn_1&starting_after=42").expect("parses");
    assert_eq!(sent.starting_after(), Some(42));

    let legacy: Replay = serde_urlencoded::from_str("thread_id=sesn_1&after=42").expect("parses");
    assert_eq!(
        legacy.starting_after(),
        Some(42),
        "a client written against the older spelling must not silently replay everything"
    );

    let none: Replay = serde_urlencoded::from_str("thread_id=sesn_1").expect("parses");
    assert_eq!(
        none.starting_after(),
        None,
        "absent means from the beginning"
    );
}

#[test]
fn a_replayed_frame_carries_the_field_names_the_client_reads() {
    // The contract that was broken. Each assertion below names a field the
    // client's parser switches on or reads; a rename here is a rename the
    // client cannot see, and the surface draws an unknown row instead.
    let projected = project(
        &logged(
            4,
            EventKind::MessageCommitted {
                content: "carried across".into(),
            },
        ),
        "sesn_1",
        "sesn_1",
    );
    let frame = serde_json::to_value(&projected.events[0]).expect("serialises");

    assert_eq!(
        frame["type"], "TEXT_MESSAGE_START",
        "the client switches on a top-level `type`; the core's own frames have none, \
         which is why reading them produced nothing but unknown rows"
    );
    assert!(frame.get("threadId").is_some(), "{frame}");
    assert!(frame.get("runId").is_some(), "{frame}");
    assert!(
        frame.get("seq").is_none(),
        "the sequence is deliberately not protocol: it rides as the SSE id, and a \
         client takes it from there. A client reading it off the body gets nothing, \
         which silently breaks both deduplication and the cursor it resumes on: {frame}"
    );
}

#[test]
fn history_and_the_live_stream_are_the_same_frames() {
    // Web_Style.md §18.1: "a turn drawn from history and a turn watched
    // arriving must produce the same result". They come from one projection,
    // and this is what keeps it that way.
    let event = logged(
        7,
        EventKind::ToolCallStarted {
            call_id: "t1".into(),
            title: "cargo build".into(),
            kind: "shell".into(),
            arguments: None,
        },
    );

    let live = project(&event, "run_9", "sesn_1");
    let replayed = project(&event, "sesn_1", "sesn_1");

    assert_eq!(
        live.events.iter().map(|e| e.kind).collect::<Vec<_>>(),
        replayed.events.iter().map(|e| e.kind).collect::<Vec<_>>(),
        "the same event must become the same kinds either way"
    );
    assert_eq!(
        serde_json::to_value(&live.events[0]).expect("json")["seq"],
        serde_json::to_value(&replayed.events[0]).expect("json")["seq"],
        "and carry the same cursor, which is what a client resumes on"
    );
}

#[test]
fn the_session_stream_is_where_a_client_watches_a_turn() {
    // Not an assertion about this function's body but about where the lane
    // has to live, recorded here because getting it wrong is invisible.
    //
    // A client posts a run and holds *this* stream — the session's, across
    // turns. It does not consume the run POST's response. So a live lane
    // offered only on the run stream reaches nothing: the transport would be
    // built, tested, and dead.
    //
    // The other reason is the one already written into this module: two
    // streams would make a client decide for itself how to interleave the
    // durable and the live, which is exactly the ordering question §18.1 of
    // `Web_Style.md` says must have one answer.
    let projected = super::super::live::project_live(
        &layer2_core::LiveFrame::MessageDelta("in flight".into()),
        "sesn_1",
        "sesn_1",
    )
    .expect("a message being typed has a shape");

    assert!(
        projected.cursor().is_none(),
        "and it must still carry no cursor here: this stream is the one a \
         client resumes on, so a live frame given an id would send the next \
         reconnect to a position the log has never held"
    );
}

#[test]
fn a_history_read_is_bounded_and_a_watch_is_not() {
    // The defect this exists for was not in either half of the route. Followed
    // is right for a client watching a session, and this stream is documented
    // as never ending on its own. But the reference client also read *history*
    // from it, with `await response.text()` — and a promise over a stream that
    // never closes can only settle by failing.
    //
    // The connection awaits that read before it delivers anything, so every
    // live frame queued behind it forever and the surface stayed empty however
    // well the rest of the path worked. One query parameter is the difference
    // between a subscription and a value.
    let watching: Replay = serde_urlencoded::from_str("thread_id=sesn_1").expect("parses");
    assert!(
        watching.follows(),
        "a client that says nothing wants the stream held open, as every existing caller does"
    );

    let reading: Replay =
        serde_urlencoded::from_str("thread_id=sesn_1&starting_after=4&follow=false")
            .expect("parses");
    assert!(!reading.follows());
    assert_eq!(
        reading.starting_after(),
        Some(4),
        "and it still resumes from where the caller had got to"
    );

    let explicit: Replay =
        serde_urlencoded::from_str("thread_id=sesn_1&follow=true").expect("parses");
    assert!(explicit.follows());
}

/// The projection batch a strict-greater-than replay read returns.
fn replay_after(events: &[Event], cursor: u64) -> Vec<crate::agui::project::AguiEvent> {
    events
        .iter()
        .filter(|event| event.seq > cursor)
        .flat_map(|event| project(event, "sesn_1", "sesn_1").events)
        .collect()
}

#[test]
fn a_drop_mid_projection_resumes_the_event_whole() {
    let log = vec![
        logged(6, EventKind::TurnStarted),
        logged(
            7,
            EventKind::MessageCommitted {
                content: "nothing after the drop is lost".into(),
            },
        ),
        logged(8, EventKind::TurnCompleted),
    ];
    let previous = project(&log[0], "sesn_1", "sesn_1");
    let message = project(&log[1], "sesn_1", "sesn_1");

    // The client received sequence 6 and then only message 7's START before
    // the connection dropped. START is deliberately unstamped, so the last id
    // it can hand back is still 6.
    let last_seen = previous
        .events
        .iter()
        .chain(message.events.iter().take(1))
        .filter_map(|frame| frame.seq)
        .next_back()
        .expect("sequence 6 was delivered");
    assert_eq!(last_seen, 6);

    let resumed = replay_after(&log, last_seen);
    assert_eq!(
        resumed.iter().map(|frame| frame.kind).collect::<Vec<_>>(),
        vec![
            "TEXT_MESSAGE_START",
            "TEXT_MESSAGE_CONTENT",
            "TEXT_MESSAGE_END",
            "RUN_FINISHED"
        ],
        "sequence 7 is replayed whole rather than from its lost tail"
    );
    assert_eq!(
        resumed.iter().map(|frame| frame.seq).collect::<Vec<_>>(),
        vec![None, None, Some(7), Some(8)]
    );

    let after_message = replay_after(&log, 7);
    assert_eq!(after_message.len(), 1);
    assert_eq!(after_message[0].kind, "RUN_FINISHED");
    assert_eq!(after_message[0].seq, Some(8));
}

mod route_resume {
    use std::{sync::Arc, time::Duration};

    use axum::{
        body::{to_bytes, Body, BodyDataStream},
        http::{header, Request, StatusCode},
        Extension, Router,
    };
    use futures_util::StreamExt;
    use layer2_core::{
        session::ProvisioningStage, AgentConfiguration, Authorization, Baseline, Capability,
        Deployment, DirectoryQuery, EnvironmentProfile, EnvironmentProfileId,
        EnvironmentProfileService, EnvironmentProfileSpec, EnvironmentProfileStore, Event,
        EventKind, KernelId, Page, PageRequest, PlatformSettingsService, RecoveryState,
        RegisteredUser, Reserved, ScreenFactory, Session, SessionCapabilities, SessionId,
        SessionListQuery, SessionMode, SessionPorts, SessionRecord, SessionService, SessionStatus,
        SessionStore, StopReason, StoreError, StoredEvent, SubjectId, TerminalGeometry,
        UserDirectory, UserDirectoryStore,
    };
    use serde::Deserialize;
    use serde_json::{json, Value};
    use tower::ServiceExt;

    use crate::{agui, auth::AuthenticatedSubject, rest::SessionsState, tickets::TerminalTickets};

    struct ReplayState {
        record: SessionRecord,
        events: Vec<Event>,
    }

    struct ReplayStore {
        state: tokio::sync::RwLock<ReplayState>,
    }

    impl ReplayStore {
        fn new(record: SessionRecord, events: Vec<Event>) -> Self {
            Self {
                state: tokio::sync::RwLock::new(ReplayState { record, events }),
            }
        }

        async fn append(&self, event: Event) {
            let mut state = self.state.write().await;
            assert_eq!(event.session_id, state.record.session.id);
            assert!(event.seq > state.record.session.last_seq);
            state.record.session.last_seq = event.seq;
            state.events.push(event);
        }

        async fn append_physical_only(&self, event: Event) {
            let mut state = self.state.write().await;
            assert_eq!(event.session_id, state.record.session.id);
            state.events.push(event);
        }

        async fn advertise_head(&self, head: u64) {
            self.state.write().await.record.session.last_seq = head;
        }
    }

    #[async_trait::async_trait]
    impl SessionStore for ReplayStore {
        async fn ping(&self) -> Result<(), StoreError> {
            Ok(())
        }

        async fn get(&self, id: &SessionId) -> Result<Option<SessionRecord>, StoreError> {
            let state = self.state.read().await;
            Ok((state.record.session.id == *id).then(|| state.record.clone()))
        }

        async fn list_for(
            &self,
            owner: &SubjectId,
            _query: &SessionListQuery,
        ) -> Result<Page<SessionRecord>, StoreError> {
            let state = self.state.read().await;
            Ok(Page {
                items: (&state.record.session.owner == owner)
                    .then(|| state.record.clone())
                    .into_iter()
                    .collect(),
                next: None,
            })
        }

        async fn reserved_capacity(&self) -> Result<Reserved, StoreError> {
            Ok(Reserved::default())
        }

        async fn list_all(
            &self,
            _query: &SessionListQuery,
        ) -> Result<Page<SessionRecord>, StoreError> {
            let state = self.state.read().await;
            Ok(Page {
                items: vec![state.record.clone()],
                next: None,
            })
        }

        async fn delete(&self, _id: &SessionId) -> Result<(), StoreError> {
            panic!("the replay fixture is read-only")
        }

        async fn append_events(&self, _events: &[Event]) -> Result<(), StoreError> {
            panic!("the replay fixture is read-only")
        }

        async fn commit_session_idempotent(
            &self,
            _write_id: &str,
            _record: &SessionRecord,
            _events: &[Event],
        ) -> Result<u64, StoreError> {
            panic!("the replay fixture is read-only")
        }

        async fn create_session_idempotent(
            &self,
            _write_id: &str,
            _record: &SessionRecord,
            _events: &[Event],
        ) -> Result<u64, StoreError> {
            panic!("the replay fixture is read-only")
        }

        async fn events_since(
            &self,
            id: &SessionId,
            after: u64,
        ) -> Result<Vec<StoredEvent>, StoreError> {
            let state = self.state.read().await;
            let mut events = state
                .events
                .iter()
                .filter(|event| event.session_id == *id && event.seq > after)
                .cloned()
                .map(StoredEvent::from)
                .collect::<Vec<_>>();
            events.sort_by_key(|event| event.seq);
            Ok(events)
        }
    }

    struct NoProfiles;

    #[async_trait::async_trait]
    impl EnvironmentProfileStore for NoProfiles {
        async fn upsert(&self, _profile: &EnvironmentProfile) -> Result<(), StoreError> {
            panic!("the replay fixture is read-only")
        }

        async fn replace_content(
            &self,
            _id: &EnvironmentProfileId,
            _content: &EnvironmentProfileSpec,
            _updated_at: &str,
        ) -> Result<Option<EnvironmentProfile>, StoreError> {
            panic!("the replay fixture is read-only")
        }

        async fn get(
            &self,
            _id: &EnvironmentProfileId,
        ) -> Result<Option<EnvironmentProfile>, StoreError> {
            Ok(None)
        }

        async fn list_for(
            &self,
            _owner: &SubjectId,
            _page: &PageRequest,
        ) -> Result<Page<EnvironmentProfile>, StoreError> {
            Ok(Page {
                items: Vec::new(),
                next: None,
            })
        }

        async fn set_preferred(
            &self,
            _owner: &SubjectId,
            _chosen: Option<&EnvironmentProfileId>,
        ) -> Result<(), StoreError> {
            panic!("the replay fixture is read-only")
        }

        async fn preferred_for(
            &self,
            _owner: &SubjectId,
        ) -> Result<Option<EnvironmentProfile>, StoreError> {
            Ok(None)
        }

        async fn delete(&self, _id: &EnvironmentProfileId) -> Result<(), StoreError> {
            panic!("the replay fixture is read-only")
        }
    }

    struct NoDirectory;

    #[async_trait::async_trait]
    impl UserDirectoryStore for NoDirectory {
        async fn record_sign_in(&self, _user: &RegisteredUser) -> Result<(), StoreError> {
            Ok(())
        }

        async fn recent(&self, _query: &DirectoryQuery) -> Result<Vec<RegisteredUser>, StoreError> {
            Ok(Vec::new())
        }

        async fn by_subjects(
            &self,
            _subjects: &[SubjectId],
        ) -> Result<Vec<RegisteredUser>, StoreError> {
            Ok(Vec::new())
        }
    }

    #[derive(Debug)]
    struct WireFrame {
        id: Option<u64>,
        body: Value,
    }

    fn parse_sse(wire: &str) -> Vec<WireFrame> {
        wire.split("\n\n")
            .filter_map(|frame| {
                let mut id = None;
                let mut data = Vec::new();
                for line in frame.lines() {
                    if let Some(value) = line.strip_prefix("id:") {
                        id = Some(
                            value
                                .trim()
                                .parse::<u64>()
                                .expect("the SSE id is a sequence"),
                        );
                    } else if let Some(value) = line.strip_prefix("data:") {
                        data.push(value.trim_start());
                    }
                }
                (!data.is_empty()).then(|| WireFrame {
                    id,
                    body: serde_json::from_str(&data.join("\n"))
                        .expect("the route emits JSON SSE data"),
                })
            })
            .collect()
    }

    async fn replay(app: &Router, starting_after: u64) -> Vec<WireFrame> {
        let response = app
            .clone()
            .oneshot(
                Request::get(format!(
                    "/api/v1/agui/events?thread_id=sesn_route_resume&starting_after={starting_after}&follow=false"
                ))
                .body(Body::empty())
                .expect("request builds"),
            )
            .await
            .expect("router answers");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE),
            Some(&header::HeaderValue::from_static("text/event-stream"))
        );
        let body = to_bytes(response.into_body(), 128 * 1024)
            .await
            .expect("the bounded replay is small");
        parse_sse(std::str::from_utf8(&body).expect("SSE is UTF-8"))
    }

    async fn next_followed_frame(body: &mut BodyDataStream) -> WireFrame {
        loop {
            let chunk = tokio::time::timeout(Duration::from_secs(2), body.next())
                .await
                .expect("the followed stream observes the next durable poll")
                .expect("the followed stream stays open")
                .expect("the followed event body is readable");
            let frames = parse_sse(std::str::from_utf8(&chunk).expect("SSE is UTF-8"));
            if frames.is_empty() {
                continue;
            }
            assert_eq!(
                frames.len(),
                1,
                "the test appends one durable event before each followed read"
            );
            return frames.into_iter().next().expect("one frame was asserted");
        }
    }

    fn durable(seq: u64, kind: EventKind) -> Event {
        Event {
            session_id: SessionId("sesn_route_resume".into()),
            seq,
            kind,
            appended_at: format!("2026-08-25T09:00:{seq:02}Z"),
        }
    }

    fn owner() -> SubjectId {
        SubjectId("subject-route-resume".into())
    }

    struct Fixture {
        app: Router,
        store: Arc<ReplayStore>,
    }

    fn fixture(subject: SubjectId, authorization: Authorization) -> Fixture {
        let events = vec![
            durable(6, EventKind::TurnStarted),
            durable(
                7,
                EventKind::MessageCommitted {
                    content: "nothing after the drop is lost".into(),
                },
            ),
            durable(8, EventKind::TurnCompleted),
        ];
        fixture_with(events, subject, authorization)
    }

    fn fixture_with(
        events: Vec<Event>,
        subject: SubjectId,
        authorization: Authorization,
    ) -> Fixture {
        let owner = owner();
        let last_seq = events.last().map_or(0, |event| event.seq);
        let record = SessionRecord::new(Session {
            options: Default::default(),
            id: SessionId("sesn_route_resume".into()),
            owner: owner.clone(),
            kernel: KernelId::new("test-kernel"),
            status: SessionStatus::Idle,
            recovery_state: RecoveryState::Idle,
            stop_reason: None,
            geometry: TerminalGeometry::default(),
            last_seq,
            environment_profile: None,
            runtime_version: None,
            rotations: 0,
            rotated_at: None,
            capabilities: SessionCapabilities {
                conversation: true,
                ..Default::default()
            },
            granted_services: Some(Vec::new()),
            granted_skills: Some(Vec::new()),
            pricing: None,
            created_at: "2026-08-25T08:00:00Z".into(),
        });
        let store = Arc::new(ReplayStore::new(record, events));
        let session_store: Arc<dyn SessionStore> = store.clone();
        let settings = Arc::new(PlatformSettingsService::unconfigured());
        let environments = EnvironmentProfileService::new(
            Arc::new(NoProfiles),
            Baseline::default(),
            None,
            None,
            AgentConfiguration::default(),
            settings,
        );
        let screen_factory: ScreenFactory =
            Arc::new(|_| panic!("a bounded history read must not start a runtime"));
        let sessions = SessionService::new(
            Default::default(),
            screen_factory,
            session_store,
            SessionPorts::default(),
            Arc::clone(&environments),
            Deployment::default(),
        );
        let state = SessionsState {
            sessions,
            users: UserDirectory::new(Arc::new(NoDirectory)),
            tickets: TerminalTickets::in_memory(),
            environments,
        };

        let app = agui::router(state)
            .layer(Extension(authorization))
            .layer(Extension(AuthenticatedSubject(subject)));
        Fixture { app, store }
    }

    fn app_with(events: Vec<Event>) -> Router {
        fixture_with(
            events,
            owner(),
            Authorization::from_capabilities([Capability::UseOwnSessions]),
        )
        .app
    }

    fn app() -> Router {
        fixture(
            owner(),
            Authorization::from_capabilities([Capability::UseOwnSessions]),
        )
        .app
    }

    fn fold_state(mut state: Value, frames: &[WireFrame]) -> Value {
        for frame in frames {
            assert_eq!(frame.body["type"], "STATE_DELTA", "{}", frame.body);
            let patch: json_patch::Patch = serde_json::from_value(frame.body["delta"].clone())
                .unwrap_or_else(|error| panic!("{} carried an invalid patch: {error}", frame.body));
            json_patch::patch(&mut state, &patch)
                .unwrap_or_else(|error| panic!("{} did not apply: {error}", frame.body));
        }
        state
    }

    #[tokio::test]
    async fn the_production_route_resumes_an_interrupted_projection_whole() {
        let app = app();
        let full = replay(&app, 0).await;
        assert_eq!(
            full.iter()
                .map(|frame| (frame.id, frame.body["type"].as_str().unwrap_or_default()))
                .collect::<Vec<_>>(),
            vec![
                (Some(6), "RUN_STARTED"),
                (None, "TEXT_MESSAGE_START"),
                (None, "TEXT_MESSAGE_CONTENT"),
                (Some(7), "TEXT_MESSAGE_END"),
                (Some(8), "RUN_FINISHED"),
            ]
        );

        // Simulate a transport that received sequence 6 and message 7's first
        // frame before dropping. The prefix has no id, so 6 is still the only
        // cursor the client can return to the production endpoint.
        let delivered_prefix = &full[..2];
        let last_seen = delivered_prefix
            .iter()
            .filter_map(|frame| frame.id)
            .next_back()
            .expect("the preceding event was numbered");
        assert_eq!(last_seen, 6);

        let resumed = replay(&app, last_seen).await;
        assert_eq!(
            resumed
                .iter()
                .map(|frame| (frame.id, frame.body["type"].as_str().unwrap_or_default()))
                .collect::<Vec<_>>(),
            vec![
                (None, "TEXT_MESSAGE_START"),
                (None, "TEXT_MESSAGE_CONTENT"),
                (Some(7), "TEXT_MESSAGE_END"),
                (Some(8), "RUN_FINISHED"),
            ]
        );
        assert_eq!(resumed[1].body["delta"], "nothing after the drop is lost");
        assert_eq!(
            resumed.iter().filter(|frame| frame.id == Some(7)).count(),
            1,
            "the event immediately after the cursor is delivered exactly once"
        );

        let after_message = replay(&app, 7).await;
        assert_eq!(after_message.len(), 1);
        assert_eq!(after_message[0].id, Some(8));
        assert_eq!(after_message[0].body["type"], "RUN_FINISHED");
    }

    #[derive(Debug, serde::Serialize, Deserialize, PartialEq, Eq)]
    struct CursorProblem {
        #[serde(rename = "type")]
        problem_type: String,
        status: u16,
        detail: String,
        code: String,
        head: u64,
    }

    #[tokio::test]
    async fn cursors_beyond_the_head_are_typed_problems_not_sse_streams() {
        for cursor in [9, u64::MAX] {
            let response = app()
                .oneshot(
                    Request::get(format!(
                        "/api/v1/agui/events?thread_id=sesn_route_resume&starting_after={cursor}"
                    ))
                    .body(Body::empty())
                    .expect("request builds"),
                )
                .await
                .expect("router answers");

            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            assert_eq!(
                response.headers().get(header::CONTENT_TYPE),
                Some(&header::HeaderValue::from_static(
                    "application/problem+json"
                )),
                "the refusal happens before an SSE response opens"
            );
            let body = to_bytes(response.into_body(), 16 * 1024)
                .await
                .expect("the problem is small");
            let problem: CursorProblem =
                serde_json::from_slice(&body).expect("the problem is machine-readable");
            assert_eq!(
                problem.problem_type,
                "https://spec.chrono-ai.fun/cma/1.0/problems/cursor-beyond-end"
            );
            assert_eq!(problem.status, StatusCode::BAD_REQUEST.as_u16());
            assert_eq!(problem.code, "cursor_beyond_end");
            assert_eq!(problem.head, 8);
            assert!(problem.detail.contains(&cursor.to_string()), "{problem:?}");
            assert!(problem.detail.contains("head 8"), "{problem:?}");

            let round_tripped: CursorProblem = serde_json::from_value(
                serde_json::to_value(&problem).expect("the client problem serializes"),
            )
            .expect("the client problem round-trips");
            assert_eq!(round_tripped, problem);
        }
    }

    #[tokio::test]
    async fn a_cursor_at_the_head_follows_the_next_appended_event() {
        let fixture = fixture(
            owner(),
            Authorization::from_capabilities([Capability::UseOwnSessions]),
        );
        let response = fixture
            .app
            .oneshot(
                Request::get("/api/v1/agui/events?thread_id=sesn_route_resume&starting_after=8")
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("router answers");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE),
            Some(&header::HeaderValue::from_static("text/event-stream"))
        );

        let mut body = response.into_body().into_data_stream();
        fixture
            .store
            .append(durable(9, EventKind::TurnStarted))
            .await;
        let chunk = tokio::time::timeout(Duration::from_secs(2), body.next())
            .await
            .expect("the followed stream observes the next poll")
            .expect("the followed stream stays open")
            .expect("the event body is readable");
        let frames = parse_sse(std::str::from_utf8(&chunk).expect("SSE is UTF-8"));
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].id, Some(9));
        assert_eq!(frames[0].body["type"], "RUN_STARTED");
        assert!(
            tokio::time::timeout(Duration::from_millis(750), body.next())
                .await
                .is_err(),
            "the appended event is not delivered a second time"
        );
    }

    #[tokio::test]
    async fn one_followed_stream_announces_a_rotation_until_the_session_is_ready() {
        let fixture = fixture(
            owner(),
            Authorization::from_capabilities([Capability::UseOwnSessions]),
        );
        let response = fixture
            .app
            .clone()
            .oneshot(
                Request::get("/api/v1/agui/events?thread_id=sesn_route_resume&starting_after=8")
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("router answers");
        assert_eq!(response.status(), StatusCode::OK);
        let mut body = response.into_body().into_data_stream();

        fixture
            .store
            .append(durable(
                9,
                EventKind::StatusChanged {
                    status: SessionStatus::Rotating,
                    stop_reason: None,
                    detail: Some("moving this session to a replacement runtime".into()),
                    capability: None,
                },
            ))
            .await;
        let returning = next_followed_frame(&mut body).await;
        assert_eq!(returning.id, Some(9));
        assert_eq!(returning.body["type"], "RUN_ERROR");
        assert_eq!(returning.body["cma:returns"], true);

        fixture
            .store
            .append(durable(
                10,
                EventKind::Progress {
                    stage: ProvisioningStage::RuntimeRequested,
                },
            ))
            .await;
        let progress = next_followed_frame(&mut body).await;
        assert_eq!(progress.id, Some(10));
        assert_eq!(progress.body["type"], "STATE_DELTA");
        assert_eq!(
            progress.body["delta"][0],
            json!({
                "op": "replace",
                "path": "/progress",
                "value": "runtime_requested",
            })
        );

        fixture
            .store
            .append(durable(
                11,
                EventKind::StatusChanged {
                    status: SessionStatus::Ready,
                    stop_reason: None,
                    detail: None,
                    capability: None,
                },
            ))
            .await;
        let ready = next_followed_frame(&mut body).await;
        assert_eq!(ready.id, Some(11));
        assert_eq!(ready.body["type"], "STATE_DELTA");
        assert_eq!(
            ready.body["delta"][0],
            json!({ "op": "replace", "path": "/status", "value": "ready" })
        );

        for frame in [&returning, &progress, &ready] {
            assert_eq!(frame.body["threadId"], "sesn_route_resume");
            assert_eq!(frame.body["runId"], "sesn_route_resume");
        }
        let state = fold_state(agui::initial_state(), &[progress, ready]);
        assert_eq!(state["progress"], "runtime_requested");
        assert_eq!(state["status"], "ready");
    }

    #[tokio::test]
    async fn a_bounded_failed_rotation_stream_never_fabricates_ready() {
        let fixture = fixture(
            owner(),
            Authorization::from_capabilities([Capability::UseOwnSessions]),
        );
        let response = fixture
            .app
            .clone()
            .oneshot(
                Request::get("/api/v1/agui/events?thread_id=sesn_route_resume&starting_after=8")
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("router answers");
        assert_eq!(response.status(), StatusCode::OK);
        let mut body = response.into_body().into_data_stream();

        fixture
            .store
            .append(durable(
                9,
                EventKind::StatusChanged {
                    status: SessionStatus::Rotating,
                    stop_reason: None,
                    detail: Some("moving this session to a replacement runtime".into()),
                    capability: None,
                },
            ))
            .await;
        let returning = next_followed_frame(&mut body).await;
        assert_eq!(returning.id, Some(9));
        assert_eq!(returning.body["cma:returns"], true);

        fixture
            .store
            .append(durable(
                10,
                EventKind::Progress {
                    stage: ProvisioningStage::RuntimeRequested,
                },
            ))
            .await;
        let progress = next_followed_frame(&mut body).await;
        assert_eq!(progress.id, Some(10));

        fixture
            .store
            .append(durable(
                11,
                EventKind::StatusChanged {
                    status: SessionStatus::Terminated,
                    stop_reason: Some(StopReason::Error),
                    detail: Some("the replacement cannot serve this session".into()),
                    capability: Some("conversation".into()),
                },
            ))
            .await;
        let failed = next_followed_frame(&mut body).await;
        assert_eq!(failed.id, Some(11));
        assert_eq!(failed.body["type"], "RUN_ERROR");
        assert_eq!(failed.body["cma:returns"], false);

        let bounded_attempt = [returning, progress, failed];
        assert!(
            bounded_attempt.iter().all(|frame| {
                frame.body["delta"].as_array().is_none_or(|operations| {
                    operations.iter().all(|operation| {
                        operation["path"] != "/status" || operation["value"] != "ready"
                    })
                })
            }),
            "the failed attempt emitted ready inside cursor 8..=11: {bounded_attempt:?}"
        );
    }

    #[tokio::test]
    async fn an_unauthorised_cursor_is_concealed_before_validation() {
        let response = fixture(
            SubjectId("somebody-else".into()),
            Authorization::from_capabilities([Capability::UseOwnSessions]),
        )
        .app
        .oneshot(
            Request::get(format!(
                "/api/v1/agui/events?thread_id=sesn_route_resume&starting_after={}",
                u64::MAX
            ))
            .body(Body::empty())
            .expect("request builds"),
        )
        .await
        .expect("router answers");

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE),
            Some(&header::HeaderValue::from_static(
                "application/problem+json"
            ))
        );
        let body: Value = serde_json::from_slice(
            &to_bytes(response.into_body(), 16 * 1024)
                .await
                .expect("the problem is small"),
        )
        .expect("the auth problem is JSON");
        assert_eq!(body["code"], "session_not_found");
        assert!(
            body.get("head").is_none(),
            "cursor validation must not disclose the session's durable head: {body}"
        );
    }

    #[tokio::test]
    async fn the_event_log_grant_alone_can_validate_a_cross_owner_cursor() {
        let app = fixture(
            SubjectId("event-log-reader".into()),
            Authorization::from_capabilities([Capability::ReadEventLog]),
        )
        .app;

        let frames = replay(&app, 7).await;
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].id, Some(8));
    }

    #[tokio::test]
    async fn validation_honours_both_physical_and_advertised_durable_tails() {
        let fixture = fixture(
            owner(),
            Authorization::from_capabilities([Capability::UseOwnSessions]),
        );
        fixture
            .store
            .append_physical_only(durable(9, EventKind::TurnStarted))
            .await;

        let physical_tail = replay(&fixture.app, 9).await;
        assert!(
            physical_tail.is_empty(),
            "a cursor at a physical tail that leads the session record is caught up, not invalid"
        );

        fixture.store.advertise_head(10).await;
        let advertised_tail = replay(&fixture.app, 10).await;
        assert!(
            advertised_tail.is_empty(),
            "an advertised legacy position remains reserved even when no event occupies it"
        );
    }

    #[tokio::test]
    async fn state_fold_converges_across_a_production_route_cursor_resume() {
        let app = app_with(vec![
            durable(3, EventKind::ConversationGap { frames: 4 }),
            durable(
                7,
                EventKind::PlaneChanged {
                    from: SessionMode::Terminal,
                    to: SessionMode::Conversation,
                    snapshot_at: "2026-08-25T08:59:57Z".into(),
                },
            ),
            durable(
                9,
                EventKind::UsageReported {
                    input_tokens: 31,
                    output_tokens: 12,
                    context_used: 1_500,
                    context_window: Some(2_000),
                },
            ),
            durable(
                10,
                EventKind::UsageReported {
                    input_tokens: 0,
                    output_tokens: 0,
                    context_used: 0,
                    context_window: None,
                },
            ),
            durable(
                11,
                EventKind::ContextCompacted {
                    was: 1_800,
                    now: 420,
                    window: Some(2_000),
                },
            ),
            durable(
                15,
                EventKind::RuntimeLost {
                    runtime_id: "runtime_first".into(),
                    cause: Some(layer2_core::RuntimeLossCause::Displaced),
                },
            ),
        ]);

        let full = replay(&app, 0).await;
        assert_eq!(
            full.iter().map(|frame| frame.id).collect::<Vec<_>>(),
            vec![Some(3), Some(7), Some(9), Some(10), Some(11), Some(15)]
        );
        let full_state = fold_state(agui::initial_state(), &full);

        let retained = fold_state(agui::initial_state(), &full[..3]);
        let resumed = replay(&app, 9).await;
        let resumed_state = fold_state(retained, &resumed);

        assert_eq!(resumed_state, full_state);
        assert_eq!(
            full_state["usage"],
            json!({
                "inputTokens": 0,
                "outputTokens": 0,
                "contextUsed": 1_500,
                "contextWindow": 2_000,
            })
        );
        assert_eq!(full_state["gaps"], json!([{ "frames": 4 }]));
        assert_eq!(
            full_state["planeChanges"],
            json!([{
                "from": "terminal",
                "to": "conversation",
                "snapshotAt": "2026-08-25T08:59:57Z",
            }])
        );
        assert_eq!(
            full_state["contextCompactions"],
            json!([{ "was": 1_800, "now": 420, "window": 2_000 }])
        );
        assert_eq!(
            full_state["runtimeLosses"],
            json!([{
                "runtime": "runtime_first",
                "cause": "displaced",
                "noticedAt": "2026-08-25T09:00:15Z",
            }])
        );
    }

    #[tokio::test]
    async fn an_idle_route_stream_sends_an_unnamed_uncursored_keepalive() {
        let response = app()
            .oneshot(
                Request::get("/api/v1/agui/events?thread_id=sesn_route_resume&starting_after=8")
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("router answers");
        assert_eq!(response.status(), StatusCode::OK);

        let mut body = response.into_body().into_data_stream();
        let chunk = tokio::time::timeout(
            super::super::super::KEEPALIVE_INTERVAL + Duration::from_secs(1),
            body.next(),
        )
        .await
        .expect("an idle stream sends a keepalive within the documented interval plus margin")
        .expect("the followed stream stays open")
        .expect("the keepalive body chunk is readable");
        let keepalive = std::str::from_utf8(&chunk).expect("SSE is UTF-8");

        assert!(
            keepalive.starts_with(':'),
            "a transport keepalive is an SSE comment: {keepalive:?}"
        );
        assert!(
            keepalive
                .lines()
                .all(|line| !line.starts_with("id:") && !line.starts_with("event:")),
            "a transport keepalive is neither cursor-bearing nor an application event: {keepalive:?}"
        );
    }
}
