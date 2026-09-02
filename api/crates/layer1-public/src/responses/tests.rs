use std::{collections::BTreeMap, sync::Arc};

use axum::{body::to_bytes, response::Response};
use futures_util::StreamExt;
use layer2_core::{
    AgentConfiguration, AgentOptions, Authorization, Baseline, Caller, Capability, ChatSession,
    Deployment, EnvironmentProfile, EnvironmentProfileId, EnvironmentProfileService,
    EnvironmentProfileSpec, EnvironmentProfileStore, Event, EventKind, KernelId, Page, PageRequest,
    PlatformSettingsService, RecoveryState, Reserved, ScreenFactory, Session, SessionId,
    SessionListQuery, SessionPorts, SessionRecord, SessionService, SessionStatus, SessionStore,
    StopReason, StoreError, StoredEvent, SubjectId, TerminalGeometry, ToolBody,
};
use serde_json::{json, Value};
use tokio::sync::Mutex;

use super::{conversations, create, items, response_id, retrieve, stream, ResponseStatus};

const SESSION: &str = "sesn_responses_contract";
const SDK_CONVERSATION_FIXTURE: &str =
    include_str!("../../tests/fixtures/responses-sdk/conversation.json");
const SDK_ITEMS_FIXTURE: &str = include_str!("../../tests/fixtures/responses-sdk/items.json");
const SDK_RESPONSE_FIXTURE: &str = include_str!("../../tests/fixtures/responses-sdk/response.json");
const SDK_STREAM_FIXTURE: &str = include_str!("../../tests/fixtures/responses-sdk/projection.sse");

fn logged(seq: u64, kind: EventKind) -> Event {
    Event {
        session_id: SessionId(SESSION.into()),
        seq,
        kind,
        appended_at: format!("2026-09-01T00:00:{seq:02}Z"),
    }
}

fn session(model: Option<&str>) -> Session {
    Session {
        options: AgentOptions {
            model: model.map(str::to_string),
            ..Default::default()
        },
        chat: None,
        id: SessionId(SESSION.into()),
        owner: SubjectId("responses-owner".into()),
        kernel: KernelId::new("fixture"),
        status: SessionStatus::Ready,
        recovery_state: RecoveryState::Normal,
        stop_reason: Some(StopReason::EndTurn),
        geometry: TerminalGeometry::default(),
        last_seq: 0,
        environment_profile: None,
        runtime_version: None,
        rotations: 0,
        rotated_at: None,
        capabilities: Default::default(),
        granted_services: Some(Vec::new()),
        granted_skills: Some(Vec::new()),
        pricing: None,
        created_at: "2026-09-01T00:00:00Z".into(),
    }
}

async fn problem(response: Response) -> Value {
    let bytes = to_bytes(response.into_body(), 64 * 1024)
        .await
        .expect("the bounded problem body is readable");
    serde_json::from_slice(&bytes).expect("field refusals are JSON problems")
}

#[tokio::test]
async fn active_response_and_stale_predecessor_have_distinct_conflict_codes() {
    let active = problem(super::response_in_progress()).await;
    assert_eq!(active["status"], 409);
    assert_eq!(active["code"], "response_in_progress");
    assert!(active.get("field").is_none());

    let stale = problem(super::previous_conflict()).await;
    assert_eq!(stale["status"], 409);
    assert_eq!(stale["code"], "previous_response_id_conflict");
    assert_eq!(stale["field"], "previous_response_id");
}

