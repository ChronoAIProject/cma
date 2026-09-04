//! RFC 6902 conformance and documentation guards for AG-UI state.

use std::collections::BTreeSet;

use json_patch::Patch;
use layer2_core::{
    session::ProvisioningStage, Event, EventKind, LiveFrame, PromptDeliveryState, ResourceReading,
    SessionId, SessionMode, SessionStatus, StopReason,
};
use serde_json::{json, Value};

use super::{initial_state, INITIAL_STATE_JSON, STATE_DELTA_PATHS};
use crate::agui::{live::project_live, project::project, resources::project_reading};

fn logged(seq: u64, kind: EventKind) -> Event {
    Event {
        session_id: SessionId("sesn_state_contract".into()),
        seq,
        kind,
        appended_at: "2026-08-25T09:00:00Z".into(),
    }
}

/// One sample of every durable core event.
///
/// The match is exhaustive on purpose. When the core vocabulary grows, this
/// fixture must be revisited so a new `STATE_DELTA` projection cannot evade the
/// path-vocabulary regression by being absent from its inputs.
fn every_durable_kind() -> Vec<EventKind> {
    let kinds = vec![
        EventKind::SessionCreated,
        serde_json::from_value(json!({ "type": "progress", "stage": "ready" }))
            .expect("the core's public durable progress shape must deserialize"),
        EventKind::StatusChanged {
            status: SessionStatus::Ready,
            stop_reason: None,
            detail: None,
            capability: None,
        },
        EventKind::TurnStarted,
        EventKind::TurnCompleted,
        EventKind::InitialPromptDeliveryChanged {
            state: PromptDeliveryState::Completed,
        },
        EventKind::ConversationOpened {
            conversation_id: "conversation_state_contract".into(),
        },
        EventKind::MessageCommitted {
            content: "complete answer".into(),
        },
        EventKind::ReasoningCommitted {
            content: "checking the state contract".into(),
        },
        EventKind::OwnerMessage {
            content: "owner input".into(),
        },
        EventKind::TurnMetadata {
            metadata: [("responses-only".into(), "not AG-UI state".into())].into(),
        },
        EventKind::ToolCallStarted {
            call_id: "call_state_contract".into(),
            title: "read a file".into(),
            kind: "read".into(),
            arguments: None,
        },
        EventKind::ToolCallUpdated {
            call_id: "call_state_contract".into(),
            status: "completed".into(),
            result: None,
        },
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
        EventKind::TerminalAttached,
        EventKind::TerminalDetached,
        EventKind::TerminalResized {
            columns: 120,
            rows: 40,
        },
        EventKind::ApprovalRequested {
            request_id: "approval/~state_contract".into(),
            action: Some("run a command".into()),
            detail: None,
            locator: None,
        },
        EventKind::ApprovalResolved {
            request_id: "approval/~state_contract".into(),
            allowed: true,
        },
        EventKind::PlaneChanged {
            from: SessionMode::Terminal,
            to: SessionMode::Conversation,
            snapshot_at: "2026-08-25T08:59:00Z".into(),
        },
        EventKind::RuntimeLost {
            runtime_id: "runtime_example".into(),
            cause: None,
        },
        EventKind::CpuPressureOpened {
            used_millis: 900,
            limit_millis: 1_000,
            occupancy_permille: 900,
        },
        EventKind::CpuPressureClosed {
            peak_millis: 950,
            limit_millis: 1_000,
            peak_permille: 950,
            duration_seconds: 8,
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
        EventKind::RuntimeHandoverStarted,
        EventKind::RuntimeIncarnationStarted,
        EventKind::RuntimeStarted {
            generation: std::num::NonZeroU64::new(3).expect("the sample generation is positive"),
        },
    ];

    for kind in &kinds {
        match kind {
            EventKind::SessionCreated
            | EventKind::Progress { .. }
            | EventKind::StatusChanged { .. }
            | EventKind::TurnStarted
            | EventKind::TurnCompleted
            | EventKind::InitialPromptDeliveryChanged { .. }
            | EventKind::ConversationOpened { .. }
            | EventKind::MessageCommitted { .. }
            | EventKind::ReasoningCommitted { .. }
            | EventKind::OwnerMessage { .. }
            | EventKind::TurnMetadata { .. }
            | EventKind::ToolCallStarted { .. }
            | EventKind::ToolCallUpdated { .. }
            | EventKind::UsageReported { .. }
            | EventKind::ContextCompacted { .. }
            | EventKind::ModeChanged { .. }
            | EventKind::PlanSet { .. }
            | EventKind::ConfigOptionsSet { .. }
            | EventKind::AvailableCommandsSet { .. }
            | EventKind::ConversationGap { .. }
            | EventKind::TerminalAttached
            | EventKind::TerminalDetached
            | EventKind::TerminalResized { .. }
            | EventKind::ApprovalRequested { .. }
            | EventKind::ApprovalResolved { .. }
            | EventKind::PlaneChanged { .. }
            | EventKind::RuntimeLost { .. }
            | EventKind::CpuPressureOpened { .. }
            | EventKind::CpuPressureClosed { .. }
            | EventKind::MemoryPressureOpened { .. }
            | EventKind::MemoryPressureClosed { .. }
            | EventKind::RuntimeHandoverStarted
            | EventKind::RuntimeIncarnationStarted
            | EventKind::RuntimeStarted { .. }
            | EventKind::SignInRequired { .. } => {}
        }
    }
    kinds
}

fn durable_state_deltas() -> Vec<crate::agui::project::AguiEvent> {
    every_durable_kind()
        .into_iter()
        .enumerate()
        .flat_map(|(offset, kind)| {
            project(
                &logged(offset as u64 + 1, kind),
                "run_state_contract",
                "sesn_state_contract",
            )
            .events
            .into_iter()
            .filter(|event| event.kind == "STATE_DELTA")
        })
        .collect()
}

/// One sample of every cursorless conversation frame.
fn every_live_frame() -> Vec<LiveFrame> {
    let frames = vec![
        LiveFrame::TerminalOutput(b"terminal output".to_vec()),
        LiveFrame::MessageDelta("message delta".into()),
        LiveFrame::ThoughtDelta("reasoning delta".into()),
        LiveFrame::AvailableCommands(Vec::new()),
        LiveFrame::Exited {
            code: Some(0),
            reason: StopReason::EndTurn,
        },
    ];

    for frame in &frames {
        match frame {
            LiveFrame::TerminalOutput(_)
            | LiveFrame::MessageDelta(_)
            | LiveFrame::ThoughtDelta(_)
            | LiveFrame::AvailableCommands(_)
            | LiveFrame::Exited { .. } => {}
        }
    }
    frames
}

fn every_state_delta() -> Vec<crate::agui::project::AguiEvent> {
    let mut events = durable_state_deltas();

    events.extend(every_live_frame().into_iter().filter_map(|frame| {
        project_live(&frame, "run_state_contract", "sesn_state_contract")
            .filter(|event| event.kind == "STATE_DELTA")
    }));

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

fn emitted_state_delta_paths() -> BTreeSet<String> {
    let mut paths = BTreeSet::new();
    for event in every_state_delta() {
        let operations = event.body["delta"]
            .as_array()
            .unwrap_or_else(|| panic!("{} carried no STATE_DELTA operation list", event.body));
        for operation in operations {
            let path = operation["path"]
                .as_str()
                .unwrap_or_else(|| panic!("{operation} carried no JSON-Pointer path"));
            paths.insert(canonical_state_delta_path(path));
        }
    }
    paths
}

fn canonical_state_delta_path(path: &str) -> String {
    const PREFIX: &str = "/pendingApprovals/";
    let Some(token) = path.strip_prefix(PREFIX) else {
        return path.to_string();
    };

    assert!(
        !token.is_empty(),
        "a pending approval path carried no request id"
    );
    assert!(
        !token.contains('/'),
        "a pending approval id was not encoded as one JSON-Pointer token: {path}"
    );
    let mut chars = token.chars();
    while let Some(character) = chars.next() {
        if character == '~' {
            assert!(
                matches!(chars.next(), Some('0' | '1')),
                "a pending approval path used an invalid RFC 6901 escape: {path}"
            );
        }
    }
    "/pendingApprovals/{requestId}".to_string()
}

fn canonical_state_delta_paths() -> BTreeSet<String> {
    STATE_DELTA_PATHS
        .iter()
        .map(|path| (*path).to_string())
        .collect()
}

fn documented_state_delta_paths(document: &str) -> BTreeSet<String> {
    const START: &str = "### `STATE_DELTA` paths\n\n";
    let section = document
        .split_once(START)
        .unwrap_or_else(|| panic!("docs/API_AGUI.md has no §3 STATE_DELTA path section"))
        .1;
    let section = section
        .split_once("\n### ")
        .map_or(section, |(before_next_section, _)| before_next_section);

    section
        .lines()
        .filter_map(|line| line.strip_prefix("| `"))
        .filter_map(|row| row.split_once("` |").map(|(path, _)| path))
        .filter(|path| path.starts_with('/'))
        .map(str::to_string)
        .collect()
}

fn assert_same_paths(label: &str, actual: &BTreeSet<String>, expected: &BTreeSet<String>) {
    let missing = expected.difference(actual).collect::<Vec<_>>();
    let unexpected = actual.difference(expected).collect::<Vec<_>>();
    assert!(
        missing.is_empty() && unexpected.is_empty(),
        "{label} STATE_DELTA paths drifted; missing: {missing:?}; unexpected: {unexpected:?}"
    );
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
        if is_pending_approval_tombstone(&event) {
            continue;
        }
        let mut state = initial_state();
        let patch = patch_of(&event);
        json_patch::patch(&mut state, &patch)
            .unwrap_or_else(|error| panic!("{} did not apply: {error}", event.body));
    }
}

fn is_pending_approval_tombstone(event: &crate::agui::project::AguiEvent) -> bool {
    let Some(operations) = event.body["delta"].as_array() else {
        return false;
    };
    operations.len() == 1
        && operations[0]["op"] == "remove"
        && operations[0]["path"]
            .as_str()
            .is_some_and(|path| canonical_state_delta_path(path) == "/pendingApprovals/{requestId}")
}

#[test]
fn an_approval_request_and_resolution_are_strictly_applicable_in_order() {
    let requested = project(
        &logged(
            1,
            EventKind::ApprovalRequested {
                request_id: "approval/~one".into(),
                action: Some("command_execution".into()),
                detail: Some("Run the command?".into()),
                locator: None,
            },
        ),
        "run_state_contract",
        "sesn_state_contract",
    );
    let resolved = project(
        &logged(
            2,
            EventKind::ApprovalResolved {
                request_id: "approval/~one".into(),
                allowed: true,
            },
        ),
        "run_state_contract",
        "sesn_state_contract",
    );
    let deltas = requested
        .events
        .iter()
        .chain(&resolved.events)
        .filter(|event| event.kind == "STATE_DELTA")
        .cloned()
        .collect::<Vec<_>>();

    let state = apply_all(initial_state(), &deltas);
    assert_eq!(state["pendingApprovals"], json!({}));
}

#[test]
fn status_and_progress_replacements_apply_to_the_canonical_document() {
    let status = project(
        &logged(
            1,
            EventKind::StatusChanged {
                status: SessionStatus::Ready,
                stop_reason: None,
                detail: None,
                capability: None,
            },
        ),
        "run_state_contract",
        "sesn_state_contract",
    );
    let progress = project(
        &logged(
            2,
            EventKind::Progress {
                stage: ProvisioningStage::RuntimeRequested,
            },
        ),
        "run_state_contract",
        "sesn_state_contract",
    );

    let deltas = status
        .events
        .iter()
        .chain(&progress.events)
        .cloned()
        .collect::<Vec<_>>();
    let state = apply_all(initial_state(), &deltas);

    assert_eq!(state["status"], "ready");
    assert_eq!(state["progress"], "runtime_requested");
}

#[test]
fn emitted_canonical_and_documented_paths_are_equal() {
    let emitted = emitted_state_delta_paths();
    let canonical = canonical_state_delta_paths();
    let documented = documented_state_delta_paths(include_str!("../../../../../docs/API_AGUI.md"));

    assert_eq!(
        canonical.len(),
        STATE_DELTA_PATHS.len(),
        "the canonical STATE_DELTA path table contains a duplicate"
    );
    assert!(
        !documented.is_empty(),
        "the §3 STATE_DELTA path table yielded no documented paths"
    );
    assert_same_paths("emitted", &emitted, &canonical);
    assert_same_paths("documented", &documented, &canonical);
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
