//! Real ordinary admission consumes the shared reserve while exact Channel
//! children are still pending. No raw writes imitate admission or capacity.
use super::*;
use layer2_core::sandbox::store::{
    AgentAdmission, AgentAdmissionStore, SandboxStoreError, WorkspaceAdmission,
    WorkspaceAdmissionKind, WorkspaceAdmissionStore,
};

#[path = "owner_reserve/coordination.rs"]
mod coordination;

#[derive(Debug, PartialEq, Eq)]
enum OrdinaryChild {
    Workspace(Box<Workspace>),
    Agent(Box<Agent>),
}

#[derive(Clone)]
enum OrdinaryRequest {
    Workspace(Box<WorkspaceAdmission>),
    Agent(Box<AgentAdmission>),
}

fn request(f: &ReuseFixture, shared_workspace: bool, trigger: bool, key: &str) -> OrdinaryRequest {
    if shared_workspace {
        OrdinaryRequest::Agent(Box::new(AgentAdmission {
            created_by: Some(caller().actor),
            owner: f.workspace.owner.clone(),
            workspace_id: f.workspace.id.clone(),
            agent_id: layer2_core::AgentId::generate(),
            worktree_id: layer2_core::WorktreeId::generate(),
            profile: f.sibling.profile.clone(),
            capabilities: f.sibling.capabilities.clone(),
            title: None,
            first_message: None,
            origin: if trigger {
                AgentOrigin::Trigger {
                    trigger_id: format!("trg_{key}"),
                    launch_id: format!("lch_{key}"),
                }
            } else {
                AgentOrigin::Manual
            },
            creation_key: key.into(),
            event_at: layer2_core::now_rfc3339(),
        }))
    } else {
        let stamp = layer2_core::now_rfc3339();
        let candidate = Workspace {
            id: layer2_core::WorkspaceId::generate(),
            name: key.into(),
            path: format!("/home/cma/{key}"),
            origin: if trigger {
                WorkspaceOrigin::Trigger {
                    launch_id: format!("lch_{key}"),
                }
            } else {
                WorkspaceOrigin::Manual
            },
            resolved_commit: None,
            status: WorkspaceStatus::Preparing,
            work: WorkspaceWorkState::new(WorkspaceWorkStep::Prepare, stamp.clone()),
            next_worktree_number: 1,
            created_at: stamp.clone(),
            ready_at: None,
            failed: None,
            removed_at: None,
            revision: 1,
            ..f.workspace.clone()
        };
        OrdinaryRequest::Workspace(Box::new(WorkspaceAdmission {
            workspace: candidate,
            kind: if trigger {
                WorkspaceAdmissionKind::Trigger
            } else {
                WorkspaceAdmissionKind::Manual
            },
            event_at: stamp,
        }))
    }
}

async fn admit(
    f: &ReuseFixture,
    request: &OrdinaryRequest,
) -> Result<OrdinaryChild, SandboxStoreError> {
    match request {
        OrdinaryRequest::Agent(request) => {
            let created = f
                .original
                .resources
                .admit_agent(request.as_ref().clone())
                .await?;
            Ok(OrdinaryChild::Agent(Box::new(
                created.creation().agent.clone(),
            )))
        }
        OrdinaryRequest::Workspace(request) => {
            let result = f
                .original
                .resources
                .admit_workspace(request.as_ref().clone())
                .await?;
            let (layer2_core::sandbox::store::WorkspaceAdmissionResult::Created(workspace)
            | layer2_core::sandbox::store::WorkspaceAdmissionResult::Existing(workspace)) = result;
            Ok(OrdinaryChild::Workspace(Box::new(workspace)))
        }
    }
}

async fn ordinary(
    f: &ReuseFixture,
    shared_workspace: bool,
    trigger: bool,
    key: &str,
) -> Result<OrdinaryChild, SandboxStoreError> {
    admit(f, &request(f, shared_workspace, trigger, key)).await
}

async fn retained(f: &ReuseFixture, child: &OrdinaryChild) {
    match child {
        OrdinaryChild::Workspace(workspace) => assert_eq!(
            f.original
                .resources
                .get_workspace(&workspace.id)
                .await
                .unwrap(),
            Some(workspace.as_ref().clone())
        ),
        OrdinaryChild::Agent(agent) => assert_eq!(
            f.original.resources.get_agent(&agent.id).await.unwrap(),
            Some(agent.as_ref().clone())
        ),
    }
    assert_eq!(
        f.original.resources.get_agent(&f.sibling.id).await.unwrap(),
        Some(f.sibling.clone())
    );
}

