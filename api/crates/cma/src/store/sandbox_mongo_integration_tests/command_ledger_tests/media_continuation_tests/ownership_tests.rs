use super::*;
use layer2_core::agent_ownership::{
    test_support::MemoryOwnershipStore, Authority, Member, OwnershipCoordinator, PROTOCOL_VERSION,
};

async fn owned(f: &Fixture) -> Arc<Authority> {
    let owners = MemoryOwnershipStore::default();
    let coordinator = OwnershipCoordinator::register(
        owners.client(),
        Member {
            instance_id: "media-owner".into(),
            protocol: PROTOCOL_VERSION,
            relay_address: "http://media-owner:8080".into(),
            expires_at_ms: 0,
            draining: false,
            owned_agents: 0,
            max_owned_agents: 16,
        },
    )
    .await
    .unwrap();
    let authority = coordinator.acquire(&f.agent.id).await.unwrap().unwrap();
    f.service.install_ownership(coordinator).unwrap();
    authority
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn coordinated_media_refuses_unscoped_upload_before_receipt_or_native_write() {
    let f = Fixture::new("media_owner_unscoped", false).await;
    let _authority = owned(&f).await;
    let result = f.upload().await;
    let writes = f.files.writes.load(Ordering::SeqCst);
    let record = f
        .store
        .get_media_upload(&MediaUploadRecord::id(
            &f.parent.receipt.envelope.command_id,
            0,
        ))
        .await
        .unwrap();
    f.close().await;
    assert!(
        matches!(result, Err(BindingMediaError::Unavailable)),
        "unscoped media must refuse before upload admission"
    );
    assert_eq!(writes, 0);
    assert!(record.is_none());
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn prepared_media_upload_cannot_borrow_successor_authority_after_revocation() {
    let f = Fixture::new("media_owner_revoked", false).await;
    let authority = owned(&f).await;
    let upload = authority
        .run(f.service.clone().prepare_upload(
            f.caller.clone(),
            f.path.clone(),
            f.params(),
            f.artifact.media_type.clone(),
            f.bytes.len() as u64,
        ))
        .await
        .unwrap()
        .unwrap();
    authority.revoke();
    let mut successor_token = authority.token.clone();
    successor_token.epoch += 1;
    let successor = Authority::new(
        successor_token,
        tokio::time::Instant::now() + std::time::Duration::from_secs(30),
    );
    let result = successor.run(upload.upload(f.bytes.clone())).await.unwrap();
    let writes = f.files.writes.load(Ordering::SeqCst);
    f.close().await;
    assert!(
        matches!(result, Err(BindingMediaError::Unavailable)),
        "prepared upload must retain its original owner scope"
    );
    assert_eq!(writes, 0);
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn media_supplement_requires_owner_before_continuation_reservation() {
    let f = Fixture::new("media_owner_supplement", false).await;
    let authority = owned(&f).await;
    let receipt = authority.run(f.upload()).await.unwrap().unwrap();
    let body = f.supplement(&receipt, "owner scoped transcript");
    let before = f
        .store
        .agent_commands
        .count_documents(doc! {})
        .await
        .unwrap();
    let result = f
        .service
        .supplement(f.caller.clone(), f.path.clone(), body)
        .await;
    let after = f
        .store
        .agent_commands
        .count_documents(doc! {})
        .await
        .unwrap();
    f.close().await;
    assert!(
        matches!(result, Err(BindingMediaError::Unavailable)),
        "unscoped supplement must refuse before reservation"
    );
    assert_eq!(after, before);
}
