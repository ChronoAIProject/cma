//! RFC 6902 conformance and documentation guards for AG-UI state.

use json_patch::Patch;
use layer2_core::{Event, EventKind, LiveFrame, ResourceReading, SessionId, SessionMode};
use serde_json::{json, Value};

use super::{initial_state, INITIAL_STATE_JSON};
use crate::agui::{live::project_live, project::project, resources::project_reading};

fn logged(seq: u64, kind: EventKind) -> Event {
    Event {
        session_id: SessionId("sesn_state_contract".into()),
        seq,
        kind,
        appended_at: "2026-08-25T09:00:00Z".into(),
    }
}

fn durable_state_deltas() -> Vec<crate::agui::project::AguiEvent> {
    let kinds = vec![
        EventKind::UsageReported {
            input_tokens: 31,
            output_tokens: 12,
            context_used: 2_048,
            context_window: Some(200_000),
        },
        EventKind::ContextCompacted {
            was: 180_000,
            now: 42_000,
            window: Some(200_000),
        },
        EventKind::ModeChanged {
            mode: "workspace-write".into(),
        },
        EventKind::PlanSet {
            entries: Vec::new(),
        },
        EventKind::ConfigOptionsSet {
            options: Vec::new(),
        },
        EventKind::AvailableCommandsSet {
            commands: Vec::new(),
        },
        EventKind::SignInRequired {
            url: "https://signin.example.test/device".into(),
            code: Some("example-code".into()),
            message: Some("Continue sign-in".into()),
        },
        EventKind::ConversationGap { frames: 3 },
        EventKind::PlaneChanged {
            from: SessionMode::Terminal,
            to: SessionMode::Conversation,
            snapshot_at: "2026-08-25T08:59:00Z".into(),
        },
        EventKind::RuntimeLost {
            runtime_id: "runtime_example".into(),
            cause: None,
        },
        EventKind::MemoryPressureOpened {
            used_bytes: 900,
            limit_bytes: 1_000,
            occupancy_permille: 900,
        },
        EventKind::MemoryPressureClosed {
            peak_bytes: 950,
            limit_bytes: 1_000,
            peak_permille: 950,
            duration_seconds: 8,
        },
    ];

    kinds
        .into_iter()
        .enumerate()
        .map(|(offset, kind)| {
            let name = format!("{kind:?}");
            let mut events = project(
                &logged(offset as u64 + 1, kind),
                "run_state_contract",
                "sesn_state_contract",
            )
            .events
            .into_iter()
            .filter(|event| event.kind == "STATE_DELTA")
            .collect::<Vec<_>>();
            assert_eq!(events.len(), 1, "{name} must yield one STATE_DELTA");
            events.pop().expect("the length was checked")
        })
        .collect()
}

fn every_state_delta() -> Vec<crate::agui::project::AguiEvent> {
    let mut events = durable_state_deltas();

    let commands = project_live(
        &LiveFrame::AvailableCommands(Vec::new()),
        "run_state_contract",
        "sesn_state_contract",
    )
    .expect("available commands have an AG-UI projection");
    assert_eq!(commands.kind, "STATE_DELTA");
    events.push(commands);

    let resources = project_reading(
        &ResourceReading {
            cpu_millis_used: None,
            cpu_millis_limit: None,
            memory_bytes_used: None,
            memory_bytes_limit: None,
            memory_bytes_peak: None,
            stall_permille: None,
            ceiling_hits: None,
            allocation_failures: None,
            kills: None,
            received_bytes: None,
            sent_bytes: None,
            received_bytes_per_second: None,
            sent_bytes_per_second: None,
            interval_ms: None,
        },
        "run_state_contract",
        "sesn_state_contract",
    );
    assert_eq!(resources.kind, "STATE_DELTA");
    events.push(resources);

    events
}

fn patch_of(event: &crate::agui::project::AguiEvent) -> Patch {
    serde_json::from_value(event.body["delta"].clone())
        .unwrap_or_else(|error| panic!("{} carried an invalid JSON Patch: {error}", event.body))
}

fn apply_all(mut state: Value, events: &[crate::agui::project::AguiEvent]) -> Value {
    for event in events {
        let patch = patch_of(event);
        json_patch::patch(&mut state, &patch)
            .unwrap_or_else(|error| panic!("{} did not apply: {error}", event.body));
    }
    state
}

