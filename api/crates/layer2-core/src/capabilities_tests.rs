//! What a session is granted, and what it is refused by name.

use layer3_adapter::KernelCapabilities;

use super::{servable_modes, Capability, SessionCapabilities};
use crate::error::CoreError;
use crate::options::SessionMode;

/// A runtime that does everything, so a test can say what it takes away.
fn everything() -> KernelCapabilities {
    KernelCapabilities {
        conversation: true,
        terminal: true,
        snapshot: true,
        steer: true,
        interrupt: true,
        usage: true,
        resources: true,
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
fn resource_reporting_is_granted_on_both_planes_when_advertised() {
    let terminal = SessionCapabilities::granted(SessionMode::Terminal, everything()).expect("ok");
    let conversation =
        SessionCapabilities::granted(SessionMode::Conversation, everything()).expect("ok");

    assert_eq!(terminal.resources, Some(true));
    assert_eq!(conversation.resources, Some(true));
}

#[test]
fn a_resource_refusal_is_recorded_rather_than_left_unsettled() {
    let granted = SessionCapabilities::granted(
        SessionMode::Terminal,
        KernelCapabilities {
            resources: false,
            ..everything()
        },
    )
    .expect("ok");

    assert_eq!(granted.resources, Some(false));
    assert_eq!(
        granted.require(Capability::Resources),
        Err(CoreError::CapabilityUnsupported("resources".into()))
    );
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
            interrupt: false,
            usage: true,
            resources: false,
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
        Capability::Interrupt.name(),
        Capability::Usage.name(),
        Capability::Resources.name(),
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
    assert_eq!(restored.resources, None);
}

#[test]
fn a_kernel_that_cannot_take_a_turn_back_does_not_grant_the_operation() {
    // Interrupting is its own ability rather than part of holding a
    // conversation. A wire can offer a prompt and no way to withdraw it, and a
    // session that accepted the request and did nothing would leave whoever
    // asked watching the agent carry on with no explanation.
    let granted = SessionCapabilities::granted(
        SessionMode::Conversation,
        KernelCapabilities {
            interrupt: false,
            ..everything()
        },
    )
    .expect("a conversation is still a conversation");
    assert!(granted.conversation);
    assert!(!granted.interrupt);
    let err = granted
        .require(Capability::Interrupt)
        .expect_err("what the kernel cannot do must be refused by name");
    assert!(
        matches!(&err, CoreError::CapabilityUnsupported(named) if named == "interrupt"),
        "got {err:?}"
    );
}

#[test]
fn interrupting_and_steering_are_told_apart() {
    // Continuing a turn and abandoning one are different things, and kernels
    // differ in which they offer. Collapsing them would advertise one on the
    // strength of the other.
    let steers_only = SessionCapabilities::granted(
        SessionMode::Conversation,
        KernelCapabilities {
            steer: true,
            interrupt: false,
            ..everything()
        },
    )
    .expect("ok");
    assert!(steers_only.steer && !steers_only.interrupt);

    let interrupts_only = SessionCapabilities::granted(
        SessionMode::Conversation,
        KernelCapabilities {
            steer: false,
            interrupt: true,
            ..everything()
        },
    )
    .expect("ok");
    assert!(interrupts_only.interrupt && !interrupts_only.steer);
}

#[test]
fn a_terminal_session_is_not_granted_an_interrupt_operation() {
    // A screen stops what it is doing with a control character, which travels
    // as input. An operation here would be a second way to say it, with
    // nothing behind it.
    let granted = SessionCapabilities::granted(SessionMode::Terminal, everything()).expect("ok");
    assert!(!granted.interrupt);
}

#[test]
fn a_runtime_offers_only_the_planes_it_can_serve() {
    let screen_only = KernelCapabilities {
        conversation: false,
        ..everything()
    };
    assert_eq!(servable_modes(screen_only), vec![SessionMode::Terminal]);

    let chat_only = KernelCapabilities {
        terminal: false,
        ..everything()
    };
    assert_eq!(servable_modes(chat_only), vec![SessionMode::Conversation]);
}

#[test]
fn a_runtime_that_serves_neither_plane_offers_nothing() {
    // Not an error and not an empty list dressed up as one: a runtime nothing
    // can be started on is a runtime a client should draw as unusable, and it
    // can only do that if the answer says so.
    let neither = KernelCapabilities {
        conversation: false,
        terminal: false,
        ..everything()
    };
    assert!(servable_modes(neither).is_empty());
}

#[test]
fn what_is_offered_is_exactly_what_would_be_granted() {
    // The property the advertisement exists for. Anything offered here must
    // survive `granted`, or the refusal has moved from before a session is
    // created to after it.
    for kernel in [
        everything(),
        KernelCapabilities {
            conversation: false,
            ..everything()
        },
        KernelCapabilities {
            terminal: false,
            ..everything()
        },
        KernelCapabilities::default(),
    ] {
        for mode in servable_modes(kernel) {
            assert!(
                SessionCapabilities::granted(mode, kernel).is_ok(),
                "{mode:?} was offered on a runtime that would refuse it"
            );
        }
    }
}