async fn reservations(f: &ReuseFixture) -> (AllocationRecord, AllocationRecord) {
    let (one, one_proof) = f.proposal("owner-reserve-one").await;
    let (two, two_proof) = f.proposal("owner-reserve-two").await;
    let (one, two) = tokio::join!(
        f.original
            .allocations
            .reserve(AllocationReservation::new(one, one_proof, now()).unwrap()),
        f.original
            .allocations
            .reserve(AllocationReservation::new(two, two_proof, now()).unwrap())
    );
    (one.unwrap(), two.unwrap())
}

async fn continuation(shared_workspace: bool, trigger: bool) {
    let f = ReuseFixture::new(shared_workspace).await;
    let (mut one, two) = reservations(&f).await;
    let original_one = one.placement.clone();
    let original_two = two.placement.clone();
    if !shared_workspace {
        // Stop at WorkspacePrepare: the next receipt advances a Workspace
        // whose exact reservation and actual row already consume one slot.
        one = f.step(one).await.unwrap();
    }
    let child = ordinary(&f, shared_workspace, trigger, "owner-spare")
        .await
        .expect("the real owner can consume the explicitly reserved spare");
    let next = f.step(one.clone()).await;
    assert!(
        next.is_ok(),
        "advancing an accepted original reservation must not demand a second spare: {:?}",
        next.err()
    );
    let one = f.drive_ready(next.unwrap()).await;
    let two = f.drive_ready(two).await;
    assert_eq!(one.placement, original_one);
    assert_eq!(two.placement, original_two);
    assert_eq!(one.state, AllocationState::Ready);
    assert_eq!(two.state, AllocationState::Ready);
    retained(&f, &child).await;
    let (fresh, proof) = f.proposal("owner-reserve-new").await;
    assert!(matches!(
        f.original
            .allocations
            .reserve(AllocationReservation::new(fresh, proof, now()).unwrap())
            .await,
        Err(AllocationError::Capacity)
    ));
    assert_eq!(
        f.reset(&one, "owner-reserve-one").await.err(),
        Some(AllocationError::Capacity)
    );
    f.original.close().await;
}

async fn pending_reservations(shared_workspace: bool, trigger: bool) {
    let f = ReuseFixture::new(shared_workspace).await;
    let (one, two) = reservations(&f).await;
    let child = ordinary(&f, shared_workspace, trigger, "owner-spare")
        .await
        .expect("reserved ordinary slot remains available before materialization");
    let excessive = ordinary(&f, shared_workspace, trigger, "owner-beyond-spare").await;
    let observed = match &excessive {
        Ok(_) => "created another ordinary child beyond the spare".to_owned(),
        Err(error) => format!("{error:?}"),
    };
    assert!(
        matches!(
            excessive,
            Err(SandboxStoreError::AgentLimitReached { .. })
                | Err(SandboxStoreError::WorkspaceLimitReached { .. })
        ),
        "ordinary admission must account for accepted unmaterialized Channel children; observed: {observed}"
    );
    assert!(f.current(&one).await == one);
    assert!(f.current(&two).await == two);
    retained(&f, &child).await;
    let (one, two) = tokio::join!(f.drive_ready(one), f.drive_ready(two));
    assert_eq!(one.state, AllocationState::Ready);
    assert_eq!(two.state, AllocationState::Ready);
    f.original.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn shared_workspace_manual_spare_allows_original_channel_continuation() {
    continuation(true, false).await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn shared_workspace_trigger_spare_allows_original_channel_continuation() {
    continuation(true, true).await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn shared_sandbox_manual_spare_allows_original_channel_continuation() {
    continuation(false, false).await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn shared_sandbox_trigger_spare_allows_original_channel_continuation() {
    continuation(false, true).await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn shared_workspace_manual_admission_counts_pending_channel_reservations() {
    pending_reservations(true, false).await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn shared_workspace_trigger_admission_counts_pending_channel_reservations() {
    pending_reservations(true, true).await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn shared_sandbox_manual_admission_counts_pending_channel_reservations() {
    pending_reservations(false, false).await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn shared_sandbox_trigger_admission_counts_pending_channel_reservations() {
    pending_reservations(false, true).await;
}
