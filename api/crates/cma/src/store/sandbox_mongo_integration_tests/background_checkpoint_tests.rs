use super::*;
use crate::store::sandbox_maintenance::mongo_integration_tests::{
    active_sandbox, insert_agent, ready_agent,
};
use layer2_core::sandbox::store::{
    SandboxCheckpointMutation, SandboxLifecycleAgentScope, SandboxLifecycleExecutionStore,
    SandboxLifecycleMutation, SandboxMutationGuard,
};
use layer2_core::sandbox::{
    SandboxBackgroundCheckpointPlan, SandboxBackgroundCheckpointProgress, SandboxCheckpointRef,
    SandboxHome,
};
use layer3_adapter::*;
use std::time::Duration;

fn parent() -> Sandbox {
    let mut sandbox = active_sandbox("sbx_background_checkpoint");
    sandbox.work = SandboxWorkState::new(SandboxWorkStep::None, NOW.into());
    let mut home = SandboxHome::reserve(
        &sandbox.id,
        HomeVolumePolicy {
            mode: Default::default(),
            storage_class: "cma-home-v1".into(),
            capacity_gib: 64,
            template_name: "seed".into(),
            template_generation: "seed-uid".into(),
        },
    )
    .unwrap();
    home.bind(HomeVolumeReceipt {
        native_authority: None,
        volume_id: home.request.volume_id.clone(),
        request_sha256: home.request.computed_sha256(),
        storage_reference: "home-pvc".into(),
        storage_generation: "home-uid".into(),
    })
    .unwrap();
    sandbox.lifecycle.home = Some(Box::new(home));
    sandbox
}

fn mutation(current: &Sandbox, next: Sandbox) -> SandboxLifecycleMutation {
    SandboxLifecycleMutation {
        guard: SandboxMutationGuard::capture(current),
        sandbox: next,
        appended_at: NOW.into(),
        agents: vec![],
        agent_scope: SandboxLifecycleAgentScope::Touched,
        events: vec![SandboxEventKind::WorkStepCompleted {
            step: SandboxWorkStep::RunLifecycleOperation,
        }],
    }
}

async fn begin(store: &MongoSandboxStore, current: &Sandbox, agent: &Agent) -> Sandbox {
    let epoch = current.next_checkpoint_epoch().unwrap();
    let plan = SandboxBackgroundCheckpointPlan {
        snapshot: HomeSnapshotRequest {
            capture_id: format!("{}-checkpoint-{epoch}", current.id),
            epoch,
            home: current
                .lifecycle
                .home
                .as_ref()
                .unwrap()
                .attachment()
                .unwrap(),
            snapshot_class: "cma-home-v1".into(),
        },
        runtime: current.runtime.clone().unwrap(),
        conversations: vec![HomeSnapshotConversationIdentity {
            agent_id: agent.id.to_string(),
            conversation_id: agent.conversation.conversation_id.clone().unwrap(),
        }],
        image: format!("registry/runtime@sha256:{}", "a".repeat(64)),
    };
    let mut next = current.clone();
    next.begin_background_checkpoint(plan, NOW.into()).unwrap();
    store
        .commit_sandbox_lifecycle(mutation(current, next.clone()))
        .await
        .unwrap();
    next
}

async fn claim(store: &MongoSandboxStore, current: &mut Sandbox) -> u64 {
    let mut next = current.clone();
    next.claim_background_checkpoint("export-worker".into(), NOW.into(), MINUTE.into())
        .unwrap();
    store
        .commit_sandbox_lifecycle(mutation(current, next.clone()))
        .await
        .unwrap();
    *current = next;
    current
        .lifecycle
        .background_checkpoint
        .as_ref()
        .unwrap()
        .work
        .lease
        .as_ref()
        .unwrap()
        .fence
}

async fn progress(
    store: &MongoSandboxStore,
    current: &mut Sandbox,
    progress: SandboxBackgroundCheckpointProgress,
) {
    let fence = claim(store, current).await;
    let mut next = current.clone();
    next.advance_background_checkpoint(progress, "export-worker", fence, NOW.into(), NOW.into())
        .unwrap();
    store
        .commit_sandbox_lifecycle(mutation(current, next.clone()))
        .await
        .unwrap();
    *current = next;
}

