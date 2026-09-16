use super::{dispatch_fixture::DispatchFixture, *};
use crate::bridge::channel_scheduler::ChannelWorkPort;
use layer2_core::bridge::cleanup::{ChildCleanupHold as Hold, ChildCleanupProgress as Progress};
use std::sync::atomic::Ordering;

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn cleanup_dispatch_requires_explicit_job_and_advances_one_stage_preserving_siblings() {
    let f = DispatchFixture::new(true).await;
    f.f.request_cleanup().await.unwrap();
    let worker = f.worker("explicit-dispatch");
    assert!(worker.discover(None).await.unwrap().is_empty());
    let parent = f.f.f.sandbox().await;
    let sibling = f.f.row("agents", f.f.f.f.sibling.id.as_str()).await;
    let runtime = f.f.f.f.original.backend.requests().await;
    let accepted = f.submit("remove-exact-child").await.unwrap();
    let id = accepted.receipt.as_ref().unwrap().id.clone();
    assert_eq!(accepted.progress, Progress::Requested);
    assert_eq!(
        worker.discover(None).await.unwrap(),
        std::slice::from_ref(&id)
    );
    assert_eq!(f.f.f.f.original.backend.requests().await, runtime);
    for progress in [
        Progress::AdmissionClosed,
        Progress::RemovalStarted,
        Progress::ProcessClosed,
        Progress::HomeRemoved,
        Progress::WorktreeRemoved,
        Progress::Complete,
    ] {
        f.due(&id).await;
        worker.run_one(&id).await.unwrap();
        let status = f.status().await;
        assert_eq!(status.progress, progress);
        assert_eq!(
            status.agent_capacity_released,
            progress == Progress::Complete
        );
        assert!(status.workspace_retained && status.history_retained);
        assert_eq!(status.receipt, accepted.receipt);
        assert_eq!(f.f.f.sandbox().await, parent);
        assert_eq!(
            f.f.row("agents", f.f.f.f.sibling.id.as_str()).await,
            sibling
        );
    }
    assert!(worker.discover(None).await.unwrap().is_empty());
    assert_eq!(
        f.submit("remove-exact-child").await.unwrap(),
        f.status().await
    );
    drop(worker);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn cleanup_dispatch_revocation_and_pending_commands_hold_without_runtime_effects() {
    let mut base = CleanupFixture::new(true).await;
    base.f.queued("accepted-before-cleanup").await;
    base.reset().await;
    let f = DispatchFixture::from_retired(base);
    let id = f
        .submit("hold-exact-child")
        .await
        .unwrap()
        .receipt
        .unwrap()
        .id;
    let worker = f.worker("held-dispatch");
    let before = f.f.f.f.original.backend.requests().await;
    f.authority.available.store(false, Ordering::SeqCst);
    worker.run_one(&id).await.unwrap();
    let held = f.status().await;
    assert_eq!(held.progress, Progress::Requested);
    assert_eq!(held.holds, [Hold::AuthorityUnavailable]);
    assert!(!f
        .f
        .row("agents", f.f.f.record.placement.target().agent_id.as_str())
        .await
        .contains_key(CLEANUP_ADMISSION_FIELD));
    f.authority.available.store(true, Ordering::SeqCst);
    f.due(&id).await;
    worker.run_one(&id).await.unwrap();
    assert_eq!(f.status().await.progress, Progress::AdmissionClosed);
    assert!(f.status().await.holds.contains(&Hold::PendingCommand));
    f.due(&id).await;
    worker.run_one(&id).await.unwrap();
    assert_eq!(f.status().await.progress, Progress::AdmissionClosed);
    assert!(f.status().await.holds.contains(&Hold::PendingCommand));
    assert_eq!(f.f.f.f.original.backend.requests().await, before);
    assert_eq!(
        f.f.db()
            .collection::<Document>(CLEANUP_TOMBSTONES)
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    drop(worker);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn cleanup_dispatch_two_workers_claim_one_stage_and_stale_claim_cannot_publish() {
    let f = DispatchFixture::new(false).await;
    let id = f.submit("two-workers").await.unwrap().receipt.unwrap().id;
    let left = f.worker("left");
    let right = f.worker("right");
    let before = f.f.f.f.original.backend.requests().await;
    let (reached, resume) = f.authority.pause_once();
    let results = tokio::join!(left.run_one(&id), async {
        reached.notified().await;
        let result = right.run_one(&id).await;
        resume.notify_one();
        result
    });
    assert!(results.0.is_ok());
    assert!(results.1.is_err());
    assert_eq!(f.status().await.progress, Progress::AdmissionClosed);
    assert_eq!(f.authority.calls.load(Ordering::SeqCst), 1);
    assert_eq!(f.f.f.f.original.backend.requests().await, before);
    f.due(&id).await;
    let store = &f.f.f.f.original.allocations;
    let (old, _, _) = store.claim_cleanup_job(&id, "crashed").await.unwrap();
    f.expire_claim(&id).await;
    let (new, _, _) = store.claim_cleanup_job(&id, "replacement").await.unwrap();
    assert!(new.lease_fence > old.lease_fence);
    assert!(store
        .observe_cleanup_job(
            &id,
            old.lease.as_ref().unwrap(),
            Progress::AdmissionClosed,
            &[]
        )
        .await
        .is_err());
    assert_eq!(f.status().await.progress, Progress::AdmissionClosed);
    drop(left);
    drop(right);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn cleanup_dispatch_terminal_proof_recovers_lost_job_publication_without_authority() {
    let f = DispatchFixture::new(false).await;
    let id = f
        .submit("lost-final-publication")
        .await
        .unwrap()
        .receipt
        .unwrap()
        .id;
    let worker = f.worker("terminal-recovery");
    for _ in 0..5 {
        f.due(&id).await;
        worker.run_one(&id).await.unwrap();
    }
    assert_eq!(f.status().await.progress, Progress::WorktreeRemoved);
    // The runtime operation and terminal tombstone commit; only publishing the
    // resulting Complete job row fails. A fresh claim must recover from proofs.
    f.f.db().run_command(doc! {"collMod":"channel_child_cleanup_jobs_v1", "validator":{"complete":{"$ne":true}},"validationLevel":"strict","validationAction":"error"}).await.unwrap();
    f.due(&id).await;
    assert!(worker.run_one(&id).await.is_err());
    let target = f.f.f.record.placement.target();
    assert_eq!(
        f.f.f
            .f
            .original
            .resources
            .get_agent(&target.agent_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        AgentStatus::Deleted
    );
    f.f.db()
        .run_command(doc! {"collMod":"channel_child_cleanup_jobs_v1", "validator":{}})
        .await
        .unwrap();
    f.expire_claim(&id).await;
    f.authority.available.store(false, Ordering::SeqCst);
    let calls = f.authority.calls.load(Ordering::SeqCst);
    let before = f.f.f.f.original.backend.requests().await;
    worker.run_one(&id).await.unwrap();
    assert_eq!(f.authority.calls.load(Ordering::SeqCst), calls);
    assert_eq!(f.f.f.f.original.backend.requests().await, before);
    assert_eq!(f.status().await.progress, Progress::Complete);
    assert!(f.status().await.agent_capacity_released);
    f.f.f.pause().await;
    assert_eq!(f.f.f.sandbox().await.status, SandboxStatus::Paused);
    assert_eq!(f.status().await.progress, Progress::Complete);
    assert!(f.status().await.agent_capacity_released);
    drop(worker);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn cleanup_dispatch_api_principal_is_not_relabelled_as_original_human() {
    use layer2_core::{bridge::cleanup::CleanupError, resource_access::ResourceActor};
    let f = DispatchFixture::new(true).await;
    let mut api = f.f.f.f.original.lease.actor_context().clone();
    api.actor = ResourceActor::Api {
        subject: api.caller.subject.clone(),
        delegation_id: None,
    };
    let before = f.f.f.f.original.backend.requests().await;
    assert!(matches!(
        f.submit_as("actor-fidelity", &api).await,
        Err(CleanupError::OriginalActorRequired)
    ));
    assert_eq!(
        f.f.db()
            .collection::<Document>("channel_child_cleanup_jobs_v1")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    f.f.assert_no_request().await;
    let accepted = f.submit("actor-fidelity").await.unwrap();
    assert_eq!(
        accepted.receipt.unwrap().requested_by,
        f.f.f.f.original.lease.actor_context().actor
    );
    assert_eq!(f.f.f.f.original.backend.requests().await, before);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn cleanup_dispatch_unknown_runtime_operations_replay_exact_identity_and_keep_exclusion() {
    use layer3_adapter::wire::{RequestFrame, WireOperation};
    let f = DispatchFixture::new(true).await;
    let id = f
        .submit("unknown-runtime-step")
        .await
        .unwrap()
        .receipt
        .unwrap()
        .id;
    let worker = f.worker("unknown-runtime");
    for _ in 0..2 {
        f.due(&id).await;
        worker.run_one(&id).await.unwrap();
    }
    let backend = &f.f.f.f.original.backend;
    for (operation, prior, next) in [
        (
            WireOperation::CloseAgent,
            Progress::RemovalStarted,
            Progress::ProcessClosed,
        ),
        (
            WireOperation::RemoveAgentHome,
            Progress::ProcessClosed,
            Progress::HomeRemoved,
        ),
        (
            WireOperation::RemoveWorktree,
            Progress::HomeRemoved,
            Progress::WorktreeRemoved,
        ),
    ] {
        backend.lose_next_response(operation);
        f.due(&id).await;
        worker.run_one(&id).await.unwrap();
        let held = f.status().await;
        assert_eq!(held.progress, prior);
        assert_eq!(held.holds, [Hold::EvidenceUnavailable]);
        assert!(!held.agent_capacity_released);
        assert!(f
            .f
            .row("sandboxes", held.target.sandbox_id.as_str())
            .await
            .contains_key(CLEANUP_EXECUTION_FIELD));
        f.due(&id).await;
        worker.run_one(&id).await.unwrap();
        assert_eq!(f.status().await.progress, next);
    }
    let requests = backend.requests().await;
    for operation in [
        WireOperation::CloseAgent,
        WireOperation::RemoveAgentHome,
        WireOperation::RemoveWorktree,
    ] {
        let matching: Vec<_> = requests
            .iter()
            .filter(|(_, request)| {
                matches!(
                    (operation, request),
                    (WireOperation::CloseAgent, RequestFrame::CloseAgent(_))
                        | (
                            WireOperation::RemoveAgentHome,
                            RequestFrame::RemoveAgentHome(_)
                        )
                        | (
                            WireOperation::RemoveWorktree,
                            RequestFrame::RemoveWorktree(_)
                        )
                )
            })
            .collect();
        assert_eq!(matching.len(), 2);
        assert_eq!(matching[0], matching[1]);
    }
    f.due(&id).await;
    worker.run_one(&id).await.unwrap();
    assert_eq!(f.status().await.progress, Progress::Complete);
    drop(worker);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn cleanup_dispatch_lost_job_audit_after_claim_blocks_runtime_work() {
    let f = DispatchFixture::new(true).await;
    let id = f
        .submit("audit-after-claim")
        .await
        .unwrap()
        .receipt
        .unwrap()
        .id;
    let worker = f.worker("audit-loss");
    for _ in 0..2 {
        f.due(&id).await;
        worker.run_one(&id).await.unwrap();
    }
    assert_eq!(f.status().await.progress, Progress::RemovalStarted);
    let before = f.f.f.f.original.backend.requests().await;
    let audit =
        f.f.row("channel_child_cleanup_job_admissions_v1", &id)
            .await;
    f.due(&id).await;
    let (reached, resume) = f.authority.pause_once();
    let (result, ()) = tokio::join!(worker.run_one(&id), async {
        reached.notified().await;
        f.f.db()
            .collection::<Document>("channel_child_cleanup_job_admissions_v1")
            .delete_one(doc! {"_id":&id})
            .await
            .unwrap();
        resume.notify_one();
    });
    assert!(result.is_err());
    assert_eq!(f.f.f.f.original.backend.requests().await, before);
    assert!(f
        .f
        .row(
            "sandboxes",
            f.f.f.record.placement.target().sandbox_id.as_str()
        )
        .await
        .contains_key(CLEANUP_EXECUTION_FIELD));
    f.f.db()
        .collection::<Document>("channel_child_cleanup_job_admissions_v1")
        .insert_one(audit)
        .await
        .unwrap();
    assert_eq!(f.status().await.progress, Progress::RemovalStarted);
    drop(worker);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn cleanup_dispatch_corrupt_early_identities_do_not_starve_valid_jobs() {
    let f = DispatchFixture::new(true).await;
    let id = f
        .submit("discover-after-corruption")
        .await
        .unwrap()
        .receipt
        .unwrap()
        .id;
    let rows =
        f.f.db()
            .collection::<Document>("channel_child_cleanup_jobs_v1");
    for (bad_id, allocation) in [
        (mongodb::bson::Bson::Int32(0), "invalid-integer"),
        (mongodb::bson::Bson::String(String::new()), "invalid-empty"),
        (
            mongodb::bson::Bson::String("!invalid".into()),
            "invalid-string",
        ),
    ] {
        rows.insert_one(
            doc! {"_id":bad_id,"allocation_id":allocation,"complete":false,
            "next_attempt_at_ms":0_i64,"lease_expires_at_ms":null},
        )
        .await
        .unwrap();
    }
    let worker = f.worker("discovery-corruption");
    assert_eq!(
        worker.discover(None).await.unwrap(),
        ["!invalid".to_owned(), id.clone()]
    );
    assert!(worker.run_one("!invalid").await.is_err());
    assert_eq!(
        worker.discover(Some("!invalid")).await.unwrap(),
        std::slice::from_ref(&id)
    );
    worker.run_one(&id).await.unwrap();
    assert_eq!(f.status().await.progress, Progress::AdmissionClosed);
    drop(worker);
    f.close().await;
}
