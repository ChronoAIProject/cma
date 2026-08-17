//! What a session is granted, and what it is refused by name.

use layer3_adapter::KernelCapabilities;

use super::{Capability, SessionCapabilities};
use crate::error::CoreError;
use crate::options::SessionMode;

/// A runtime that does everything, so a test can say what it takes away.
fn everything() -> KernelCapabilities {
    KernelCapabilities {
        conversation: true,
        terminal: true,
        snapshot: true,
        steer: true,
        usage: true,
    }
}

#[test]
fn a_terminal_session_needs_a_terminal() {
    let err = SessionCapabilities::granted(
        SessionMode::Terminal,
        KernelCapabilities {
            terminal: false,
            ..everything()
        },
    )
    .expect_err("a terminal session on a screenless runtime cannot exist");
    assert_eq!(err, CoreError::CapabilityUnsupported("terminal".into()));
    assert_eq!(err.code(), "capability_unsupported");
}

#[test]
fn a_conversation_session_needs_a_conversation() {
    let err = SessionCapabilities::granted(
        SessionMode::Conversation,
        KernelCapabilities {
            conversation: false,
            ..everything()
        },
    )
    .expect_err("a conversation session on a byte-only runtime cannot exist");
    assert_eq!(err, CoreError::CapabilityUnsupported("conversation".into()));
}

#[test]
fn the_two_planes_are_exclusive() {
    // Both would make send_input ambiguous — keystrokes or a prompt — with no
    // way for an interface to tell which the caller meant.
    let terminal = SessionCapabilities::granted(SessionMode::Terminal, everything()).expect("ok");
    assert!(terminal.terminal && !terminal.conversation);

    let conversation =
        SessionCapabilities::granted(SessionMode::Conversation, everything()).expect("ok");
    assert!(conversation.conversation && !conversation.terminal);
}

#[test]
fn a_terminal_session_is_granted_no_turn_level_abilities() {
    // Steering and usage are expressed in turns and tokens. A screen has
    // neither, so advertising them would promise operations with nothing
    // behind them.
    let granted = SessionCapabilities::granted(SessionMode::Terminal, everything()).expect("ok");
    assert!(!granted.steer);
    assert!(!granted.usage);
    assert!(granted.snapshot, "rotation is not plane-specific");
}

#[test]
fn a_conversation_session_gets_only_what_its_runtime_offers() {
    let granted = SessionCapabilities::granted(
        SessionMode::Conversation,
        KernelCapabilities {
            conversation: true,
            terminal: false,
            snapshot: false,
            steer: false,
            usage: true,
        },
    )
    .expect("ok");
    assert!(granted.conversation && granted.usage);
    assert!(
        !granted.steer && !granted.snapshot,
        "nothing is granted that the runtime did not offer"
    );
}

#[test]
fn a_missing_ability_is_refused_by_its_own_name() {
    let granted = SessionCapabilities::granted(SessionMode::Terminal, everything()).expect("ok");
    assert_eq!(granted.require(Capability::Terminal), Ok(()));
    assert_eq!(
        granted.require(Capability::Usage),
        Err(CoreError::CapabilityUnsupported("usage".into())),
        "a refusal that does not say which ability is missing is a bare 404 with extra steps"
    );
}

#[test]
fn every_capability_has_a_distinct_wire_name() {
    // Clients branch on these, so two abilities sharing a name would make one
    // refusal indistinguishable from another.
    let names = [
        Capability::Conversation.name(),
        Capability::Terminal.name(),
        Capability::Snapshot.name(),
        Capability::Steer.name(),
        Capability::Usage.name(),
    ];
    let mut sorted = names.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), names.len(), "names must be unique: {names:?}");
}

#[test]
fn a_session_recorded_before_capabilities_existed_reads_back_as_holding_none() {
    // The safe direction: such a session's granted set is settled the next
    // time it is placed, and until then it claims nothing it cannot prove.
    let restored: SessionCapabilities = serde_json::from_str("{}").expect("deserialises");
    assert_eq!(restored, SessionCapabilities::default());
}