#[test]
fn responses_sdk_fixtures_are_server_projections() {
    let id = SessionId("sesn_fixture".into());
    let mut retained = session(Some("chat-model-fixture"));
    retained.id = id.clone();
    retained.chat = Some(ChatSession {
        worktree_name: "fixture-chat".into(),
        worktree_path: "/workspace/fixture-chat".into(),
        title: Some("Fixture chat".into()),
        paused_notice: None,
    });
    let fixture_event = |seq, second, kind| Event {
        session_id: id.clone(),
        seq,
        kind,
        appended_at: format!("2026-09-01T09:00:{second:02}Z"),
    };

    let conversation = conversations::project(
        retained.clone(),
        &[fixture_event(1, 0, EventKind::SessionCreated)],
    );
    assert_eq!(
        serde_json::to_value(conversation).expect("conversation serializes"),
        serde_json::from_str::<Value>(SDK_CONVERSATION_FIXTURE).expect("fixture is JSON")
    );

    let events = vec![
        fixture_event(1, 0, EventKind::TurnStarted),
        fixture_event(
            2,
            1,
            EventKind::OwnerMessage {
                content: "Open the contract".into(),
            },
        ),
        fixture_event(
            3,
            2,
            EventKind::TurnMetadata {
                metadata: BTreeMap::from([("trace".into(), "sdk-fixture".into())]),
            },
        ),
        fixture_event(
            4,
            3,
            EventKind::ToolCallStarted {
                call_id: "call_fixture".into(),
                title: "Read contract".into(),
                kind: "read".into(),
                arguments: Some(ToolBody {
                    text: "docs/API_Responses.md".into(),
                    truncated: false,
                }),
            },
        ),
        fixture_event(
            5,
            4,
            EventKind::ToolCallUpdated {
                call_id: "call_fixture".into(),
                status: "completed".into(),
                result: Some(ToolBody {
                    text: "contract read".into(),
                    truncated: false,
                }),
            },
        ),
        fixture_event(
            6,
            5,
            EventKind::MessageCommitted {
                content: "Hello from CMA".into(),
            },
        ),
        fixture_event(
            7,
            6,
            EventKind::UsageReported {
                input_tokens: 12,
                output_tokens: 4,
                context_used: 16,
                context_window: Some(200_000),
            },
        ),
        fixture_event(8, 7, EventKind::TurnCompleted),
    ];
    let projected_items = items::project(&id, &events);
    let items = items::ConversationItemList {
        object: super::OBJECT_LIST,
        first_id: projected_items.first().map(|item| match item {
            items::ConversationItem::Message(item) => item.id.clone(),
            items::ConversationItem::ToolActivity(item) => item.id.clone(),
        }),
        last_id: projected_items.last().map(|item| match item {
            items::ConversationItem::Message(item) => item.id.clone(),
            items::ConversationItem::ToolActivity(item) => item.id.clone(),
        }),
        data: projected_items,
        has_more: false,
    };
    assert_eq!(
        serde_json::to_value(items).expect("item list serializes"),
        serde_json::from_str::<Value>(SDK_ITEMS_FIXTURE).expect("fixture is JSON")
    );

    let response = retrieve::project(&retained, &events, 1).expect("fixture turn projects");
    assert_eq!(
        serde_json::to_value(response).expect("response serializes"),
        serde_json::from_str::<Value>(SDK_RESPONSE_FIXTURE).expect("fixture is JSON")
    );

    let mut expected_sequence = 0;
    for data in SDK_STREAM_FIXTURE
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
    {
        let parsed: stream::ResponsesConnectionEvent =
            serde_json::from_str(data).expect("server event fixture parses");
        assert_eq!(parsed.sequence_number, expected_sequence);
        assert_eq!(
            serde_json::to_value(parsed).expect("server event serializes"),
            serde_json::from_str::<Value>(data).expect("event data is JSON")
        );
        expected_sequence += 1;
    }
    assert_eq!(expected_sequence, 16);
}

#[test]
fn accepted_request_fields_preserve_the_linear_turn_context() {
    let parsed = create::parse(json!({
        "conversation": SESSION,
        "input": [{
            "type": "message",
            "role": "user",
            "content": [{ "type": "input_text", "text": "continue" }]
        }],
        "stream": true,
        "metadata": { "trace": "one" },
        "previous_response_id": format!("resp_{SESSION}_7"),
        "model": "caller-choice-is-ignored",
        "store": true
    }))
    .expect("the declared subset is accepted");

    assert_eq!(parsed.conversation, SessionId(SESSION.into()));
    assert_eq!(parsed.input, "continue");
    assert_eq!(
        parsed.metadata,
        BTreeMap::from([("trace".into(), "one".into())])
    );
    assert_eq!(
        parsed.previous_response_id.as_deref(),
        Some("resp_sesn_responses_contract_7")
    );
}

#[tokio::test]
async fn every_explicitly_absent_response_field_is_refused_even_when_null() {
    let cases = [
        ("instructions", json!(null)),
        ("reasoning", json!(null)),
        ("tools", json!(null)),
        ("tool_choice", json!(null)),
        ("background", json!(null)),
        ("stream_options", json!(null)),
        ("starting_after", json!(null)),
        ("include", json!(null)),
        ("store", json!(false)),
    ];
    for (field, value) in cases {
        let mut request = json!({
            "conversation": SESSION,
            "input": "hello",
            "stream": true
        });
        request[field] = value;
        let response = create::parse(request).expect_err("the field is outside the subset");
        let body = problem(*response).await;
        assert_eq!(body["code"], "responses_field_unsupported", "{field}");
        assert_eq!(body["field"], field, "{field}");
    }
}

