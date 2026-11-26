//! Real lifecycle root: checkpoint fences commit or abort with parent effects.
use super::*;
use crate::store::sandbox_maintenance::mongo_integration_tests::active_sandbox;
use layer2_core::sandbox::{
    store::{
        SandboxEventStore, SandboxLifecycleAgentScope, SandboxLifecycleExecutionStore,
        SandboxLifecycleMutation, SandboxMutationGuard, SandboxStore, WorkspaceStore,
    },
    Sandbox, SandboxEventKind, SandboxWorkState, SandboxWorkStep,
};

const QUIET: &str = "2026-11-11T00:02:00Z";

pub(super) async fn parent(f: &Fixture) -> Sandbox {
    let mut parent = active_sandbox(f.agent.sandbox_id.as_str());
    parent.owner = f.agent.owner.clone();
    parent.work = SandboxWorkState::new(SandboxWorkStep::None, NOW.into());
    f.store.create_sandbox(parent.clone()).await.unwrap();
    f.store
        .create_workspace(workspace(f, &parent))
        .await
        .unwrap();
    parent
}

fn workspace(f: &Fixture, parent: &Sandbox) -> layer2_core::sandbox::Workspace {
    use layer2_core::{profiles::*, sandbox::*};
    Workspace {
        id: f.agent.workspace_id.clone(),
        sandbox_id: parent.id.clone(),
        owner: parent.owner.clone(),
        origin: WorkspaceOrigin::Manual,
        profile: WorkspaceProfileSnapshot {
            id: layer2_core::WorkspaceProfileId("wkp_receipt".into()),
            revision: 1,
            digest: "d".repeat(64),
            content: WorkspaceProfileContent {
                location: WorkspaceLocation::Directory {
                    name: "receipt".into(),
                },
                name: "receipt".into(),
                setup_commands: vec![],
                agent_profiles: vec![],
            },
        },
        name: "receipt".into(),
        path: "/home/cma/receipt".into(),
        source: WorkspaceSource::Empty,
        resolved_commit: Some("a".repeat(40)),
        status: WorkspaceStatus::Ready,
        work: WorkspaceWorkState::new(WorkspaceWorkStep::None, NOW.into()),
        next_worktree_number: 1,
        created_at: NOW.into(),
        ready_at: Some(NOW.into()),
        failed: None,
        removed_at: None,
        revision: 1,
    }
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

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn lifecycle_reconciliation_requires_original_parent_events_and_child_fences() {
    use crate::store::lifecycle_transaction::{apply_lifecycle, reconciliation};
    let f = fixture(2).await;
    let parent = parent(&f).await;
    let request = begin(&parent, QUIET);
    let mut session = transaction(&f).await;
    let scope = TransactionScope::start(&mut session).await.unwrap();
    let aborted = apply_lifecycle(&f.store, &request, scope, None)
        .await
        .unwrap();
    session.abort_transaction().await.unwrap();
    let mut snapshot = f.client.start_session().snapshot(true).await.unwrap();
    assert!(!reconciliation::snapshot(&f.store, &aborted, &mut snapshot)
        .await
        .unwrap());
    assert_eq!(fence(&f).await, None);

    let scope = TransactionScope::start(&mut session).await.unwrap();
    let receipt = apply_lifecycle(&f.store, &request, scope, None)
        .await
        .unwrap();
    assert!(matches!(
        commit_transaction(&mut session).await,
        CommitOutcome::Committed
    ));
    let mut snapshot = f.client.start_session().snapshot(true).await.unwrap();
    assert!(reconciliation::snapshot(&f.store, &receipt, &mut snapshot)
        .await
        .unwrap());
    let mut foreign = receipt.clone();
    foreign.sandbox.name.push_str("-foreign");
    assert!(!reconciliation::snapshot(&f.store, &foreign, &mut snapshot)
        .await
        .unwrap());
    let mut foreign = receipt.clone();
    foreign.events[0].appended_at = NOW.into();
    assert_eq!(
        reconciliation::snapshot(&f.store, &foreign, &mut snapshot).await,
        Err(SandboxStoreError::SequenceConflict)
    );
    f.store
        .workspaces
        .update_one(
            doc! {"_id":f.agent.workspace_id.as_str()},
            doc! {"$inc":{"checkpoint_capture_fence":1_i64}},
        )
        .await
        .unwrap();
    let mut changed_workspace = f.client.start_session().snapshot(true).await.unwrap();
    assert!(
        !reconciliation::snapshot(&f.store, &receipt, &mut changed_workspace)
            .await
            .unwrap()
    );
    f.store
        .workspaces
        .update_one(
            doc! {"_id":f.agent.workspace_id.as_str()},
            doc! {"$inc":{"checkpoint_capture_fence":-1_i64}},
        )
        .await
        .unwrap();
    f.store
        .agents
        .update_one(
            doc! {"_id":f.agent.id.as_str()},
            doc! {"$inc":{"checkpoint_capture_fence":1_i64}},
        )
        .await
        .unwrap();
    let mut later = f.client.start_session().snapshot(true).await.unwrap();
    assert!(!reconciliation::snapshot(&f.store, &receipt, &mut later)
        .await
        .unwrap());
    // The original coherent snapshot still proves the exact original fence.
    assert!(reconciliation::snapshot(&f.store, &receipt, &mut snapshot)
        .await
        .unwrap());
    unchanged(&f).await;
    let other = fixture(2).await;
    let other_parent = self::parent(&other).await;
    other
        .store
        .commit_sandbox_lifecycle(begin(&other_parent, QUIET))
        .await
        .unwrap();
    let mut foreign_database = other.client.start_session().snapshot(true).await.unwrap();
    assert!(
        !reconciliation::snapshot(&other.store, &receipt, &mut foreign_database)
            .await
            .unwrap()
    );
    other.destroy().await;
    f.destroy().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn lifecycle_root_reconciles_uncertain_checkpoint_without_repeating_fences() {
    use mongodb::{options::ClientOptions, Client};
    let mut f = fixture(2).await;
    let parent = parent(&f).await;
    let admin = f.client.clone();
    let application = format!("lifecycle-unknown-{}", uuid::Uuid::new_v4().simple());
    let mut options =
        ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap())
            .await
            .unwrap();
    options.app_name = Some(application.clone());
    f.client = Client::with_options(options).unwrap();
    f.store = MongoSandboxStore::open(&f.client, &f.store.agents.namespace().db)
        .await
        .unwrap();
    admin.database("admin").run_command(doc! {
        "configureFailPoint":"failCommand", "mode":{"times":5_i32},
        "data":{"failCommands":["commitTransaction"],"appName":application,
        "writeConcernError":{"code":50_i32,"codeName":"MaxTimeMSExpired","errmsg":"injected uncertainty"}}
    }).await.unwrap();
    let request = begin(&parent, QUIET);
    let result = f.store.commit_sandbox_lifecycle(request.clone()).await;
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
    result.unwrap();
    assert_eq!(fence(&f).await, Some(1));
    assert_eq!(
        f.store.get_sandbox(&parent.id).await.unwrap(),
        Some(request.sandbox)
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