#[test]
fn every_emitted_state_delta_applies_with_a_conforming_library() {
    for event in every_state_delta() {
        let mut state = initial_state();
        let patch = patch_of(&event);
        json_patch::patch(&mut state, &patch)
            .unwrap_or_else(|error| panic!("{} did not apply: {error}", event.body));
    }
}

#[test]
fn every_emitted_operation_rejects_an_empty_document() {
    for event in every_state_delta() {
        for operation in patch_of(&event).0 {
            let mut empty = json!({});
            let result = json_patch::patch(&mut empty, std::slice::from_ref(&operation));
            assert!(
                result.is_err(),
                "{operation:?} unexpectedly applied to an empty document as {empty}"
            );
        }
    }
}

#[test]
fn zero_token_counters_are_reports_while_zero_context_stays_unreported() {
    let initial = initial_state();
    assert_eq!(
        initial["usage"],
        json!({
            "inputTokens": null,
            "outputTokens": null,
            "contextUsed": null,
            "contextWindow": null,
        })
    );

    let reported = project(
        &logged(
            1,
            EventKind::UsageReported {
                input_tokens: 0,
                output_tokens: 0,
                context_used: 0,
                context_window: None,
            },
        ),
        "run_state_contract",
        "sesn_state_contract",
    );
    let state = apply_all(initial, &reported.events);

    assert_eq!(
        state["usage"],
        json!({
            "inputTokens": 0,
            "outputTokens": 0,
            "contextUsed": null,
            "contextWindow": null,
        }),
        "zero token totals are real, but zero context means no reading arrived"
    );
}

#[test]
fn zero_context_sentinel_preserves_the_last_reported_reading() {
    let reported = project(
        &logged(
            1,
            EventKind::UsageReported {
                input_tokens: 31,
                output_tokens: 12,
                context_used: 42_100,
                context_window: Some(200_000),
            },
        ),
        "run_state_contract",
        "sesn_state_contract",
    );
    let state = apply_all(initial_state(), &reported.events);

    let contextless = project(
        &logged(
            2,
            EventKind::UsageReported {
                input_tokens: 0,
                output_tokens: 0,
                context_used: 0,
                context_window: None,
            },
        ),
        "run_state_contract",
        "sesn_state_contract",
    );
    let state = apply_all(state, &contextless.events);

    assert_eq!(
        state["usage"],
        json!({
            "inputTokens": 0,
            "outputTokens": 0,
            "contextUsed": 42_100,
            "contextWindow": 200_000,
        }),
        "contextless turn accounting must retain the runtime's last reading"
    );
}

#[test]
fn unreported_commands_are_distinct_from_a_reported_empty_list() {
    let initial = initial_state();
    assert!(
        initial["availableCommands"].is_null(),
        "no command report has arrived yet"
    );

    let reported = project(
        &logged(
            1,
            EventKind::AvailableCommandsSet {
                commands: Vec::new(),
            },
        ),
        "run_state_contract",
        "sesn_state_contract",
    );
    let state = apply_all(initial, &reported.events);

    assert_eq!(
        state["availableCommands"],
        json!([]),
        "the kernel explicitly reported that no commands are available"
    );
}

fn documented_initial_state(document: &str) -> &str {
    const START: &str = "### Canonical initial state\n\n```json\n";
    let after_start = document
        .split_once(START)
        .unwrap_or_else(|| panic!("docs/API_AGUI.md has no canonical initial-state JSON block"))
        .1;
    after_start
        .split_once("\n```")
        .unwrap_or_else(|| panic!("the canonical initial-state JSON block is not closed"))
        .0
}

#[test]
fn the_document_publishes_the_canonical_state_verbatim() {
    let document = include_str!("../../../../../docs/API_AGUI.md");
    let published = documented_initial_state(document);

    assert_eq!(published, INITIAL_STATE_JSON, "the §3 JSON block drifted");
    assert_eq!(
        serde_json::from_str::<Value>(published).expect("the documented state is JSON"),
        initial_state(),
        "the documented state and the code's state differ"
    );
}

#[test]
fn client_notes_start_deltas_from_the_canonical_document() {
    let document = include_str!("../../../../../docs/API_AGUI.md");
    assert!(document.contains("canonical initial state defined in §3"));
    assert!(!document.contains("Apply it to the last `STATE_SNAPSHOT`"));
}