#[tokio::test]
async fn stateless_non_streaming_and_malformed_inputs_are_named_refusals() {
    let cases = [
        (json!({ "input": "hello", "stream": true }), "conversation"),
        (
            json!({ "conversation": SESSION, "input": "hello", "stream": false }),
            "stream",
        ),
        (
            json!({
                "conversation": SESSION,
                "input": [{ "role": "assistant", "content": "no" }],
                "stream": true
            }),
            "input",
        ),
        (
            json!({
                "conversation": SESSION,
                "input": [
                    { "role": "user", "content": "one" },
                    { "role": "user", "content": "two" }
                ],
                "stream": true
            }),
            "input",
        ),
    ];
    for (request, field) in cases {
        let body = problem(*create::parse(request).expect_err("the request is refused")).await;
        assert_eq!(body["field"], field);
    }
}

#[tokio::test]
async fn metadata_uses_the_openai_pair_key_and_value_bounds() {
    let accepted = json!({
        "conversation": SESSION,
        "input": "hello",
        "stream": true,
        "metadata": (0..16)
            .map(|index| (format!("k{index}"), json!("v")))
            .collect::<serde_json::Map<_, _>>()
    });
    assert_eq!(create::parse(accepted).unwrap().metadata.len(), 16);

    for metadata in [
        (0..17)
            .map(|index| (format!("k{index}"), json!("v")))
            .collect::<serde_json::Map<_, _>>(),
        [("k".repeat(65), json!("v"))].into_iter().collect(),
        [("k".into(), json!("v".repeat(513)))].into_iter().collect(),
        [("k".into(), json!(7))].into_iter().collect(),
    ] {
        let body = problem(
            *create::parse(json!({
                "conversation": SESSION,
                "input": "hello",
                "stream": true,
                "metadata": metadata
            }))
            .expect_err("metadata outside the standard bounds is refused"),
        )
        .await;
        assert_eq!(body["field"], "metadata");
    }
}

#[test]
fn durable_response_projection_keeps_metadata_usage_model_and_failure() {
    let events = vec![
        logged(1, EventKind::TurnStarted),
        logged(
            2,
            EventKind::OwnerMessage {
                content: "question".into(),
            },
        ),
        logged(
            3,
            EventKind::TurnMetadata {
                metadata: BTreeMap::from([("trace".into(), "one".into())]),
            },
        ),
        logged(
            4,
            EventKind::MessageCommitted {
                content: "partial answer".into(),
            },
        ),
        logged(
            5,
            EventKind::UsageReported {
                input_tokens: 12,
                output_tokens: 5,
                context_used: 17,
                context_window: Some(200_000),
            },
        ),
        logged(6, EventKind::TurnCompleted),
        logged(
            7,
            EventKind::StatusChanged {
                status: SessionStatus::Ready,
                stop_reason: Some(StopReason::Error),
                detail: Some("fixture failure".into()),
                capability: Some("conversation".into()),
            },
        ),
    ];
    let response = retrieve::project(&session(Some("configured-model")), &events, 1)
        .expect("the durable turn is a response");

    assert_eq!(response.id, response_id(&SessionId(SESSION.into()), 1));
    assert_eq!(response.status, ResponseStatus::Failed);
    assert_eq!(response.model, "configured-model");
    assert_eq!(response.metadata["trace"], "one");
    assert_eq!(response.error.unwrap().code, "capability_unsupported");
    assert_eq!(response.usage.unwrap().total_tokens, 17);
    assert_eq!(response.output[0].content[0].text, "partial answer");
    assert_eq!(
        response.reasoning,
        json!({ "effort": null, "summary": null })
    );
}

