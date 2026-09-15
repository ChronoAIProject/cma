//! Job acceptance rollback and causal disagreement with physical stage proof.
use super::dispatch_fixture::DispatchFixture;
use super::*;
use crate::bridge::channel_scheduler::ChannelWorkPort;
use layer2_core::bridge::cleanup::{ChildCleanupProgress, CleanupError, CleanupJob};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn cleanup_job_audit_failure_rolls_back_new_job_and_stage_a_then_same_key_recovers() {
    let f = DispatchFixture::new(true).await;
    f.f.db().run_command(doc! {"collMod":"channel_child_cleanup_job_admissions_v1", "validator":{"_id":{"$exists":false}},"validationLevel":"strict","validationAction":"error"}).await.unwrap();
    assert!(f.submit("atomic-audit").await.is_err());
    for name in [
        "channel_child_cleanup_jobs_v1",
        "channel_child_cleanup_job_admissions_v1",
        "channel_reuse_cleanup_requests",
        "channel_reuse_cleanup_audits",
    ] {
        assert_eq!(
            f.f.db()
                .collection::<Document>(name)
                .count_documents(doc! {})
                .await
                .unwrap(),
            0,
            "partial job/admission remained in {name}"
        );
    }
    let row = f.f.row("bridge_allocations", &f.f.f.record.id).await;
    assert!(
        !row.contains_key("channel_child_cleanup_job_v1")
            && !row.contains_key("channel_reuse_cleanup_request")
    );
    f.f.db()
        .run_command(doc! {"collMod":"channel_child_cleanup_job_admissions_v1", "validator":{}})
        .await
        .unwrap();
    let accepted = f.submit("atomic-audit").await.unwrap();
    let replay = f.submit("atomic-audit").await.unwrap();
    assert_eq!(accepted.receipt, replay.receipt);
    assert_eq!(accepted.progress, ChildCleanupProgress::Requested);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn cleanup_job_cannot_report_nonterminal_progress_ahead_of_actual_removal() {
    let f = DispatchFixture::new(true).await;
    let accepted = f.submit("progress-proof").await.unwrap();
    let id = accepted.receipt.as_ref().unwrap().id.clone();
    let worker = f.worker("proof-worker");
    worker.run_one(&id).await.unwrap();
    f.due(&id).await;
    worker.run_one(&id).await.unwrap();
    assert_eq!(
        f.status().await.progress,
        ChildCleanupProgress::RemovalStarted
    );
    let rows =
        f.f.db()
            .collection::<Document>("channel_child_cleanup_jobs_v1");
    let original = f.f.row("channel_child_cleanup_jobs_v1", &id).await;
    let mut altered = original.clone();
    let mut job: CleanupJob = crate::store::sandbox_mongo::read_record(&altered).unwrap();
    job.progress = ChildCleanupProgress::HomeRemoved;
    job.validate().unwrap();
    altered.insert(
        "record",
        crate::store::sandbox_mongo::record_bson(&job).unwrap(),
    );
    rows.replace_one(doc! {"_id":&id}, altered).await.unwrap();
    let source = &f.f.f.f.original;
    let result = f
        .service
        .status(
            source.lease.actor_context(),
            Some(source.lease.credential()),
            &f.f.f.record.identity.bridge_id,
            &f.f.f.record.id,
        )
        .await;
    assert!(matches!(result, Err(CleanupError::Corrupt)));
    rows.replace_one(doc! {"_id":&id}, original).await.unwrap();
    assert_eq!(
        f.status().await.progress,
        ChildCleanupProgress::RemovalStarted
    );
    drop(worker);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn cleanup_missing_job_or_marker_cannot_reconstruct_an_accepted_intent() {
    let f = DispatchFixture::new(true).await;
    let accepted = f.submit("retained-intent").await.unwrap();
    let id = accepted.receipt.as_ref().unwrap().id.clone();
    let jobs =
        f.f.db()
            .collection::<Document>("channel_child_cleanup_jobs_v1");
    let saved_job = f.f.row("channel_child_cleanup_jobs_v1", &id).await;
    jobs.delete_one(doc! {"_id":&id}).await.unwrap();
    assert!(matches!(
        f.submit("replacement-intent").await,
        Err(CleanupError::Corrupt)
    ));
    assert_eq!(jobs.count_documents(doc! {}).await.unwrap(), 0);
    jobs.insert_one(saved_job).await.unwrap();
    let allocations = f.f.db().collection::<Document>("bridge_allocations");
    let saved_allocation = f.f.row("bridge_allocations", &f.f.f.record.id).await;
    allocations
        .update_one(
            doc! {"_id":&f.f.f.record.id},
            doc! {"$unset":{"channel_child_cleanup_job_v1":""}},
        )
        .await
        .unwrap();
    assert!(matches!(
        f.submit("replacement-intent").await,
        Err(CleanupError::Corrupt)
    ));
    allocations
        .replace_one(doc! {"_id":&f.f.f.record.id}, saved_allocation)
        .await
        .unwrap();
    assert_eq!(
        f.submit("retained-intent").await.unwrap().receipt,
        accepted.receipt
    );
    f.close().await;
}
