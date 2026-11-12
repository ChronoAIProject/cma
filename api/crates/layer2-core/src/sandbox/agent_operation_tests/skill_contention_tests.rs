use super::*;
use crate::profiles::{SkillRef, SkillSource};
use crate::sandbox::store::SandboxStore;

fn reference() -> SkillRef {
    SkillRef {
        source: SkillSource::Ornn,
        id: crate::SkillId::new("research"),
        version: Some("1.0".into()),
    }
}

#[tokio::test]
async fn idle_skill_cas_exhaustion_is_unavailable_and_retry_admits_once() {
    let store = Arc::new(RejectingConversationStore::default());
    seed_active_parent(&store.inner).await;
    let ready = test_agent(AgentStatus::Ready, None);
    store.create_agent(ready.clone()).await.unwrap();
    let (sender, _) = recording_sender(Ok(()));
    let service = AgentOperationService::new(
        store.clone(),
        TestRuntimeRegistry::new(full_capabilities(), sender),
    );
    store.conflict_commits.store(true, Ordering::SeqCst);
    assert!(matches!(
        service
            .install_skill(&owner(), &ready.id, reference())
            .await,
        Err(AgentOperationError::Unavailable(_))
    ));
    assert_eq!(
        store.commit_attempts.load(Ordering::SeqCst),
        agent_operation_service::MAX_OPERATION_COMMIT_RETRIES
    );
    assert_eq!(
        store.get_agent(&ready.id).await.unwrap(),
        Some(ready.clone())
    );
    assert!(store.agent_log(&ready.id, 0, 100).await.unwrap().is_empty());
    store.conflict_commits.store(false, Ordering::SeqCst);
    let pending = service
        .install_skill(&owner(), &ready.id, reference())
        .await
        .unwrap();
    assert_eq!(pending.work.live_skills.len(), 1);
    assert_eq!(
        service
            .install_skill(&owner(), &ready.id, reference())
            .await
            .unwrap(),
        pending
    );
    assert_eq!(store.agent_log(&ready.id, 0, 100).await.unwrap().len(), 1);
}

#[tokio::test]
async fn ready_child_waits_for_parent_active_without_futile_store_retries() {
    let store = Arc::new(RejectingConversationStore::default());
    let mut parent = seed_active_parent(&store.inner).await;
    let revision = parent.revision;
    parent.status = SandboxStatus::Paused;
    parent.lifecycle.checkpoints.latest = Some(lifecycle_test_support::checkpoint(1, NOW));
    parent
        .begin_revive(SandboxReviveCause::ExplicitRequest, LATER.into())
        .unwrap();
    let Some(SandboxLifecycleOperation::Revive { phase, .. }) = &mut parent.lifecycle.operation
    else {
        panic!("expected revival");
    };
    *phase = SandboxRevivePhase::PublishActive;
    store.save_sandbox(revision, parent).await.unwrap();
    let ready = test_agent(AgentStatus::Ready, None);
    store.create_agent(ready.clone()).await.unwrap();
    let (sender, _) = recording_sender(Ok(()));
    let service = AgentOperationService::new(
        store.clone(),
        TestRuntimeRegistry::new(full_capabilities(), sender),
    );
    assert_eq!(
        service
            .install_skill(&owner(), &ready.id, reference())
            .await,
        Err(AgentOperationError::StatusConflict {
            status: AgentStatus::Ready
        })
    );
    assert_eq!(store.commit_attempts.load(Ordering::SeqCst), 0);
    assert_eq!(store.get_agent(&ready.id).await.unwrap(), Some(ready));
}
