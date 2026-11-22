use super::*;
use layer2_core::sandbox::SandboxPolicy;

struct ProviderKey {
    id: String,
    expires_at: SystemTime,
}

impl ProviderKey {
    fn accept_without_result(request: Exchange, now: SystemTime) -> Self {
        assert_eq!(request.method, axum::http::Method::POST);
        assert_eq!(request.uri.path(), "/api/v1/api-keys");
        request.stall_body();
        Self {
            id: format!("provider-{}", uuid::Uuid::new_v4().simple()),
            expires_at: now + Duration::from_secs(30 * 24 * 60 * 60),
        }
    }

    fn assert_live(&self, peer: &Peer, now: SystemTime) {
        assert!(now < self.expires_at);
        assert_eq!(
            peer.count_route("DELETE", &format!("/api/v1/api-keys/{}", self.id)),
            0
        );
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn lost_issuance_reply_retains_discoverable_owner_beyond_retention_grace() {
    let mut f = Fixture::new("purge_owner_unknown").await;
    let started = SystemTime::now();
    let issuing = f.issue(1);
    let provider = ProviderKey::accept_without_result(f.prepare().await, started);
    issuing.abort();
    assert!(issuing.await.unwrap_err().is_cancelled());
    let intent = f.custody.issuance(&f.claim).await.unwrap().unwrap();
    assert_eq!(intent.state, crate::sandbox::IssuanceState::Unknown);
    let policy = SandboxPolicy::default();
    assert_eq!(
        policy.retention_grace,
        Duration::from_secs(7 * 24 * 60 * 60)
    );
    let later = started + policy.retention_grace + Duration::from_secs(60);
    let (worker, _) =
        retirement_fixture::begin_with_policy(&f, Arc::new(peer(&f.name).await), policy).await;
    for _ in 0..2 {
        assert!(f
            .store
            .due_sandbox_work(&at(0), 10)
            .await
            .unwrap()
            .contains(&f.claim.id));
        assert!(worker
            .run_one(&f.claim.id, SystemTime::now())
            .await
            .unwrap());
    }
    let mut blocked = f.store.get_sandbox(&f.claim.id).await.unwrap().unwrap();
    assert_eq!(blocked.status, SandboxStatus::Retiring);
    assert_eq!(
        blocked.work.step,
        SandboxWorkStep::RevokeRetirementCredentials
    );
    assert!(blocked.purge_after.is_none());
    let previous_fence = blocked.work.next_fence;
    let revision = blocked.revision;
    // Make the committed retry due on Mongo's clock. Lifecycle and issuance
    // remain untouched; the next worker uses a later application clock and a
    // newly acquired server-capped lease, never a lease carried across days.
    blocked.work.ready_at = at(-1);
    blocked.revision += 1;
    f.store.save_sandbox(revision, blocked).await.unwrap();
    assert!(worker.run_one(&f.claim.id, later).await.unwrap());
    let held = f.store.get_sandbox(&f.claim.id).await.unwrap().unwrap();
    assert_eq!(held.status, SandboxStatus::Retiring);
    assert_eq!(held.work.step, SandboxWorkStep::RevokeRetirementCredentials);
    assert!(held.purge_after.is_none());
    assert!(held.work.next_fence > previous_fence);
    assert!(held.work.lease.is_none());
    assert_eq!(f.custody.issuance(&held).await.unwrap(), Some(intent));
    assert!(f.custody.list(&held.id).await.unwrap().is_empty());
    assert!(f
        .store
        .due_sandbox_work(&held.work.ready_at, 10)
        .await
        .unwrap()
        .contains(&held.id));
    assert_eq!(f.peer.count_route("POST", "/api/v1/api-keys"), 1);
    assert_eq!(
        f.peer.count(),
        3,
        "cleanup cannot send provider effects for an unknown key"
    );
    provider.assert_live(&f.peer, later);
    f.cleanup().await;
}
