//! Real disposable Mongo transactions with the in-memory runtime contract and
//! bounded local Nyx HTTP key fixture. No production provider is contacted.
use super::{
    caller, change,
    fixture::{Fixture, VerifiedHuman},
};
use crate::sandbox::{
    channel_children::{self, ChannelChildEvidence},
    channel_provisioning::state::{ChannelProvisioningPhase, ChannelProvisioningSnapshot},
};
use crate::store::{agent_provenance, MongoBridgeAllocationStore};
use layer2_core::bridge::{
    allocation::AllocationError, AcceptedWorkPolicy, BridgeChange, BridgeStore,
};
use layer2_core::sandbox::{
    store::{AgentStore, AgentWorkerStore, WorkspaceWorkerStore, WorktreeStore},
    AgentCreationOrigin, AgentStatus, AgentWorkStep, WorkspaceStatus, WorktreeStatus,
};
use layer3_adapter::{wire::WireOperation, RuntimeHandle};
use mongodb::bson::{doc, Document};
use std::sync::Arc;

async fn commit(
    f: &mut Fixture,
    snapshot: &ChannelProvisioningSnapshot,
    evidence: &ChannelChildEvidence,
) -> ChannelProvisioningSnapshot {
    let result = f
        .allocations
        .commit_channel_child(&f.authority, &f.lease, snapshot, evidence)
        .await
        .unwrap();
    f.record = result.allocation;
    f.children().await
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_children_reconcile_real_prepare_and_admit_one_exact_held_agent_with_provenance() {
    let mut f = Fixture::new().await;
    let snapshot = f.children().await;
    let snapshot = commit(&mut f, &snapshot, &ChannelChildEvidence::WorkspaceAdmission).await;
    assert_eq!(
        snapshot.state.phase,
        ChannelProvisioningPhase::WorkspacePrepare
    );
    let workspace = f.allocations.channel_workspace(&snapshot).await.unwrap();
    assert!(workspace.resolved_commit.is_none());
    let request = channel_children::prepare_request(&snapshot, &workspace);
    let handle = RuntimeHandle::new(&snapshot.sandbox.runtime.as_ref().unwrap().runtime_id);
    f.backend
        .lose_next_response(WireOperation::PrepareWorkspace);
    assert!(f
        .adapter
        .prepare_workspace(&handle, &request)
        .await
        .is_err());
    let receipt = f
        .adapter
        .prepare_workspace(&handle, &request)
        .await
        .unwrap();
    let actual_commit = receipt.resolved_commit.clone();
    let prepares = f
        .backend
        .requests()
        .await
        .into_iter()
        .filter(|(_, frame)| frame.operation() == WireOperation::PrepareWorkspace)
        .collect::<Vec<_>>();
    assert_eq!(prepares.len(), 2);
    assert_eq!(prepares[0], prepares[1]);
    let snapshot = commit(
        &mut f,
        &snapshot,
        &ChannelChildEvidence::WorkspacePrepare { request, receipt },
    )
    .await;
    let workspace = f.allocations.channel_workspace(&snapshot).await.unwrap();
    assert_eq!(workspace.resolved_commit.as_ref(), Some(&actual_commit));
    assert_eq!(workspace.status, WorkspaceStatus::Preparing);
    let request = channel_children::setup_request(&snapshot, &workspace);
    let receipt = f
        .adapter
        .run_workspace_setup(&handle, &request)
        .await
        .unwrap();
    let snapshot = commit(
        &mut f,
        &snapshot,
        &ChannelChildEvidence::WorkspaceSetup { request, receipt },
    )
    .await;
    let evidence = ChannelChildEvidence::AgentAdmission {
        capabilities: crate::sandbox::runtime_capabilities(f.adapter.capabilities()),
    };
    let old = snapshot.clone();
    let snapshot = commit(&mut f, &snapshot, &evidence).await;
    assert_eq!(snapshot.state.phase, ChannelProvisioningPhase::AgentStart);
    assert!(f
        .allocations
        .commit_channel_child(&f.authority, &f.lease, &old, &evidence)
        .await
        .is_err());
    let target = f.record.placement.target();
    let agent = f
        .resources
        .get_agent(&target.agent_id)
        .await
        .unwrap()
        .unwrap();
    let worktree = f
        .resources
        .get_worktree(&target.worktree_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(agent.status, AgentStatus::Starting);
    assert_eq!(agent.work.step, AgentWorkStep::None);
    assert!(agent.conversation.conversation_id.is_none());
    assert_eq!(worktree.status, WorktreeStatus::Creating);
    assert_eq!(worktree.base_commit, actual_commit);
    let database = f.client.database(&f.database);
    for collection in [
        "workspaces",
        "worktrees",
        "agents",
        "agent_creation_provenance",
        "agent_log",
    ] {
        assert_eq!(
            database
                .collection::<Document>(collection)
                .count_documents(doc! {})
                .await
                .unwrap(),
            1,
            "{collection}"
        );
    }
    let document = database
        .collection::<Document>("agent_creation_provenance")
        .find_one(doc! {"_id":agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    let provenance = agent_provenance::read(&document).unwrap();
    provenance
        .validate_against(&agent, &snapshot.sandbox)
        .unwrap();
    assert!(
        matches!(provenance.creation_origin, AgentCreationOrigin::ChannelBridge { ref bridge_id, ref partition_id, generation, ref source_reference }
        if bridge_id == f.record.identity.bridge_id.as_str() && partition_id == f.record.identity.key.hash()
            && generation == f.record.identity.generation && source_reference == f.record.first_source.inbox_id())
    );
    assert!(f
        .resources
        .due_workspace_work("2999-01-01T00:00:00Z", 10)
        .await
        .unwrap()
        .is_empty());
    assert!(f
        .resources
        .due_agent_work("2999-01-01T00:00:00Z", 10)
        .await
        .unwrap()
        .is_empty());
    let reopened = MongoBridgeAllocationStore::open(f.bridges.clone(), None, &f.database)
        .await
        .unwrap();
    let workspace = reopened.channel_workspace(&snapshot).await.unwrap();
    assert_eq!(workspace.status, WorkspaceStatus::Ready);
    assert_eq!(workspace.next_worktree_number, 2);
    assert_eq!(
        f.backend.snapshot(&handle).await.unwrap().workspaces.len(),
        1
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_child_prepare_rejects_uncommitted_or_substituted_runtime_facts_without_progress() {
    let mut f = Fixture::new().await;
    let snapshot = f.children().await;
    let snapshot = commit(&mut f, &snapshot, &ChannelChildEvidence::WorkspaceAdmission).await;
    let workspace = f.allocations.channel_workspace(&snapshot).await.unwrap();
    let request = channel_children::prepare_request(&snapshot, &workspace);
    let handle = RuntimeHandle::new(&snapshot.sandbox.runtime.as_ref().unwrap().runtime_id);
    let receipt = f
        .adapter
        .prepare_workspace(&handle, &request)
        .await
        .unwrap();
    for field in ["commit", "path", "operation", "request", "workspace"] {
        let mut wrong = receipt.clone();
        match field {
            "commit" => wrong.resolved_commit = "uncommitted".into(),
            "path" => wrong.path = "/home/cma/foreign".into(),
            "operation" => wrong.operation_id = "replacement".into(),
            "request" => wrong.request_sha256 = "f".repeat(64),
            _ => wrong.workspace_id = "foreign".into(),
        }
        assert!(
            matches!(
                f.allocations
                    .commit_channel_child(
                        &f.authority,
                        &f.lease,
                        &snapshot,
                        &ChannelChildEvidence::WorkspacePrepare {
                            request: request.clone(),
                            receipt: wrong
                        }
                    )
                    .await,
                Err(AllocationError::IdentityMismatch)
            ),
            "{field}"
        );
    }
    assert_eq!(
        f.allocations.channel_workspace(&snapshot).await.unwrap(),
        workspace
    );
    assert_eq!(
        f.client
            .database(&f.database)
            .collection::<Document>("agents")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_child_creation_refuses_removed_human_grant_before_any_child_insert() {
    let mut f = Fixture::new().await;
    let snapshot = f.children().await;
    let database = f.client.database(&f.database);
    let changed = database
        .collection::<Document>("bridge_credentials")
        .update_one(
            doc! {"_id":snapshot.allocation.config.connector.custody.as_str()},
            doc! {"$unset":{"control_plane":""}},
        )
        .await
        .unwrap();
    assert_eq!(changed.matched_count, 1);
    assert!(matches!(
        f.allocations
            .commit_channel_child(
                &f.authority,
                &f.lease,
                &snapshot,
                &ChannelChildEvidence::WorkspaceAdmission
            )
            .await,
        Err(AllocationError::AuthorityUnavailable)
    ));
    for collection in [
        "workspaces",
        "worktrees",
        "agents",
        "agent_creation_provenance",
    ] {
        assert_eq!(
            database
                .collection::<Document>(collection)
                .count_documents(doc! {})
                .await
                .unwrap(),
            0
        );
    }
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_child_provider_work_refuses_suspension_after_snapshot_before_prepare_or_setup() {
    for phase in [
        ChannelProvisioningPhase::WorkspacePrepare,
        ChannelProvisioningPhase::WorkspaceSetup,
    ] {
        let mut f = Fixture::new().await;
        let snapshot = f.children().await;
        let mut snapshot =
            commit(&mut f, &snapshot, &ChannelChildEvidence::WorkspaceAdmission).await;
        if phase == ChannelProvisioningPhase::WorkspaceSetup {
            let workspace = f.allocations.channel_workspace(&snapshot).await.unwrap();
            let request = channel_children::prepare_request(&snapshot, &workspace);
            let handle = RuntimeHandle::new(&snapshot.sandbox.runtime.as_ref().unwrap().runtime_id);
            let receipt = f
                .adapter
                .prepare_workspace(&handle, &request)
                .await
                .unwrap();
            snapshot = commit(
                &mut f,
                &snapshot,
                &ChannelChildEvidence::WorkspacePrepare { request, receipt },
            )
            .await;
        }
        assert_eq!(snapshot.state.phase, phase);
        let before = f.backend.requests().await;
        let workspace = f.allocations.channel_workspace(&snapshot).await.unwrap();
        // The captured local proof and human lease remain live. Only the fresh
        // transactional current-state check can observe this suspension.
        let bridge = f
            .bridges
            .bridge(&f.record.pin.bridge_id)
            .await
            .unwrap()
            .unwrap();
        f.svc
            .mutate(
                &caller(),
                None,
                change(
                    &bridge.id,
                    bridge.revision,
                    "child-provider-suspend",
                    BridgeChange::Disable {
                        accepted_work: AcceptedWorkPolicy::SuspendOriginal,
                    },
                ),
            )
            .await
            .unwrap();
        f.authority.require_record(&snapshot.allocation).unwrap();
        let driver = channel_children::ChannelChildDriver {
            store: f.allocations.clone(),
            credentials: Arc::new(VerifiedHuman),
            adapter: f.adapter.clone(),
        };
        assert!(driver
            .advance(&f.authority, &f.lease, &snapshot)
            .await
            .is_err());
        assert_eq!(f.backend.requests().await, before, "{phase:?}");
        assert_eq!(
            f.allocations.channel_workspace(&snapshot).await.unwrap(),
            workspace
        );
        f.close().await;
    }
}
