//! The durable vocabulary, walked variant by variant.

use std::num::NonZeroU64;

use super::{AvailableCommand, ConfigOption, EventKind, PlanStep};
use crate::session::{RuntimeLossCause, SessionStatus, StopReason};

#[test]
fn status_change_serializes_with_a_type_tag() {
    let e = EventKind::StatusChanged {
        status: SessionStatus::Ready,
        stop_reason: None,
        detail: None,
        capability: None,
    };
    let json = serde_json::to_value(&e).expect("serializes");
    assert_eq!(json["type"], "status_changed");
    assert_eq!(json["status"], "ready");
    assert!(
        json.get("stop_reason").is_none(),
        "absent reason must be omitted, not null"
    );
}

#[test]
fn stop_reason_is_carried_when_present() {
    let e = EventKind::StatusChanged {
        status: SessionStatus::Idle,
        stop_reason: Some(StopReason::RequiresAction),
        detail: None,
        capability: None,
    };
    let json = serde_json::to_value(&e).expect("serializes");
    assert_eq!(json["stop_reason"], "requires_action");
}

/// One sample of every variant.
///
/// The match is exhaustive on purpose. A variant added without being taught
/// to [`KNOWN_TAGS`] stops compiling here, rather than shipping as an event
/// this build writes and then refuses to read back.
fn one_of_every_kind() -> Vec<EventKind> {
    let all = vec![
        EventKind::SessionCreated,
        EventKind::Progress {
            stage: crate::session::ProvisioningStage::Ready,
        },
        EventKind::StatusChanged {
            status: SessionStatus::Ready,
            stop_reason: None,
            detail: None,
            capability: None,
        },
        EventKind::TurnStarted,
        EventKind::TurnCompleted,
        EventKind::InitialPromptDeliveryChanged {
            state: crate::PromptDeliveryState::DispatchFenced,
        },
        EventKind::MessageCommitted {
            content: "hello".into(),
        },
        EventKind::OwnerMessage {
            content: "hello back".into(),
        },
        EventKind::TurnMetadata {
            metadata: [("trace".into(), "one".into())].into(),
        },
        EventKind::ToolCallStarted {
            call_id: "call-1".into(),
            title: "read a file".into(),
            kind: "read".into(),
            arguments: None,
        },
        EventKind::ToolCallUpdated {
            call_id: "call-1".into(),
            status: "completed".into(),
            result: None,
        },
        EventKind::UsageReported {
            input_tokens: 1234,
            output_tokens: 56,
            context_used: 1290,
            context_window: Some(258_400),
        },
        EventKind::ContextCompacted {
            was: 198_000,
            now: 42_100,
            window: Some(200_000),
        },
        EventKind::ModeChanged {
            mode: "autonomous".into(),
        },
        EventKind::PlanSet {
            entries: vec![PlanStep {
                title: "read the file".into(),
                status: "pending".into(),
            }],
        },
        EventKind::ConfigOptionsSet {
            options: vec![ConfigOption {
                id: "thought".into(),
                name: "Thinking level".into(),
                kind: "select".into(),
            }],
        },
        EventKind::AvailableCommandsSet {
            commands: vec![AvailableCommand {
                name: "review".into(),
                description: "Review the working tree".into(),
            }],
        },
        EventKind::ConversationGap { frames: 7 },
        EventKind::TerminalAttached,
        EventKind::TerminalDetached,
        EventKind::TerminalResized {
            columns: 80,
            rows: 24,
        },
        EventKind::ApprovalRequested {
            request_id: "r-1".into(),
            action: Some("run a command".into()),
            detail: None,
            locator: None,
        },
        EventKind::ApprovalResolved {
            request_id: "r-1".into(),
            allowed: true,
        },
        EventKind::CpuPressureOpened {
            used_millis: 850,
            limit_millis: 1_000,
            occupancy_permille: 850,
        },
        EventKind::CpuPressureClosed {
            peak_millis: 970,
            limit_millis: 1_000,
            peak_permille: 970,
            duration_seconds: 181,
        },
        EventKind::MemoryPressureOpened {
            used_bytes: 850,
            limit_bytes: 1_000,
            occupancy_permille: 850,
        },
        EventKind::MemoryPressureClosed {
            peak_bytes: 970,
            limit_bytes: 1_000,
            peak_permille: 970,
            duration_seconds: 181,
        },
        EventKind::RuntimeHandoverStarted,
        EventKind::RuntimeIncarnationStarted,
        EventKind::RuntimeStarted {
            generation: NonZeroU64::new(7).expect("the sample generation is positive"),
        },
        EventKind::RuntimeLost {
            runtime_id: "sbx_1".into(),
            cause: Some(RuntimeLossCause::OutOfMemory),
        },
        EventKind::ConversationOpened {
            conversation_id: "conv-1".into(),
        },
        EventKind::PlaneChanged {
            from: crate::options::SessionMode::Terminal,
            to: crate::options::SessionMode::Conversation,
            snapshot_at: "2026-08-18T09:00:00Z".into(),
        },
        EventKind::SignInRequired {
            url: "https://example.invalid/activate".into(),
            code: Some("ABCD-EFGH".into()),
            message: Some("Open the page and enter the code".into()),
        },
    ];
    for kind in &all {
        match kind {
            EventKind::SessionCreated
            | EventKind::Progress { .. }
            | EventKind::StatusChanged { .. }
            | EventKind::TurnStarted
            | EventKind::TurnCompleted
            | EventKind::InitialPromptDeliveryChanged { .. }
            | EventKind::ConversationOpened { .. }
            | EventKind::MessageCommitted { .. }
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
            | EventKind::CpuPressureOpened { .. }
            | EventKind::CpuPressureClosed { .. }
            | EventKind::MemoryPressureOpened { .. }
            | EventKind::MemoryPressureClosed { .. }
            | EventKind::RuntimeHandoverStarted
            | EventKind::RuntimeIncarnationStarted
            | EventKind::RuntimeStarted { .. }
            | EventKind::RuntimeLost { .. }
            | EventKind::PlaneChanged { .. }
            | EventKind::SignInRequired { .. } => {}
        }
    }
    all
}

