use super::*;
use crate::{KernelId, SubjectId};

fn snapshot(max_threads: u8) -> ChatSettingsSnapshot {
    ChatSettingsSnapshot::new(
        4,
        ChatSettings {
            enabled: true,
            host: ChatHostSettings {
                max_threads,
                ..ChatHostSettings::default()
            },
            ..ChatSettings::default()
        },
    )
    .unwrap()
}

fn host(owner: &str) -> ChatHostRecord {
    ChatHostRecord::new(
        SubjectId(owner.into()),
        snapshot(2),
        "Pinned deployment briefing\n".into(),
        KernelId::new("kernel-chat"),
        "2026-09-01T00:00:00Z".into(),
        "claim-1".into(),
    )
}

#[test]
fn owner_identity_is_deterministic_opaque_and_owner_specific() {
    let first = ChatHostId::for_owner(&SubjectId("owner-a".into()));
    let repeated = ChatHostId::for_owner(&SubjectId("owner-a".into()));
    let other = ChatHostId::for_owner(&SubjectId("owner-b".into()));

    assert_eq!(first, repeated);
    assert_ne!(first, other);
    assert!(first.as_str().starts_with("chho_"));
    assert_eq!(first.as_str().len(), 37);
    assert_eq!(
        first.credential_session_id().as_str(),
        first.as_str().replacen("chho_", "sesn_chat_", 1)
    );
}

#[test]
fn aggregate_validation_rejects_mismatched_identity_and_duplicate_slots() {
    let mut mismatched = host("owner-a");
    mismatched.id = ChatHostId::for_owner(&SubjectId("owner-b".into()));
    assert_eq!(
        mismatched.validate().unwrap_err(),
        "chat host identity does not match its owner"
    );

    let mut duplicate = host("owner-a");
    duplicate.threads = vec![
        ChatThreadSlot {
            session_id: crate::SessionId("sesn_1".into()),
            worktree_name: "chat-1".into(),
            worktree_path: "/workspace/chat-1".into(),
            created_at: "2026-09-01T00:01:00Z".into(),
            removed: false,
        },
        ChatThreadSlot {
            session_id: crate::SessionId("sesn_1".into()),
            worktree_name: "chat-2".into(),
            worktree_path: "/workspace/chat-2".into(),
            created_at: "2026-09-01T00:02:00Z".into(),
            removed: false,
        },
    ];
    duplicate.threads_created = 2;
    assert_eq!(
        duplicate.validate().unwrap_err(),
        "chat host repeats a session slot"
    );
}

#[test]
fn chat_unavailability_classifies_only_retryable_causes_as_transient() {
    for cause in [
        ChatUnavailableCause::NotConfigured,
        ChatUnavailableCause::HostStoreUnavailable,
        ChatUnavailableCause::Provisioning,
        ChatUnavailableCause::ProvisioningFailed {
            phase: ChatProvisioningPhase::WorkspaceSource,
            command_index: None,
            retryable: true,
        },
    ] {
        assert!(cause.is_transient(), "{cause:?}");
    }
    for cause in [
        ChatUnavailableCause::Disabled,
        ChatUnavailableCause::Retired,
        ChatUnavailableCause::CapabilityUnsupported("snapshot".into()),
        ChatUnavailableCause::ProvisioningFailed {
            phase: ChatProvisioningPhase::WorkspaceSetup,
            command_index: Some(2),
            retryable: false,
        },
    ] {
        assert!(!cause.is_transient(), "{cause:?}");
    }
}
