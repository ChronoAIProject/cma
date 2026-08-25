//! The server-owned AG-UI contract consumed by the repository SDK.
//!
//! Unit tests beside each projection arm remain useful, but they cannot catch
//! a client and server that each changed consistently in different ways. This
//! corpus crosses that boundary: Rust builds the bytes the route emits and the
//! TypeScript SDK parses byte-identical checked-in copies.

use std::{collections::BTreeSet, convert::Infallible, path::PathBuf};

use axum::response::IntoResponse;
use layer2_core::{Event, EventKind, LiveFrame, SessionId, SessionStatus, StopReason};

use super::{capabilities, live, project};

const RUN_1: &str = "run_fixture_1";
const RUN_2: &str = "run_fixture_2";
const THREAD: &str = "sesn_fixture";

fn logged(seq: u64, kind: EventKind) -> Event {
    Event {
        session_id: SessionId(THREAD.into()),
        seq,
        kind,
        appended_at: String::new(),
    }
}

fn push_durable(events: &mut Vec<project::AguiEvent>, seq: u64, run: &str, kind: EventKind) {
    events.extend(project::project(&logged(seq, kind), run, THREAD).events);
}

fn fixture_events() -> Vec<project::AguiEvent> {
    let mut events = Vec::new();

    push_durable(&mut events, 1, RUN_1, EventKind::TurnStarted);
    push_durable(
        &mut events,
        2,
        RUN_1,
        EventKind::OwnerMessage {
            content: "open the contract".into(),
        },
    );
    push_durable(
        &mut events,
        3,
        RUN_1,
        EventKind::ToolCallStarted {
            call_id: "call_fixture".into(),
            title: "Read contract".into(),
            kind: "read".into(),
            arguments: Some(layer2_core::event::ToolBody {
                text: r#"{"path":"docs/API_AGUI.md"}"#.into(),
                truncated: false,
            }),
        },
    );
    push_durable(
        &mut events,
        4,
        RUN_1,
        EventKind::ToolCallUpdated {
            call_id: "call_fixture".into(),
            status: "completed".into(),
            result: Some(layer2_core::event::ToolBody {
                text: "contract read".into(),
                truncated: false,
            }),
        },
    );
    push_durable(
        &mut events,
        5,
        RUN_1,
        EventKind::UsageReported {
            input_tokens: 120,
            output_tokens: 30,
            context_used: 4_096,
            context_window: Some(200_000),
        },
    );
    events.push(
        live::project_live(
            &LiveFrame::ThoughtDelta("checking the contract".into()),
            RUN_1,
            THREAD,
        )
        .expect("reasoning has an AG-UI shape"),
    );
    push_durable(&mut events, 6, RUN_1, EventKind::TurnCompleted);
    push_durable(&mut events, 7, RUN_2, EventKind::TurnStarted);
    push_durable(
        &mut events,
        8,
        RUN_2,
        EventKind::StatusChanged {
            status: SessionStatus::Terminated,
            stop_reason: Some(StopReason::Error),
            detail: Some("fixture failure".into()),
            capability: None,
        },
    );

    events
}

fn enabled_capability_events() -> BTreeSet<String> {
    capabilities::document()["events"]
        .as_object()
        .expect("the capability document has an events map")
        .iter()
        .filter(|(_, enabled)| enabled.as_bool() == Some(true))
        .map(|(kind, _)| kind.clone())
        .collect()
}

#[test]
fn the_fixture_covers_every_event_the_capability_document_enables() {
    let fixture = fixture_events()
        .iter()
        .map(|event| event.kind.to_string())
        .collect::<BTreeSet<_>>();
    assert_eq!(fixture, enabled_capability_events());
}

async fn on_the_wire(event: project::AguiEvent) -> String {
    let frame = project::sse_frame(&event);
    let response = axum::response::sse::Sse::new(futures_util::stream::once(async move {
        Ok::<_, Infallible>(frame)
    }))
    .into_response();
    let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .expect("the fixture frame is bounded");
    String::from_utf8(body.to_vec()).expect("SSE is UTF-8")
}

async fn projection_fixture() -> String {
    let mut body = String::new();
    for event in fixture_events() {
        body.push_str(&on_the_wire(event).await);
    }
    body
}

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/agui-sdk")
        .join(name)
}

fn assert_or_update_fixture(name: &str, expected: &str) {
    let path = fixture_path(name);
    if std::env::var("UPDATE_AGUI_SDK_FIXTURES").as_deref() == Ok("1") {
        std::fs::create_dir_all(path.parent().expect("fixture has a parent"))
            .expect("the fixture directory can be created");
        std::fs::write(&path, expected).expect("the fixture can be updated");
    }
    let actual = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{} is missing: {error}", path.display()));
    assert_eq!(
        actual, expected,
        "{name} drifted; regenerate with UPDATE_AGUI_SDK_FIXTURES=1"
    );
}

#[tokio::test]
async fn checked_in_sdk_fixtures_are_generated_from_the_server_contract() {
    let capability = format!(
        "{}\n",
        serde_json::to_string_pretty(&capabilities::document())
            .expect("the capability document serializes")
    );
    let projection = projection_fixture().await;

    assert_or_update_fixture("capabilities.json", &capability);
    assert_or_update_fixture("projection.sse", &projection);
}

#[tokio::test]
async fn fixture_generation_is_deterministic() {
    assert_eq!(projection_fixture().await, projection_fixture().await);
}