#[test]
fn a_runtime_handover_start_is_a_unit_discriminator() {
    let json =
        serde_json::to_value(EventKind::RuntimeHandoverStarted).expect("the marker serializes");
    assert_eq!(
        json,
        serde_json::json!({ "type": "runtime_handover_started" })
    );
}

#[test]
fn a_compatibility_runtime_incarnation_start_is_a_unit_marker() {
    let json = serde_json::to_value(EventKind::RuntimeIncarnationStarted)
        .expect("the compatibility marker serializes");
    assert_eq!(
        json,
        serde_json::json!({ "type": "runtime_incarnation_started" })
    );
}

#[test]
fn a_runtime_start_carries_a_positive_generation() {
    let json = serde_json::to_value(EventKind::RuntimeStarted {
        generation: NonZeroU64::new(12).expect("the sample generation is positive"),
    })
    .expect("serializes");
    assert_eq!(
        json,
        serde_json::json!({ "type": "runtime_started", "generation": 12 })
    );
    assert!(
        serde_json::from_value::<EventKind>(
            serde_json::json!({ "type": "runtime_started", "generation": 0 })
        )
        .is_err(),
        "zero cannot identify a runtime authority generation"
    );
}

#[test]
fn a_runtime_loss_with_no_cause_omits_it_instead_of_writing_null() {
    let json = serde_json::to_value(EventKind::RuntimeLost {
        runtime_id: "sbx_1".into(),
        cause: None,
    })
    .expect("serializes");
    assert_eq!(
        json,
        serde_json::json!({"type": "runtime_lost", "runtime_id": "sbx_1"})
    );
}

#[test]
fn every_variant_is_recognized_by_the_tag_it_serializes_to() {
    for kind in one_of_every_kind() {
        let json = serde_json::to_value(&kind).expect("serializes");
        let tag = json["type"].as_str().expect("carries a type tag");
        assert!(
            EventKind::recognizes_tag(tag),
            "{tag} is written by this build but would be read back as a stranger's"
        );
    }
}

#[test]
fn the_tag_a_variant_reports_is_the_one_it_serializes_under() {
    // The reporting path and the storage path must agree: a diagnostic
    // naming a different event than the one that was lost sends whoever
    // reads it looking in the wrong place.
    for kind in one_of_every_kind() {
        let json = serde_json::to_value(&kind).expect("serializes");
        assert_eq!(kind.tag(), json["type"].as_str().expect("carries a tag"));
        assert!(EventKind::recognizes_tag(&kind.tag()), "{}", kind.tag());
    }
}

#[test]
fn every_variant_round_trips_through_its_serialized_form() {
    for kind in one_of_every_kind() {
        let json = serde_json::to_value(&kind).expect("serializes");
        let back: EventKind = serde_json::from_value(json.clone()).expect("deserializes");
        assert_eq!(
            serde_json::to_value(&back).expect("re-serializes"),
            json,
            "{kind:?} did not survive a round trip"
        );
    }
}

#[test]
fn a_tag_this_build_does_not_own_is_not_recognized() {
    assert!(
        !EventKind::recognizes_tag("message_committed_by_a_newer_control_plane"),
        "an unknown discriminator must not be validated as one of ours"
    );
}
