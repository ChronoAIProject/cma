//! A held binding must not prevent the later real Mongo recovery transaction.
use super::*;
use crate::sandbox::SandboxReconciler;
use layer2_core::sandbox::{SandboxPolicy, SandboxWorkState};
use layer3_adapter::{
    codex::{CodexAdapter, CodexSandboxSpec},
    SandboxAdapter, SandboxSpec,
};
use std::{sync::Arc, time::SystemTime};

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn held_binding_preserves_authority_and_later_recovery_commits_once() {
    let (client, name, store) = database().await;
    let (adapter, backend) = CodexAdapter::in_memory(CodexSandboxSpec::default());
    let mut early = active_sandbox("sbx_000_held");
    let handle = adapter
        .provision(&SandboxSpec {
            sandbox_id: early.id.to_string(),
            ..Default::default()
        })
        .await
        .unwrap();
    early.runtime.as_mut().unwrap().runtime_id = handle.0.clone();
    early.runtime.as_mut().unwrap().provision_generation = Some(2);
    early.lifecycle.runtime_generation_high_water = 2;
    early.status = SandboxStatus::Failed;
    early.failure = Some(TypedFailure {
        class: "fixture".into(),
        detail: "fixture".into(),
    });
    early.work = SandboxWorkState::new(SandboxWorkStep::None, BEFORE.into());
    early.validate().unwrap();
    store.create_sandbox(early.clone()).await.unwrap();
    let (later, agent) = checkpoint_fixture(&store, "sbx_999_replaced").await;
    let reconciler = SandboxReconciler::new(
        Arc::new(store.clone()),
        Arc::new(adapter),
        Arc::new(SandboxPolicy::default()),
        "fixture".into(),
    );
    let now = SystemTime::UNIX_EPOCH
        + std::time::Duration::from_secs(layer2_core::seconds_from_rfc3339(OBSERVED).unwrap());
    for _ in 0..3 {
        assert!(reconciler.run_due(now).await.unwrap());
    }
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    let recovered = reopened.get_sandbox(&later.id).await.unwrap().unwrap();
    assert_eq!(recovered.status, SandboxStatus::Reviving);
    assert!(recovered.runtime.is_none());
    assert_eq!(recovered.lifecycle.checkpoints, later.lifecycle.checkpoints);
    assert_eq!(reopened.get_sandbox(&early.id).await.unwrap(), Some(early));
    assert!(
        backend.snapshot(&handle).await.is_ok(),
        "held runtime must not be reclaimed"
    );
    let recovered_agent = reopened.get_agent(&agent.id).await.unwrap().unwrap();
    assert_eq!(recovered_agent.status, AgentStatus::Paused);
    assert_eq!(
        recovered_agent.conversation.conversation_id,
        agent.conversation.conversation_id
    );
    let before = recovered.clone();
    assert!(reconciler.run_due(now).await.unwrap());
    assert_eq!(reopened.get_sandbox(&later.id).await.unwrap(), Some(before));
    client.database(&name).drop().await.unwrap();
}
