//! Real lifecycle root: checkpoint fences commit or abort with parent effects.
use super::*;
use crate::store::sandbox_maintenance::mongo_integration_tests::active_sandbox;
use layer2_core::sandbox::{
    store::{
        SandboxEventStore, SandboxLifecycleAgentScope, SandboxLifecycleExecutionStore,
        SandboxLifecycleMutation, SandboxMutationGuard, SandboxStore,
    },
    Sandbox, SandboxEventKind, SandboxWorkState, SandboxWorkStep,
};

const QUIET: &str = "2026-11-11T00:02:00Z";

async fn parent(f: &Fixture) -> Sandbox {
    let mut parent = active_sandbox(f.agent.sandbox_id.as_str());
    parent.owner = f.agent.owner.clone();
    parent.work = SandboxWorkState::new(SandboxWorkStep::None, NOW.into());
    f.store.create_sandbox(parent.clone()).await.unwrap();
    parent
}

fn begin(parent: &Sandbox, at: &str) -> SandboxLifecycleMutation {
    let mut next = parent.clone();
    next.begin_checkpoint_capture(at.into()).unwrap();
    SandboxLifecycleMutation {
        guard: SandboxMutationGuard::capture(parent),
        sandbox: next,
        agents: vec![],
        agent_scope: SandboxLifecycleAgentScope::Touched,
        appended_at: at.into(),
        events: vec![SandboxEventKind::WorkStepCompleted {
            step: SandboxWorkStep::RunLifecycleOperation,
        }],
    }
}

async fn fence(f: &Fixture) -> Option<i64> {
    f.store
        .agents
        .find_one(doc! {"_id": f.agent.id.as_str()})
        .await
        .unwrap()
        .unwrap()
        .get_i64("checkpoint_capture_fence")
        .ok()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn lifecycle_root_captures_segmented_children_and_preserves_quiet_boundary() {
    let f = fixture(2).await;
    let parent = parent(&f).await;
    assert_eq!(
        f.store.commit_sandbox_lifecycle(begin(&parent, NOW)).await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(fence(&f).await, None);
    assert_eq!(
        f.store.get_sandbox(&parent.id).await.unwrap(),
        Some(parent.clone())
    );
    assert!(f
        .store
        .sandbox_events(&parent.id, 0, 10)
        .await
        .unwrap()
        .is_empty());

    let mutation = begin(&parent, QUIET);
    f.store
        .commit_sandbox_lifecycle(mutation.clone())
        .await
        .unwrap();
    assert_eq!(fence(&f).await, Some(1));
    assert_eq!(
        f.store.get_sandbox(&parent.id).await.unwrap(),
        Some(mutation.sandbox)
    );
    assert_eq!(
        f.store
            .sandbox_events(&parent.id, 0, 10)
            .await
            .unwrap()
            .len(),
        1
    );
    unchanged(&f).await;
    f.destroy().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn lifecycle_root_aborts_segmented_child_fence_after_later_domain_refusal() {
    let f = fixture(2).await;
    let parent = parent(&f).await;
    let mut invalid = begin(&parent, QUIET);
    // Capture and child fencing run first; the lifecycle validator then rejects
    // the missing required event. Its error must abort those earlier writes.
    invalid.events.clear();
    assert!(f.store.commit_sandbox_lifecycle(invalid).await.is_err());
    assert_eq!(fence(&f).await, None);
    assert_eq!(
        f.store.get_sandbox(&parent.id).await.unwrap(),
        Some(parent.clone())
    );
    assert!(f
        .store
        .sandbox_events(&parent.id, 0, 10)
        .await
        .unwrap()
        .is_empty());
    unchanged(&f).await;
    // A valid retry must still be able to acquire the original child fence.
    f.store
        .commit_sandbox_lifecycle(begin(&parent, QUIET))
        .await
        .unwrap();
    assert_eq!(fence(&f).await, Some(1));
    f.destroy().await;
}
