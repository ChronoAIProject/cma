//! Causal Mongo and dedicated-worker evidence; runtime IO is explicitly simulated.
use super::{dispatch_fixture::DispatchFixture, *};
use crate::bridge::channel_scheduler::ChannelWorkPort;
use layer2_core::bridge::cleanup::*;
use layer3_adapter::SandboxAdapter;
use std::sync::atomic::Ordering;
#[path = "workspace_cleanup_adapter.rs"]
mod adapter;
#[path = "workspace_cleanup_fixture.rs"]
mod fixture;
use fixture::WorkspaceFixture;
const JOBS: &str = "owned_workspace_cleanup_jobs_v1";
const STATES: &str = "owned_workspace_cleanup_tombstones_v1";
const BARRIER: &str = "owned_workspace_cleanup_admission_v1";

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn owned_workspace_cleanup_separate_consent_monotonic_worker_and_exact_release() {
    let f = WorkspaceFixture::new(false).await;
    let child = f.d.status().await;
    let parent = f.d.f.f.sandbox().await;
    let sibling = f.d.f.row("agents", f.d.f.f.f.sibling.id.as_str()).await;
    assert_eq!(f.workspace().await.status, WorkspaceStatus::Ready);
    assert!(!f.release_proof().await.unwrap());
    assert!(f.worker().discover(None).await.unwrap().is_empty());
    assert!(f.status().await.unwrap().eligible);
    let accepted = f.submit("separate-workspace-consent").await.unwrap();
    let id = &accepted.receipt.as_ref().unwrap().id;
    assert_eq!(accepted.progress, WorkspaceCleanupProgress::Requested);
    assert_eq!(f.adapter.calls.load(Ordering::SeqCst), 0);
    for progress in [
        WorkspaceCleanupProgress::AdmissionClosed,
        WorkspaceCleanupProgress::RemovalStarted,
        WorkspaceCleanupProgress::RuntimeRemoved,
        WorkspaceCleanupProgress::Complete,
    ] {
        f.advance(id, 1).await;
        let status = f.status().await.unwrap();
        assert_eq!(status.progress, progress);
        assert_eq!(status.receipt, accepted.receipt);
        assert_eq!(
            status.workspace_capacity_released,
            progress == WorkspaceCleanupProgress::Complete
        );
        assert!(status.parent_retained && status.history_retained);
        assert_eq!(f.d.status().await, child);
        assert_eq!(f.d.f.f.sandbox().await, parent);
        assert_eq!(
            f.d.f.row("agents", f.d.f.f.f.sibling.id.as_str()).await,
            sibling
        );
    }
    assert_eq!(f.workspace().await.status, WorkspaceStatus::Removed);
    assert!(f.release_proof().await.unwrap());
    assert_eq!(f.adapter.calls.load(Ordering::SeqCst), 1);
    let replay = f.submit("separate-workspace-consent").await.unwrap();
    assert_eq!(replay.receipt, accepted.receipt);
    assert_eq!(replay.progress, WorkspaceCleanupProgress::Complete);
    assert!(f.submit("replacement-key").await.is_err());
    let mut changed = f.request.clone();
    changed.expected_workspace_revision += 1;
    assert!(f
        .submit_request("separate-workspace-consent", changed)
        .await
        .is_err());
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn owned_workspace_cleanup_lost_response_retries_same_canonical_operation() {
    let f = WorkspaceFixture::new(false).await;
    let id = f
        .submit("lost-workspace-response")
        .await
        .unwrap()
        .receipt
        .unwrap()
        .id;
    f.advance(&id, 2).await;
    f.adapter.lose_response.store(true, Ordering::SeqCst);
    f.advance(&id, 1).await;
    assert_eq!(
        f.status().await.unwrap().progress,
        WorkspaceCleanupProgress::RemovalStarted
    );
    assert!(!f.release_proof().await.unwrap());
    assert!(f
        .d
        .f
        .row(
            "sandboxes",
            f.d.f.f.record.placement.target().sandbox_id.as_str()
        )
        .await
        .contains_key(CLEANUP_EXECUTION_FIELD));
    assert!(f.plan().await.is_err());
    f.advance(&id, 2).await;
    assert_eq!(
        f.status().await.unwrap().progress,
        WorkspaceCleanupProgress::Complete
    );
    let requests = f.adapter.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0], requests[1]);
    assert_eq!(
        requests[0].operation_id,
        format!(
            "workspace:{}:remove:v1",
            f.d.f.f.record.placement.target().workspace_id
        )
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn owned_workspace_cleanup_successor_ack_cannot_publish_absence_or_release() {
    let f = WorkspaceFixture::new(false).await;
    let id = f.submit("successor-ack").await.unwrap().receipt.unwrap().id;
    f.advance(&id, 2).await;
    f.adapter.successor.store(true, Ordering::SeqCst);
    f.advance(&id, 1).await;
    let status = f.status().await.unwrap();
    assert_eq!(status.progress, WorkspaceCleanupProgress::RemovalStarted);
    assert_eq!(status.holds, [WorkspaceCleanupHold::EvidenceInconsistent]);
    assert_eq!(f.workspace().await.status, WorkspaceStatus::Ready);
    assert!(!f.release_proof().await.unwrap());
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn owned_workspace_cleanup_reused_workspace_and_missing_creation_are_refused() {
    let shared = WorkspaceFixture::new(true).await;
    assert!(shared
        .status()
        .await
        .unwrap()
        .holds
        .contains(&WorkspaceCleanupHold::WorkspaceNotOwned));
    assert!(shared.submit("never-remove-shared").await.is_err());
    assert!(shared.worker().discover(None).await.unwrap().is_empty());
    shared.close().await;
    let f = WorkspaceFixture::new(false).await;
    let workspace = f.workspace().await;
    f.d.f
        .db()
        .collection::<Document>("workspaces")
        .update_one(
            doc! {"_id":workspace.id.as_str()},
            doc! {"$unset":{"channel_reuse_creation":""}},
        )
        .await
        .unwrap();
    assert!(f.submit("missing-creation").await.is_err());
    assert!(f.worker().discover(None).await.unwrap().is_empty());
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn owned_workspace_cleanup_closure_fences_ordinary_and_prepared_admission() {
    let f = WorkspaceFixture::new(false).await;
    let id = f
        .submit("close-all-admission")
        .await
        .unwrap()
        .receipt
        .unwrap()
        .id;
    f.advance(&id, 1).await;
    let resources = &f.d.f.f.f.original.resources;
    let workspace = f.workspace().await;
    let ordinary = f.manual(&workspace, "after-workspace-closure");
    assert!(resources.admit_agent(ordinary.clone()).await.is_err());
    assert!(resources
        .admit_agent_reusing_profile(ordinary)
        .await
        .is_err());
    let mut prepared = f.manual(&workspace, "trigger-after-closure");
    prepared.origin = AgentOrigin::Trigger {
        trigger_id: "trg_workspace_cleanup".into(),
        launch_id: "lch_workspace_cleanup".into(),
    };
    assert!(resources
        .admit_agent_reusing_profile(prepared)
        .await
        .is_err());
    assert!(f
        .d
        .f
        .row("workspaces", workspace.id.as_str())
        .await
        .contains_key(BARRIER));
    f.advance(&id, 3).await;
    let mut successor = f.workspace().await;
    successor.id = layer2_core::WorkspaceId::generate();
    successor.status = WorkspaceStatus::Preparing;
    successor.work =
        WorkspaceWorkState::new(WorkspaceWorkStep::Prepare, layer2_core::now_rfc3339());
    successor.revision = 1;
    successor.created_at = layer2_core::now_rfc3339();
    successor.removed_at = None;
    successor.ready_at = None;
    successor.resolved_commit = None;
    assert!(resources
        .admit_workspace(WorkspaceAdmission {
            workspace: successor,
            kind: WorkspaceAdmissionKind::Manual,
            event_at: layer2_core::now_rfc3339()
        })
        .await
        .is_err());
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn owned_workspace_cleanup_child_admitted_before_closure_holds_without_effect() {
    let f = WorkspaceFixture::new(false).await;
    let id = f
        .submit("inventory-race")
        .await
        .unwrap()
        .receipt
        .unwrap()
        .id;
    let worker = f.worker();
    let (reached, resume) = f.d.authority.pause_once();
    let workspace = f.workspace().await;
    let admission = f.manual(&workspace, "wins-before-closure");
    let result = tokio::join!(worker.run_one(&id), async {
        reached.notified().await;
        let admitted = f.d.f.f.f.original.resources.admit_agent(admission).await;
        resume.notify_one();
        admitted
    });
    assert!(result.0.is_ok());
    assert!(result.1.is_ok());
    assert_eq!(
        f.status().await.unwrap().progress,
        WorkspaceCleanupProgress::Requested
    );
    assert_eq!(f.adapter.calls.load(Ordering::SeqCst), 0);
    assert!(!f
        .d
        .f
        .row("workspaces", workspace.id.as_str())
        .await
        .contains_key(BARRIER));
    drop(worker);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn owned_workspace_cleanup_atomic_restore_plan_and_corrupt_reverse_evidence() {
    let f = WorkspaceFixture::new(false).await;
    let id = f.submit("restore-plan").await.unwrap().receipt.unwrap().id;
    assert!(f.plan().await.unwrap().workspaces.is_empty());
    f.advance(&id, 4).await;
    let plan = f.plan().await.unwrap();
    assert_eq!(plan, f.plan().await.unwrap());
    assert_eq!(plan.agents.len(), 1);
    assert_eq!(plan.workspaces.len(), 1);
    assert_eq!(plan.agents[0].workspace_id, plan.workspaces[0].workspace_id);
    assert_eq!(plan.workspaces[0].cleanup_receipt_id, id);
    let workspace = f.workspace().await;
    let workspace_row = f.d.f.row("workspaces", workspace.id.as_str()).await;
    for (collection, key) in [
        (STATES, id.clone()),
        ("owned_workspace_cleanup_consents_v1", id.clone()),
        (
            "owned_workspace_cleanup_audits_v1",
            format!("{id}:revision:5"),
        ),
        (
            "channel_child_cleanup_jobs_v1",
            f.request.child_cleanup_receipt_id.clone(),
        ),
    ] {
        let saved = f.d.f.row(collection, &key).await;
        f.d.f
            .db()
            .collection::<Document>(collection)
            .delete_one(doc! {"_id":&key})
            .await
            .unwrap();
        assert!(
            f.plan().await.is_err(),
            "missing {collection} must block restore"
        );
        assert!(
            f.release_proof().await.is_err(),
            "missing {collection} must block capacity"
        );
        f.d.f
            .db()
            .collection::<Document>(collection)
            .insert_one(saved)
            .await
            .unwrap();
    }
    f.d.f
        .db()
        .collection::<Document>("workspaces")
        .update_one(
            doc! {"_id":workspace.id.as_str()},
            doc! {"$unset":{BARRIER:""}},
        )
        .await
        .unwrap();
    assert!(f.plan().await.is_err());
    f.d.f
        .db()
        .collection::<Document>("workspaces")
        .replace_one(doc! {"_id":workspace.id.as_str()}, workspace_row)
        .await
        .unwrap();
    assert_eq!(f.plan().await.unwrap(), plan);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn owned_workspace_cleanup_unavailable_authority_and_old_runtime_hold_before_effect() {
    let f = WorkspaceFixture::new(false).await;
    let id = f
        .submit("capability-hold")
        .await
        .unwrap()
        .receipt
        .unwrap()
        .id;
    f.d.authority.available.store(false, Ordering::SeqCst);
    f.advance(&id, 1).await;
    assert_eq!(
        f.status().await.unwrap().holds,
        [WorkspaceCleanupHold::AuthorityUnavailable]
    );
    f.d.authority.available.store(true, Ordering::SeqCst);
    f.adapter.unsupported.store(true, Ordering::SeqCst);
    f.advance(&id, 1).await;
    assert_eq!(
        f.status().await.unwrap().holds,
        [WorkspaceCleanupHold::RuntimeUnsupported]
    );
    assert_eq!(f.adapter.calls.load(Ordering::SeqCst), 0);
    assert!(!f
        .d
        .f
        .row("workspaces", f.workspace().await.id.as_str())
        .await
        .contains_key(BARRIER));
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn owned_workspace_cleanup_claim_expiry_blocks_io_and_terminal_proof_recovers_without_authority(
) {
    let f = WorkspaceFixture::new(false).await;
    let id = f.submit("claim-fences").await.unwrap().receipt.unwrap().id;
    let (old, _, _) = f
        .store()
        .claim_workspace_cleanup_job(&id, "old-claimant")
        .await
        .unwrap();
    let stale = old.lease.clone().unwrap();
    tokio::time::sleep(Duration::from_millis(10)).await;
    let mut row = f.d.f.row(JOBS, &id).await;
    let mut job: WorkspaceCleanupJob = crate::store::sandbox_mongo::read_record(&row).unwrap();
    job.lease.as_mut().unwrap().expires_at_ms =
        mongodb::bson::DateTime::now().timestamp_millis() - 1;
    row.insert(
        "lease_expires_at_ms",
        job.lease.as_ref().unwrap().expires_at_ms,
    );
    row.insert(
        "record",
        crate::store::sandbox_mongo::record_bson(&job).unwrap(),
    );
    f.d.f
        .db()
        .collection::<Document>(JOBS)
        .replace_one(doc! {"_id":&id}, row)
        .await
        .unwrap();
    assert!(f
        .store()
        .observe_workspace_cleanup_job(&id, &stale, WorkspaceCleanupProgress::AdmissionClosed, &[])
        .await
        .is_err());
    assert_eq!(f.adapter.calls.load(Ordering::SeqCst), 0);
    f.advance(&id, 3).await;
    assert_eq!(
        f.status().await.unwrap().progress,
        WorkspaceCleanupProgress::RuntimeRemoved
    );
    f.due(&id).await;
    let (claimed, _, _) = f
        .store()
        .claim_workspace_cleanup_job(&id, "lost-final-publication")
        .await
        .unwrap();
    let claim = claimed.lease.as_ref().unwrap();
    let permit = crate::bridge::provisioning::workspace_cleanup::WorkspaceCleanupAuthority::bind(
        f.d.f.permit().await,
        &id,
        claim,
    )
    .unwrap();
    let sandbox = &permit.common().parent().lineage.sandbox;
    let support = f
        .adapter
        .workspace_removal_support(&layer3_adapter::RuntimeHandle(
            sandbox.runtime.as_ref().unwrap().runtime_id.clone(),
        ))
        .await
        .unwrap();
    let terminal = f
        .store()
        .step_workspace_cleanup(&permit, &support, None)
        .await
        .unwrap();
    assert_eq!(terminal.progress, WorkspaceCleanupProgress::Complete);
    assert_eq!(
        f.status().await.unwrap().progress,
        WorkspaceCleanupProgress::RuntimeRemoved
    );
    let calls = f.adapter.calls.load(Ordering::SeqCst);
    f.d.authority.available.store(false, Ordering::SeqCst);
    let complete = f
        .store()
        .reconcile_workspace_cleanup_job(&id, claim)
        .await
        .unwrap();
    assert_eq!(complete.progress, WorkspaceCleanupProgress::Complete);
    assert_eq!(f.adapter.calls.load(Ordering::SeqCst), calls);
    assert!(f.release_proof().await.unwrap());
    drop(permit);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn owned_workspace_cleanup_extra_historical_child_and_workspace_alias_block_consent() {
    let f = WorkspaceFixture::new(false).await;
    let original = &f.d.f.f.f.original;
    let mut agent = original
        .resources
        .get_agent(&f.d.f.f.record.placement.target().agent_id)
        .await
        .unwrap()
        .unwrap();
    agent.id = layer2_core::AgentId::generate();
    agent.worktree_id = layer2_core::WorktreeId::generate();
    agent.creation_key = "historical-extra-owned-workspace-child".into();
    let mut row = crate::store::agents::agent_document(&agent).unwrap();
    row.insert("last_seq", agent.conversation.last_seq as i64);
    original.resources.agents.insert_one(row).await.unwrap();
    assert!(f
        .status()
        .await
        .unwrap()
        .holds
        .contains(&WorkspaceCleanupHold::OtherWorkspaceChildren));
    assert!(f.submit("extra-history").await.is_err());
    original
        .resources
        .agents
        .delete_one(doc! {"_id":agent.id.as_str()})
        .await
        .unwrap();
    let mut alias = f.workspace().await;
    alias.id = layer2_core::WorkspaceId::generate();
    alias.status = WorkspaceStatus::Removed;
    alias.removed_at = Some(layer2_core::now_rfc3339());
    original
        .resources
        .workspaces
        .insert_one(crate::store::workspaces::workspace_document(&alias).unwrap())
        .await
        .unwrap();
    assert!(f.submit("same-path-successor").await.is_err());
    assert!(f.worker().discover(None).await.unwrap().is_empty());
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn owned_workspace_cleanup_releases_only_its_retired_reservation_at_real_capacity_boundary() {
    let f = WorkspaceFixture::new(false).await;
    let next = f
        .store()
        .current(&f.d.f.f.record.identity)
        .await
        .unwrap()
        .unwrap();
    let next = f.d.f.f.f.drive_ready(next).await;
    assert_eq!(
        f.d.f.f.f.reset(&next, "67890").await.err(),
        Some(AllocationError::Capacity)
    );
    let id = f
        .submit("release-one-workspace-slot")
        .await
        .unwrap()
        .receipt
        .unwrap()
        .id;
    f.advance(&id, 3).await;
    assert!(!f.release_proof().await.unwrap());
    // Actual capacity is checked before the in-flight parent exclusion.
    assert_eq!(
        f.d.f.f.f.reset(&next, "67890").await.err(),
        Some(AllocationError::Capacity)
    );
    f.advance(&id, 1).await;
    let replacement = f.d.f.f.f.reset(&next, "67890").await.unwrap();
    assert_ne!(
        replacement.placement.target().workspace_id,
        f.d.f.f.record.placement.target().workspace_id
    );
    assert_eq!(f.workspace().await.status, WorkspaceStatus::Removed);
    assert!(f.release_proof().await.unwrap());
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn owned_workspace_cleanup_expired_claim_after_runtime_io_cannot_publish_absence() {
    let f = WorkspaceFixture::new(false).await;
    let accepted = f.submit("expire-after-io").await.unwrap();
    let id = accepted.receipt.as_ref().unwrap().id.clone();
    f.advance(&id, 2).await;
    let before = f.d.f.row(STATES, &id).await;
    let (reached, resume) = f.adapter.pause_after_removal_once();
    f.due(&id).await;
    let worker = f.worker();
    let work_id = id.clone();
    let running = tokio::spawn(async move { worker.run_one(&work_id).await });
    tokio::time::timeout(Duration::from_secs(10), reached.notified())
        .await
        .unwrap();
    let mut row = f.d.f.row(JOBS, &id).await;
    let mut job: WorkspaceCleanupJob = crate::store::sandbox_mongo::read_record(&row).unwrap();
    // Preserve a structurally valid historical claim, but expire it before the
    // returning runtime result is allowed to enter a publication transaction.
    tokio::time::sleep(Duration::from_millis(10)).await;
    job.lease.as_mut().unwrap().expires_at_ms =
        mongodb::bson::DateTime::now().timestamp_millis() - 1;
    row.insert(
        "lease_expires_at_ms",
        job.lease.as_ref().unwrap().expires_at_ms,
    );
    row.insert(
        "record",
        crate::store::sandbox_mongo::record_bson(&job).unwrap(),
    );
    f.d.f
        .db()
        .collection::<Document>(JOBS)
        .replace_one(doc! {"_id":&id}, row)
        .await
        .unwrap();
    resume.notify_one();
    assert!(running.await.unwrap().is_err());
    assert_eq!(f.adapter.calls.load(Ordering::SeqCst), 1);
    let after = f.d.f.row(STATES, &id).await;
    assert_eq!(after.get("record"), before.get("record"));
    let status = f.status().await.unwrap();
    assert_eq!(status.progress, WorkspaceCleanupProgress::RemovalStarted);
    assert_eq!(status.receipt, accepted.receipt);
    assert!(!f.release_proof().await.unwrap());
    assert!(f.plan().await.is_err());
    f.advance(&id, 2).await;
    assert_eq!(
        f.status().await.unwrap().progress,
        WorkspaceCleanupProgress::Complete
    );
    let requests = f.adapter.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0], requests[1]);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn owned_workspace_cleanup_spare_capacity_and_competing_closure_abort_all_reservation_writes()
{
    use futures_util::TryStreamExt;
    let f = WorkspaceFixture::with_capacity(false, 8).await;
    let (proposal, authority) = f.d.f.f.f.proposal("spare-capacity-race").await;
    let write = AllocationReservation::new(proposal, authority, now()).unwrap();
    let mut session = f.d.f.f.f.original.client.start_session().await.unwrap();
    crate::store::sandbox_mongo::start_transaction(&mut session)
        .await
        .unwrap();
    // Prove this exact reservation has spare capacity before cleanup closes.
    let admitted = f
        .store()
        .reserve_in_session(&write, &mut session)
        .await
        .unwrap();
    session.abort_transaction().await.unwrap();
    let id = f
        .submit("competing-reservation")
        .await
        .unwrap()
        .receipt
        .unwrap()
        .id;
    crate::store::sandbox_mongo::start_transaction(&mut session)
        .await
        .unwrap();
    f.d.f
        .db()
        .collection::<Document>("sandboxes")
        .find_one(doc! {"_id":f.d.f.f.record.placement.target().sandbox_id.as_str()})
        .session(&mut session)
        .await
        .unwrap()
        .unwrap();
    f.advance(&id, 2).await;
    let stale = f.store().reserve_in_session(&write, &mut session).await;
    assert!(
        matches!(stale, Err(crate::store::bridge_allocations::MutationError::Mongo(error))
        if crate::store::sandbox_mongo::is_transient(&error))
    );
    session.abort_transaction().await.unwrap();
    let names = [
        "bridge_allocations",
        "bridge_allocation_heads",
        "sandboxes",
        "workspaces",
        "resource_policies",
    ];
    let mut before = Vec::new();
    for name in names {
        before.push(
            f.d.f
                .db()
                .collection::<Document>(name)
                .find(doc! {})
                .sort(doc! {"_id":1})
                .await
                .unwrap()
                .try_collect::<Vec<_>>()
                .await
                .unwrap(),
        );
    }
    // The fresh transaction reaches the final cleanup guard with spare
    // capacity, then aborts its pending reservation/head and all authority fences.
    assert_eq!(
        f.store().reserve(write).await.err(),
        Some(AllocationError::Conflict)
    );
    for (name, rows) in names.into_iter().zip(before) {
        let after =
            f.d.f
                .db()
                .collection::<Document>(name)
                .find(doc! {})
                .sort(doc! {"_id":1})
                .await
                .unwrap()
                .try_collect::<Vec<_>>()
                .await
                .unwrap();
        assert_eq!(after, rows, "aborted reservation changed {name}");
    }
    assert!(f
        .store()
        .generation(&admitted.identity)
        .await
        .unwrap()
        .is_none());
    assert_eq!(f.adapter.calls.load(Ordering::SeqCst), 0);
    assert!(!f.release_proof().await.unwrap());
    f.close().await;
}