async fn upload_evidence(store: &MongoSandboxStore, current: &mut Sandbox) -> SandboxCheckpointRef {
    let capture = current.lifecycle.background_checkpoint.as_ref().unwrap();
    let request = capture.plan.snapshot.clone();
    let mut next = capture.progress.clone();
    next.snapshot = Some(HomeSnapshotObservation {
        receipt: HomeSnapshotReceipt {
            capture_id: request.capture_id.clone(),
            request_sha256: request.computed_sha256(),
            snapshot_reference: "snapshot".into(),
            snapshot_generation: "snapshot-uid".into(),
        },
        captured_at: Some(NOW.into()),
        ready: true,
    });
    progress(store, current, next.clone()).await;
    let export = current
        .lifecycle
        .background_checkpoint
        .as_ref()
        .unwrap()
        .export_request()
        .unwrap();
    let digest = export.computed_sha256().unwrap();
    next.view = Some(HomeSnapshotViewObservation {
        receipt: HomeSnapshotViewReceipt {
            request_sha256: digest.clone(),
            volume_reference: format!("cma-view-{}", &digest[..52]),
            volume_generation: "view-uid".into(),
        },
        bound_volume: None,
    });
    progress(store, current, next.clone()).await;
    let job = current
        .lifecycle
        .background_checkpoint
        .as_ref()
        .unwrap()
        .job_request()
        .unwrap();
    let digest = job.computed_sha256().unwrap();
    let mut view = next.view.clone().unwrap();
    view.bound_volume = Some("derived-disk".into());
    next.job = Some(HomeSnapshotExportJobObservation {
        receipt: HomeSnapshotExportJobReceipt {
            request_sha256: digest.clone(),
            pod_reference: format!("cma-export-{}", &digest[..48]),
            pod_generation: "pod-uid".into(),
        },
        view,
        phase: HomeSnapshotExportJobPhase::Ready,
    });
    progress(store, current, next.clone()).await;
    let receipt = HomeSnapshotExportReceipt {
        request_sha256: export.computed_sha256().unwrap(),
        checkpoint: CheckpointReceipt {
            epoch: request.epoch,
            object_reference: format!("home-{}.cma", "b".repeat(64)),
            sha256: "b".repeat(64),
            writers: WriterState::Resumed,
        },
        archive_bytes: 42,
        conversations: export
            .conversations
            .iter()
            .cloned()
            .map(|identity| HomeSnapshotExportConversation {
                identity,
                archive_sha256: "c".repeat(64),
            })
            .collect(),
    };
    next.uploaded = Some(receipt.clone());
    progress(store, current, next).await;
    SandboxCheckpointRef::seal_export(&export, &receipt).unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn background_checkpoint_mongo_publishes_without_reading_or_fencing_live_agents() {
    let (client, name) = database("background_publication").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let parent = parent();
    store.create_sandbox(parent.clone()).await.unwrap();
    let agent = ready_agent(&parent, "agt_background");
    insert_agent(&store, &agent, NOW).await;
    let mut current = begin(&store, &parent, &agent).await;
    let checkpoint = upload_evidence(&store, &mut current).await;
    let fence = claim(&store, &mut current).await;
    let mut next = current.clone();
    next.publish_background_checkpoint(checkpoint.clone(), "export-worker", fence, NOW.into())
        .unwrap();
    let publication = SandboxCheckpointMutation {
        guard: SandboxMutationGuard::capture(&current),
        agents: vec![],
        nonblocking: true,
        checkpoint,
        sandbox: next.clone(),
        appended_at: NOW.into(),
    };

    // An uncommitted live Agent write must not conflict with background publication.
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    store
        .agents
        .update_one(
            doc! {"_id": agent.id.as_str()},
            doc! {"$inc": {"checkpoint_publication_fence": 1}},
        )
        .session(&mut session)
        .await
        .unwrap();
    tokio::time::timeout(
        Duration::from_secs(5),
        store.publish_sandbox_checkpoint(publication.clone()),
    )
    .await
    .unwrap()
    .unwrap();
    session.abort_transaction().await.unwrap();
    // Exact replay remains idempotent after reopening the database.
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    reopened
        .publish_sandbox_checkpoint(publication)
        .await
        .unwrap();
    assert_eq!(reopened.get_sandbox(&parent.id).await.unwrap(), Some(next));
    assert_eq!(reopened.get_agent(&agent.id).await.unwrap(), Some(agent));
    assert_eq!(
        store
            .checkpoints
            .count_documents(doc! {"sandbox_id": parent.id.as_str()})
            .await
            .unwrap(),
        1
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn background_checkpoint_mongo_exclusive_claim_and_stale_pause_publication() {
    let (client, name) = database("background_claim").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let parent = parent();
    store.create_sandbox(parent.clone()).await.unwrap();
    let agent = ready_agent(&parent, "agt_background");
    insert_agent(&store, &agent, NOW).await;
    let mut current = begin(&store, &parent, &agent).await;
    let checkpoint = upload_evidence(&store, &mut current).await;
    let mut a = current.clone();
    a.claim_background_checkpoint("a".into(), NOW.into(), MINUTE.into())
        .unwrap();
    let mut b = current.clone();
    b.claim_background_checkpoint("b".into(), NOW.into(), MINUTE.into())
        .unwrap();
    let (a_result, b_result) = tokio::join!(
        store.commit_sandbox_lifecycle(mutation(&current, a)),
        store.commit_sandbox_lifecycle(mutation(&current, b))
    );
    assert_ne!(a_result.is_ok(), b_result.is_ok());
    current = store.get_sandbox(&parent.id).await.unwrap().unwrap();
    let lease = current
        .lifecycle
        .background_checkpoint
        .as_ref()
        .unwrap()
        .work
        .lease
        .as_ref()
        .unwrap();
    let mut published = current.clone();
    published
        .publish_background_checkpoint(checkpoint.clone(), &lease.holder, lease.fence, NOW.into())
        .unwrap();
    let mut pausing = current.clone();
    pausing.begin_pause(NOW.into(), None).unwrap();
    store
        .save_sandbox(current.revision, pausing.clone())
        .await
        .unwrap();
    let publication = SandboxCheckpointMutation {
        guard: SandboxMutationGuard::capture(&current),
        agents: vec![],
        nonblocking: true,
        checkpoint,
        sandbox: published,
        appended_at: NOW.into(),
    };
    assert_eq!(
        store.publish_sandbox_checkpoint(publication).await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(store.get_sandbox(&parent.id).await.unwrap(), Some(pausing));
    assert_eq!(
        store
            .checkpoints
            .count_documents(doc! {"sandbox_id": parent.id.as_str()})
            .await
            .unwrap(),
        0
    );
    client.database(&name).drop().await.unwrap();
}