#[test]
fn durable_items_are_newest_first_and_fold_tool_activity_without_reasoning() {
    let events = vec![
        logged(1, EventKind::TurnStarted),
        logged(
            2,
            EventKind::OwnerMessage {
                content: "question".into(),
            },
        ),
        logged(
            3,
            EventKind::ToolCallStarted {
                call_id: "call-1".into(),
                title: "Read contract".into(),
                kind: "read".into(),
                arguments: Some(ToolBody {
                    text: "bounded args".into(),
                    truncated: false,
                }),
            },
        ),
        logged(
            4,
            EventKind::ToolCallUpdated {
                call_id: "call-1".into(),
                status: "completed".into(),
                result: Some(ToolBody {
                    text: "bounded result".into(),
                    truncated: true,
                }),
            },
        ),
        logged(
            5,
            EventKind::MessageCommitted {
                content: "answer".into(),
            },
        ),
        logged(6, EventKind::TurnCompleted),
    ];
    let projected = serde_json::to_value(items::project(&SessionId(SESSION.into()), &events))
        .expect("items serialize");

    assert_eq!(projected[0]["role"], "assistant");
    assert_eq!(projected[1]["type"], "cma_chat.tool_activity");
    assert_eq!(projected[1]["kind"], "read");
    assert_eq!(projected[1]["status"], "completed");
    assert_eq!(projected[1]["result"]["truncated"], true);
    assert_eq!(projected[2]["role"], "user");
    assert!(!projected.to_string().contains("reasoning"));
}

#[test]
fn response_identity_parsing_is_strict_and_round_trips_session_underscores() {
    let id = SessionId("sesn_owner_thread_with_underscores".into());
    let encoded = response_id(&id, 42);
    assert_eq!(retrieve::parse_response_id(&encoded), Some((id, 42)));
    for invalid in [
        "resp_sesn_thread_0",
        "resp_thread_1",
        "resp_sesn_thread_nope",
        "sesn_thread_1",
    ] {
        assert_eq!(retrieve::parse_response_id(invalid), None, "{invalid}");
    }
}

#[test]
fn conversation_projection_uses_the_server_owned_chat_title() {
    let mut retained = session(None);
    retained.chat = Some(ChatSession {
        worktree_name: "chat-1".into(),
        worktree_path: "/workspace/chat-1".into(),
        title: Some("Durable thread title".into()),
        paused_notice: None,
    });
    let projected =
        super::conversations::project(retained, &[logged(1, EventKind::SessionCreated)]);
    assert_eq!(projected.object, "conversation");
    assert_eq!(projected.metadata["title"], "Durable thread title");
    assert!(projected.created_at > 0);
}

#[test]
fn an_incomplete_turn_followed_by_another_turn_is_not_projected_as_live() {
    let events = vec![
        logged(1, EventKind::TurnStarted),
        logged(
            2,
            EventKind::OwnerMessage {
                content: "orphaned turn".into(),
            },
        ),
        logged(3, EventKind::TurnStarted),
    ];
    assert!(
        retrieve::project(&session(None), &events, 1).is_none(),
        "a corrupt historical boundary must not masquerade as an active response"
    );
    assert_eq!(
        retrieve::project(&session(None), &events, 3)
            .expect("the latest turn remains visible")
            .status,
        ResponseStatus::InProgress
    );
}

struct StreamStore {
    record: SessionRecord,
    events: Mutex<Vec<Event>>,
}

#[async_trait::async_trait]
impl SessionStore for StreamStore {
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

    async fn list_all(&self, _query: &SessionListQuery) -> Result<Page<SessionRecord>, StoreError> {
        Ok(Page {
            items: vec![self.record.clone()],
            next: None,
        })
    }

    async fn delete(&self, _id: &SessionId) -> Result<(), StoreError> {
        panic!("the stream fixture never deletes its session")
    }

    async fn append_events(&self, _events: &[Event]) -> Result<(), StoreError> {
        panic!("the stream fixture appends through its explicit test port")
    }

    async fn commit_session_idempotent(
        &self,
        _write_id: &str,
        _record: &SessionRecord,
        _events: &[Event],
    ) -> Result<u64, StoreError> {
        panic!("the stream fixture is read-only through the production port")
    }

    async fn create_session_idempotent(
        &self,
        _write_id: &str,
        _record: &SessionRecord,
        _events: &[Event],
    ) -> Result<u64, StoreError> {
        panic!("the stream fixture never creates a session")
    }

    async fn events_since(
        &self,
        id: &SessionId,
        after: u64,
    ) -> Result<Vec<StoredEvent>, StoreError> {
        Ok(self
            .events
            .lock()
            .await
            .iter()
            .filter(|event| event.session_id == *id && event.seq > after)
            .cloned()
            .map(StoredEvent::from)
            .collect())
    }
}

