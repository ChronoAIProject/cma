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
    use std::sync::Arc;

    use axum::{
        body::{to_bytes, Body},
        http::{header, Request, StatusCode},
        Extension, Router,
    };
    use layer2_core::{
        AgentConfiguration, Authorization, Baseline, Capability, Deployment, DirectoryQuery,
        EnvironmentProfile, EnvironmentProfileId, EnvironmentProfileService,
        EnvironmentProfileSpec, EnvironmentProfileStore, Event, EventKind, KernelId, Page,
        PageRequest, PlatformSettingsService, RecoveryState, RegisteredUser, Reserved,
        ScreenFactory, Session, SessionCapabilities, SessionId, SessionListQuery, SessionPorts,
        SessionRecord, SessionService, SessionStatus, SessionStore, StoreError, StoredEvent,
        SubjectId, TerminalGeometry, UserDirectory, UserDirectoryStore,
    };
    use serde_json::Value;
    use tower::ServiceExt;

    use crate::{agui, auth::AuthenticatedSubject, rest::SessionsState, tickets::TerminalTickets};

    struct ReplayStore {
        record: SessionRecord,
        events: Vec<Event>,
    }

    #[async_trait::async_trait]
    impl SessionStore for ReplayStore {
        async fn ping(&self) -> Result<(), StoreError> {
            Ok(())
        }

        async fn get(&self, id: &SessionId) -> Result<Option<SessionRecord>, StoreError> {
            Ok((self.record.session.id == *id).then(|| self.record.clone()))
        }

        async fn list_for(
            &self,
            owner: &SubjectId,
            _query: &SessionListQuery,
        ) -> Result<Page<SessionRecord>, StoreError> {
            Ok(Page {
                items: (&self.record.session.owner == owner)
                    .then(|| self.record.clone())
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
            Ok(Page {
                items: vec![self.record.clone()],
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
            let mut events = self
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

    fn durable(seq: u64, kind: EventKind) -> Event {
        Event {
            session_id: SessionId("sesn_route_resume".into()),
            seq,
            kind,
            appended_at: format!("2026-08-25T09:00:{seq:02}Z"),
        }
    }

    fn app() -> Router {
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
        let owner = SubjectId("subject-route-resume".into());
        let record = SessionRecord::new(Session {
            options: Default::default(),
            id: SessionId("sesn_route_resume".into()),
            owner: owner.clone(),
            kernel: KernelId::new("test-kernel"),
            status: SessionStatus::Idle,
            recovery_state: RecoveryState::Idle,
            stop_reason: None,
            geometry: TerminalGeometry::default(),
            last_seq: 8,
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
        let store: Arc<dyn SessionStore> = Arc::new(ReplayStore { record, events });
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
            store,
            SessionPorts::default(),
            Arc::clone(&environments),
            Deployment::default(),
        );
        let state = SessionsState {
            sessions,
            users: UserDirectory::new(Arc::new(NoDirectory)),
            tickets: TerminalTickets::new(),
            environments,
        };
        let authorization = Authorization::from_capabilities([Capability::UseOwnSessions]);

        agui::router(state)
            .layer(Extension(authorization))
            .layer(Extension(AuthenticatedSubject(owner)))
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

        let after_message = replay(&app, 7).await;
        assert_eq!(after_message.len(), 1);
        assert_eq!(after_message[0].id, Some(8));
        assert_eq!(after_message[0].body["type"], "RUN_FINISHED");
    }
}
