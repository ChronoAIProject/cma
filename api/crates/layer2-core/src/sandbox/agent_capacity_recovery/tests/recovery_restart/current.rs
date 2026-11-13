//! Domain proposals only; positive current-publication proof belongs to Mongo.
use super::*;
use crate::sandbox::agent_capacity_recovery::activation::RecoveryActivation;

async fn completed() -> (Fixture, RecoveryWork) {
    let store = Store::new(Fixture::new());
    let service = enabled(store.clone(), Runtime::default());
    let caller = store.caller();
    let request = store.request();
    service.admit(&caller, &request).await.unwrap();
    service
        .resume(&caller, &request, "original", NOW, LATER, EXPIRES)
        .await
        .unwrap();
    let state = store.0.lock().unwrap();
    (state.fixture.clone(), state.work.clone().unwrap())
}

#[tokio::test]
async fn current_activation_keeps_original_result_and_requires_current_native_state() {
    let (mut fixture, original) = completed().await;
    let serialized = serde_json::to_vec(&original).unwrap();
    fixture.agent.revision += 1;
    fixture.agent.conversation.last_seq += 1;
    fixture.agent.conversation.active_turn = Some(fixture.agent.conversation.last_seq);
    fixture.agent.status = AgentStatus::Working;
    fixture.agent.work.next_fence += 2;
    fixture.agent.conversation.managed_guidance_revision += 1;
    assert!(RecoveryActivation::capture(&fixture.context(), &original).is_err());
    let activation = RecoveryActivation::capture_current(&fixture.context(), &original).unwrap();
    assert_eq!(
        serde_json::to_vec(activation.completed()).unwrap(),
        serialized
    );
    assert_eq!(
        activation.target().active_turn,
        fixture.agent.conversation.active_turn
    );
    assert_ne!(
        activation.target().input_sha256,
        original.target.input_sha256
    );
    assert_eq!(activation.writer_fence(), fixture.agent.work.next_fence - 1);
    let confirmation = RecoveryConfirmation {
        operation_key: original.request.key().unwrap(),
        target: activation.target().clone(),
        authority_fence: activation.writer_fence(),
        native_state: ConfirmedNativeState::Working,
    };
    activation.require_confirmation(&confirmation).unwrap();
    for variant in 0..4 {
        let mut stale = confirmation.clone();
        match variant {
            0 => stale.target = original.target.clone(),
            1 => stale.authority_fence = original.next_fence - 1,
            2 => stale.native_state = ConfirmedNativeState::Idle,
            _ => stale.target.input_sha256 = original.target.input_sha256.clone(),
        }
        assert_eq!(
            activation.require_confirmation(&stale),
            Err(RecoveryError::InvalidEvidence)
        );
    }
}

#[tokio::test]
async fn current_activation_refuses_changed_native_identity_parent_intent_and_fence_regression() {
    let (fixture, original) = completed().await;
    let changes: [fn(&mut Fixture); 10] = [
        |f| f.agent.conversation.conversation_id = Some("replacement".into()),
        |f| f.agent.conversation.runtime_generation = Some(99),
        |f| f.sandbox.runtime.as_mut().unwrap().boot_id = Some("b".repeat(32)),
        |f| {
            f.agent
                .work
                .receipts
                .iter_mut()
                .find(|r| r.step == AgentWorkStep::OpenConversation)
                .unwrap()
                .request_sha256 = "0".repeat(64)
        },
        |f| f.sandbox.status = SandboxStatus::Retiring,
        |f| f.sandbox.status = SandboxStatus::CredentialRotating,
        |f| f.agent.work.next_fence -= 1,
        |f| f.agent.work.step = AgentWorkStep::PublishConnection,
        |f| f.agent.requested_stop_reason = Some(AgentStopReason::Owner),
        |f| {
            f.agent.work.lease = Some(AgentWorkLease {
                holder: "other".into(),
                fence: f.agent.work.next_fence - 1,
                expires_at: LATER.into(),
            })
        },
    ];
    for (index, change) in changes.into_iter().enumerate() {
        let mut changed = fixture.clone();
        changed.agent.revision += 1;
        change(&mut changed);
        assert!(
            RecoveryActivation::capture_current(&changed.context(), &original).is_err(),
            "variant {index}"
        );
    }
}

#[tokio::test]
async fn current_activation_changes_when_authority_advances_during_native_io() {
    let (mut fixture, original) = completed().await;
    let before = RecoveryActivation::capture_current(&fixture.context(), &original).unwrap();
    fixture.agent.revision += 1;
    fixture.agent.work.next_fence += 1;
    let after = RecoveryActivation::capture_current(&fixture.context(), &original).unwrap();
    assert_ne!(
        before, after,
        "worker must refuse the attachment on recapture"
    );
    assert_eq!(before.completed(), after.completed());
}