struct NoProfiles;

#[async_trait::async_trait]
impl EnvironmentProfileStore for NoProfiles {
    async fn upsert(&self, _profile: &EnvironmentProfile) -> Result<(), StoreError> {
        panic!("the stream fixture does not author profiles")
    }

    async fn replace_content(
        &self,
        _id: &EnvironmentProfileId,
        _content: &EnvironmentProfileSpec,
        _updated_at: &str,
    ) -> Result<Option<EnvironmentProfile>, StoreError> {
        panic!("the stream fixture does not author profiles")
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
        panic!("the stream fixture does not author profiles")
    }

    async fn preferred_for(
        &self,
        _owner: &SubjectId,
    ) -> Result<Option<EnvironmentProfile>, StoreError> {
        Ok(None)
    }

    async fn delete(&self, _id: &EnvironmentProfileId) -> Result<(), StoreError> {
        panic!("the stream fixture does not author profiles")
    }
}

fn stream_fixture(events: Vec<Event>) -> (Arc<SessionService>, Arc<StreamStore>, Caller, Session) {
    let mut retained = session(Some("configured-chat-model"));
    retained.last_seq = events.last().map_or(0, |event| event.seq);
    let store = Arc::new(StreamStore {
        record: SessionRecord::new(retained.clone()),
        events: Mutex::new(events),
    });
    let settings = Arc::new(PlatformSettingsService::unconfigured());
    let environments = EnvironmentProfileService::new(
        Arc::new(NoProfiles),
        Baseline::default(),
        None,
        None,
        AgentConfiguration::default(),
        settings,
    );
    let screen: ScreenFactory = Arc::new(|_| panic!("stream replay opens no runtime"));
    let service = SessionService::new(
        Default::default(),
        screen,
        Arc::clone(&store) as Arc<dyn SessionStore>,
        SessionPorts::default(),
        environments,
        Deployment::default(),
    );
    let caller = Caller::new(
        retained.owner.clone(),
        Authorization::from_capabilities([Capability::UseOwnSessions]),
    );
    (service, store, caller, retained)
}

async fn next_sse(body: &mut axum::body::BodyDataStream) -> Value {
    loop {
        let bytes = tokio::time::timeout(std::time::Duration::from_secs(1), body.next())
            .await
            .expect("the response stream emits within the test bound")
            .expect("the response stream has not ended")
            .expect("the response body is readable");
        let wire = String::from_utf8(bytes.to_vec()).expect("SSE is UTF-8");
        let data = wire
            .lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .map(str::trim_start)
            .collect::<String>();
        if !data.is_empty() {
            return serde_json::from_str(&data).expect("SSE data is JSON");
        }
    }
}

