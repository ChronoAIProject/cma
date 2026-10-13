use super::*;
use layer2_core::sandbox::AgentSettings;
use layer2_core::{Authorization, Caller, Capability};

#[tokio::test]
async fn settings_survive_reattachment_with_original_profile_and_open_receipt() {
    let harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let admitted = harness.admit("settings-replay", Vec::new()).await;
    let ready = harness.drive_ready(&admitted.id).await;
    let settings = AgentSettings {
        model: "gpt-6".into(),
        reasoning_effort: ThinkingEffort::XHigh,
        permission_mode: layer2_core::profiles::PermissionLevel::Guarded,
    };
    let caller = Caller::new(
        ready.owner.clone(),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    let operations = AgentOperationService::new(harness.store.clone(), harness.registry.clone());
    operations
        .update_settings(&caller, &ready.id, settings.clone())
        .await
        .unwrap();
    harness.registry.fence(
        &ready.id,
        ready.conversation.runtime_generation,
        ready.work.next_fence - 1,
    );
    let now = SystemTime::now();
    assert_eq!(harness.worker.reconcile_connections(now).await.unwrap(), 1);
    harness.run(&ready.id, now).await;
    let recovered = harness.stored(&ready.id).await;
    assert_eq!(recovered.work.step, AgentWorkStep::None);
    assert_eq!(recovered.status, AgentStatus::Ready);
    assert_eq!(recovered.settings(), settings);
    assert_eq!(recovered.profile, ready.profile);
    assert_eq!(recovered.work.receipts, ready.work.receipts);
    assert_eq!(
        recovered.conversation.conversation_id,
        ready.conversation.conversation_id
    );
    assert!(harness.registry.resolve(&Harness::target(&ready)).is_some());
}
