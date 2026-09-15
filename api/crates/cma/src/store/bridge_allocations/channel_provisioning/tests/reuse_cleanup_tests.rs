use super::{reuse_command_fixture::ReuseCommandFixture, *};
use crate::sandbox::channel_reuse_cleanup::*;
use layer2_core::sandbox::{agent_io::*, store::*, *};
use mongodb::bson::Document;
use std::time::SystemTime;

#[path = "reuse_cleanup_drain_tests.rs"]
mod drain;
#[path = "reuse_cleanup_effects.rs"]
mod effects;
#[path = "reuse_cleanup_fixture.rs"]
mod fixture;
#[path = "reuse_cleanup_refusals.rs"]
mod refusals;
use fixture::CleanupFixture;

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn shared_child_cleanup_request_is_explicit_atomic_and_exact_after_reopen() {
    for shared_workspace in [false, true] {
        let mut f = CleanupFixture::new(shared_workspace).await;
        assert!(
            f.request_cleanup().await.is_err(),
            "live allocation cannot request cleanup"
        );
        f.assert_no_request().await;
        let next = f.reset().await;
        let parent = f.f.sandbox().await;
        let sibling = f.row("agents", f.f.f.sibling.id.as_str()).await;
        let backend_before = f.f.f.original.backend.requests().await;
        let permit = f.permit().await;
        let request = f.request();
        let (first, replay) = tokio::join!(
            f.f.f
                .original
                .allocations
                .request_channel_reuse_cleanup(&permit, &request),
            f.f.f
                .original
                .allocations
                .request_channel_reuse_cleanup(&permit, &request),
        );
        let first = first.unwrap();
        assert!(first == replay.unwrap());
        assert!(first.holds.is_empty());
        assert_eq!(first.seal.workspace_owned, !shared_workspace);
        assert_eq!(first.seal.target, *f.original.placement.target());
        assert_eq!(
            first.requested_by,
            f.f.f.original.lease.actor_context().actor
        );
        let invokes = Arc::new(
            crate::store::MongoBridgeInvokeStore::open(
                f.f.f.original.bridges.clone(),
                f.f.f.original.resources.clone(),
                &f.f.f.original.database,
            )
            .await
            .unwrap(),
        );
        let reopened = crate::store::MongoBridgeAllocationStore::open(
            f.f.f.original.bridges.clone(),
            Some(invokes),
            &f.f.f.original.database,
        )
        .await
        .unwrap();
        let recovered = reopened
            .request_channel_reuse_cleanup(&permit, &request)
            .await
            .unwrap();
        assert!(first == recovered);
        for name in [
            "channel_reuse_cleanup_requests",
            "channel_reuse_cleanup_audits",
        ] {
            assert_eq!(
                f.db()
                    .collection::<Document>(name)
                    .count_documents(doc! {})
                    .await
                    .unwrap(),
                1
            );
        }
        assert_eq!(f.f.sandbox().await, parent);
        assert_eq!(f.row("agents", f.f.f.sibling.id.as_str()).await, sibling);
        assert_eq!(f.f.f.original.backend.requests().await, backend_before);
        assert!(f.f.f.current(&next).await == next);
        let mut changed = request.clone();
        changed.request_id = "changed-cleanup-request".into();
        assert_eq!(
            reopened
                .request_channel_reuse_cleanup(&permit, &changed)
                .await
                .err(),
            Some(AllocationError::Conflict)
        );
        changed = request;
        changed.expected_allocation_revision += 1;
        assert_eq!(
            reopened
                .request_channel_reuse_cleanup(&permit, &changed)
                .await
                .err(),
            Some(AllocationError::IdentityMismatch)
        );
        f.close().await;
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn shared_child_cleanup_audit_failure_rolls_back_every_request_and_fence() {
    let mut f = CleanupFixture::new(true).await;
    let next = f.reset().await;
    let permit = f.permit().await;
    let target = f.original.placement.target();
    let identities = [
        ("bridge_allocations", f.f.record.id.clone()),
        ("bridge_allocations", next.id.clone()),
        ("bridge_allocation_heads", next.identity.head_id().unwrap()),
        (
            "bridge_definitions",
            f.f.record.identity.bridge_id.as_str().to_owned(),
        ),
        (
            "bridge_credentials",
            f.f.record.config.connector.custody.as_str().to_owned(),
        ),
        ("sandboxes", target.sandbox_id.to_string()),
        ("workspaces", target.workspace_id.to_string()),
        ("worktrees", target.worktree_id.to_string()),
        ("agents", target.agent_id.to_string()),
        ("agent_creation_provenance", target.agent_id.to_string()),
    ];
    let mut before = Vec::new();
    for (collection, id) in &identities {
        before.push(f.row(collection, id).await);
    }
    f.db().run_command(doc! {"collMod":"channel_reuse_cleanup_audits","validator":{"forced_failure":true},"validationLevel":"strict","validationAction":"error"}).await.unwrap();
    assert_eq!(
        f.f.f
            .original
            .allocations
            .request_channel_reuse_cleanup(&permit, &f.request())
            .await
            .err(),
        Some(AllocationError::Unavailable)
    );
    f.assert_no_request().await;
    for ((collection, id), expected) in identities.iter().zip(before) {
        assert_eq!(
            f.row(collection, id).await,
            expected,
            "changed {collection}"
        );
    }
    f.db()
        .run_command(doc! {"collMod":"channel_reuse_cleanup_audits","validator":{}})
        .await
        .unwrap();
    f.request_cleanup().await.unwrap();
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn shared_child_cleanup_replay_refuses_missing_or_corrupted_permanent_audit() {
    let mut f = CleanupFixture::new(true).await;
    f.reset().await;
    let state = f.request_cleanup().await.unwrap();
    let request = f.row("channel_reuse_cleanup_requests", &state.id).await;
    let audit = f.row("channel_reuse_cleanup_audits", &state.id).await;
    let audits = f
        .db()
        .collection::<Document>("channel_reuse_cleanup_audits");
    audits.delete_one(doc! {"_id":&state.id}).await.unwrap();
    assert_eq!(
        f.request_cleanup().await.err(),
        Some(AllocationError::Corrupt)
    );
    audits.insert_one(audit.clone()).await.unwrap();
    audits
        .update_one(
            doc! {"_id":&state.id},
            doc! {"$set":{"record.digest":"0".repeat(64)}},
        )
        .await
        .unwrap();
    assert_eq!(
        f.request_cleanup().await.err(),
        Some(AllocationError::Corrupt)
    );
    assert_eq!(
        f.row("channel_reuse_cleanup_requests", &state.id).await,
        request
    );
    audits
        .replace_one(doc! {"_id":&state.id}, audit)
        .await
        .unwrap();
    assert!(f.request_cleanup().await.unwrap() == state);
    f.close().await;
}
