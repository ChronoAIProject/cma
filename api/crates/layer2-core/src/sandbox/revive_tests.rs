use super::*;

#[test]
fn revive_phase_order_is_complete_deterministic_and_has_no_install_step() {
    assert_eq!(
        SandboxRevivePhase::first(),
        SandboxRevivePhase::CheckpointVerification
    );
    for phases in SANDBOX_REVIVE_PHASE_ORDER.windows(2) {
        assert_eq!(phases[0].next(), Some(phases[1]));
    }
    assert_eq!(SandboxRevivePhase::PublishActive.next(), None);

    let wire = serde_json::to_string(&SANDBOX_REVIVE_PHASE_ORDER).unwrap();
    assert_eq!(
        wire,
        concat!(
            "[\"checkpoint_verification\",\"runtime_provisioning\",",
            "\"credential_delivery\",\"home_restore\",\"conversation_reopen\",",
            "\"queued_message_delivery\",\"publish_active\"]"
        )
    );
    assert!(!wire.contains("install"));
}

#[test]
fn every_required_entry_point_has_a_stable_revive_cause() {
    let causes = [
        SandboxReviveCause::AgentMessage,
        SandboxReviveCause::WorkspaceCreation,
        SandboxReviveCause::AgentCreation,
        SandboxReviveCause::ExplicitRequest,
        SandboxReviveCause::TriggerLaunch,
        SandboxReviveCause::ImageDrift,
        SandboxReviveCause::RuntimeLost,
    ];
    assert_eq!(
        serde_json::to_value(causes).unwrap(),
        serde_json::json!([
            "agent_message",
            "workspace_creation",
            "agent_creation",
            "explicit_request",
            "trigger_launch",
            "image_drift",
            "runtime_lost"
        ])
    );
}

#[test]
fn revive_failures_are_classified_by_the_phase_that_failed() {
    for phase in [
        SandboxRevivePhase::CheckpointVerification,
        SandboxRevivePhase::HomeRestore,
        SandboxRevivePhase::ConversationReopen,
        SandboxRevivePhase::QueuedMessageDelivery,
        SandboxRevivePhase::PublishActive,
    ] {
        assert_eq!(
            phase.failure_class(),
            SandboxReviveFailureClass::RestoreFailed
        );
    }
    for phase in [
        SandboxRevivePhase::RuntimeProvisioning,
        SandboxRevivePhase::CredentialDelivery,
    ] {
        assert_eq!(
            phase.failure_class(),
            SandboxReviveFailureClass::ProvisioningFailed
        );
    }

    assert_eq!(
        SandboxReviveFailureClass::RestoreFailed.as_str(),
        "restore_failed"
    );
    assert_eq!(
        SandboxReviveFailureClass::ProvisioningFailed.as_str(),
        "provisioning_failed"
    );
    assert_eq!(
        SandboxReviveFailureClass::RuntimeLost.as_str(),
        "runtime_lost"
    );
    assert_eq!(
        SandboxReviveFailureClass::RuntimeLost.typed_failure("no complete checkpoint"),
        TypedFailure {
            class: "runtime_lost".into(),
            detail: "no complete checkpoint".into(),
        }
    );
}