#[tokio::test]
async fn response_stream_folds_live_reasoning_text_tools_and_durable_completion() {
    let base = vec![
        logged(1, EventKind::TurnStarted),
        logged(
            2,
            EventKind::OwnerMessage {
                content: "question".into(),
            },
        ),
        logged(
            3,
            EventKind::TurnMetadata {
                metadata: BTreeMap::from([("trace".into(), "stream-one".into())]),
            },
        ),
    ];
    let (service, store, caller, retained) = stream_fixture(base);
    let (live, watching) = tokio::sync::broadcast::channel(8);
    let response = stream::open(service, caller, retained, 1, Some(watching));
    assert_eq!(
        response.headers()[axum::http::header::CONTENT_TYPE],
        "text/event-stream"
    );
    let mut body = response.into_body().into_data_stream();
    live.send(layer2_core::LiveFrame::ThoughtDelta("thinking".into()))
        .expect("the stream receiver is live");

    let mut projected = Vec::new();
    loop {
        let event = next_sse(&mut body).await;
        let done = event["type"] == "response.reasoning_summary_text.delta";
        projected.push(event);
        if done {
            break;
        }
    }
    live.send(layer2_core::LiveFrame::MessageDelta("Hel".into()))
        .expect("the stream receiver is live");
    loop {
        let event = next_sse(&mut body).await;
        let done = event["type"] == "response.output_text.delta";
        projected.push(event);
        if done {
            break;
        }
    }

    store.events.lock().await.extend([
        logged(
            4,
            EventKind::ToolCallStarted {
                call_id: "call-stream".into(),
                title: "Read contract".into(),
                kind: "read".into(),
                arguments: Some(ToolBody {
                    text: "arguments".into(),
                    truncated: false,
                }),
            },
        ),
        logged(
            5,
            EventKind::ToolCallUpdated {
                call_id: "call-stream".into(),
                status: "completed".into(),
                result: Some(ToolBody {
                    text: "result".into(),
                    truncated: false,
                }),
            },
        ),
        logged(
            6,
            EventKind::MessageCommitted {
                content: "Hello".into(),
            },
        ),
        logged(
            7,
            EventKind::UsageReported {
                input_tokens: 10,
                output_tokens: 2,
                context_used: 12,
                context_window: Some(200_000),
            },
        ),
        logged(8, EventKind::TurnCompleted),
        logged(
            9,
            EventKind::StatusChanged {
                status: SessionStatus::Ready,
                stop_reason: Some(StopReason::EndTurn),
                detail: None,
                capability: None,
            },
        ),
        logged(10, EventKind::TurnStarted),
        logged(
            11,
            EventKind::ToolCallStarted {
                call_id: "future-call".into(),
                title: "Must stay hidden".into(),
                kind: "future".into(),
                arguments: None,
            },
        ),
    ]);

    loop {
        let event = next_sse(&mut body).await;
        let terminal = matches!(
            event["type"].as_str(),
            Some("response.completed" | "response.failed" | "response.incomplete")
        );
        projected.push(event);
        if terminal {
            break;
        }
    }

    for (expected, event) in projected.iter().enumerate() {
        assert_eq!(event["sequence_number"], expected as u64);
    }
    let kinds = projected
        .iter()
        .map(|event| event["type"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(kinds[0], "response.created");
    assert_eq!(kinds[1], "response.in_progress");
    assert!(kinds.contains(&"response.reasoning_summary_part.added"));
    assert!(kinds.contains(&"response.reasoning_summary_text.done"));
    assert_eq!(
        kinds
            .iter()
            .filter(|kind| **kind == "response.cma_chat.tool_activity")
            .count(),
        2
    );
    assert!(!projected
        .iter()
        .any(|event| event.to_string().contains("future-call")));
    let suffix = projected
        .iter()
        .find(|event| event["type"] == "response.output_text.delta" && event["delta"] == "lo")
        .expect("durability fills the suffix missed by the live stream");
    assert_eq!(suffix["delta"], "lo");
    let item_done = projected
        .iter()
        .find(|event| event["type"] == "response.output_item.done")
        .expect("the committed message closes its output item");
    assert!(item_done["item"].is_object(), "the done item is never null");
    let terminal = projected.last().unwrap();
    assert_eq!(terminal["type"], "response.completed");
    assert_eq!(terminal["response"]["model"], "configured-chat-model");
    assert_eq!(terminal["response"]["metadata"]["trace"], "stream-one");
    assert_eq!(
        terminal["response"]["output"][0]["content"][0]["text"],
        "Hello"
    );
    assert_eq!(terminal["response"]["usage"]["total_tokens"], 12);
    assert_eq!(
        terminal["response"]["reasoning"],
        json!({ "effort": null, "summary": null })
    );
}

#[tokio::test]
async fn completed_boundary_and_failure_status_fold_into_one_failed_stream() {
    let events = vec![
        logged(1, EventKind::TurnStarted),
        logged(
            2,
            EventKind::OwnerMessage {
                content: "question".into(),
            },
        ),
        logged(
            3,
            EventKind::MessageCommitted {
                content: "partial".into(),
            },
        ),
        logged(4, EventKind::TurnCompleted),
        logged(
            5,
            EventKind::StatusChanged {
                status: SessionStatus::Ready,
                stop_reason: Some(StopReason::Error),
                detail: Some("runtime refused the turn".into()),
                capability: None,
            },
        ),
    ];
    let (service, _, caller, retained) = stream_fixture(events);
    let response = stream::open(service, caller, retained, 1, None);
    let mut body = response.into_body().into_data_stream();
    let mut projected = Vec::new();
    loop {
        let event = next_sse(&mut body).await;
        let terminal = event["type"] == "response.failed";
        projected.push(event);
        if terminal {
            break;
        }
    }
    assert_eq!(
        projected.last().unwrap()["response"]["error"]["code"],
        "turn_failed"
    );
    assert!(projected
        .iter()
        .find(|event| event["type"] == "response.output_item.done")
        .unwrap()["item"]
        .is_object());
}
