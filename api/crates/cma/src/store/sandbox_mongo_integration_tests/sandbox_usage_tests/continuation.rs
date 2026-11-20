use super::*;
use base64::{engine::general_purpose::STANDARD, Engine};
use layer2_core::sandbox_usage_cursor::{UsageCursorPolicy, UsageCursors};
use layer2_core::sandbox_usage_service::{SandboxUsageService, UsageReadError};

fn service(
    store: MongoSandboxStore,
    deployment: &str,
    policy: UsageCursorPolicy,
) -> Arc<SandboxUsageService> {
    let root = AuthEncryptionKey::from_base64(&STANDARD.encode([7; 32])).unwrap();
    let key = root.usage_cursor_key(deployment);
    SandboxUsageService::new(Arc::new(store), UsageCursors::new(&key, policy))
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn usage_signed_continuations_cross_replicas_preserve_raw_store_and_reject_scope_changes() {
    let (client, name, store, parent) = fixture("usage_continuation").await;
    for id in ["usage-z-b", "usage-z-c"] {
        let mut next = sandbox(id);
        next.name = id.into();
        store.create_sandbox(next.clone()).await.unwrap();
        store
            .append_sandbox_event(&next.id, NOW.into(), SandboxEventKind::Created)
            .await
            .unwrap();
    }
    store
        .checkpoint_usage_one(parent.id.as_str(), MINUTE)
        .await
        .unwrap();
    store
        .checkpoint_usage_one(parent.id.as_str(), TWO_MINUTES)
        .await
        .unwrap();
    let owner = Caller::new(
        parent.owner.clone(),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    let before = store.usage(&parent.id).await.unwrap().unwrap();
    for raw in ["invalid", "zzz"] {
        assert!(store
            .usage_page(
                Some(&owner.subject),
                &PageRequest::new(1, Some(raw.into())).unwrap()
            )
            .await
            .is_ok());
    }
    let first = service(store, "deployment-a", UsageCursorPolicy::Strict);
    let replica = service(
        MongoSandboxStore::open(&client, &name).await.unwrap(),
        "deployment-a",
        UsageCursorPolicy::ReaderOnly,
    );
    let other_deployment = service(
        MongoSandboxStore::open(&client, &name).await.unwrap(),
        "deployment-b",
        UsageCursorPolicy::Strict,
    );
    let page = first
        .list(&owner, false, None, &PageRequest::new(1, None).unwrap())
        .await
        .unwrap();
    assert_eq!(page.items[0].sandbox_id, parent.id);
    let token = page.next.unwrap();
    assert!(token.starts_with("u1."));
    let request = PageRequest::new(1, Some(token)).unwrap();
    let page = replica.list(&owner, false, None, &request).await.unwrap();
    assert_eq!(page.items[0].sandbox_id.as_str(), "usage-z-b");
    let next = page.next.unwrap();
    assert!(next.starts_with("u1."));
    let last = first
        .list(
            &owner,
            false,
            None,
            &PageRequest::new(200, Some(next)).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(last.items.len(), 1);
    assert_eq!(last.items[0].sandbox_id.as_str(), "usage-z-c");
    assert!(last.next.is_none());
    assert!(matches!(
        other_deployment.list(&owner, false, None, &request).await,
        Err(UsageReadError::InvalidCursor)
    ));
    let other = Caller::new(SubjectId("other".into()), owner.authorization.clone());
    assert!(matches!(
        first.list(&other, false, None, &request).await,
        Err(UsageReadError::InvalidCursor)
    ));
    for raw in ["invalid", "zzz"] {
        assert!(matches!(
            first
                .list(
                    &owner,
                    false,
                    None,
                    &PageRequest::new(1, Some(raw.into())).unwrap()
                )
                .await,
            Err(UsageReadError::InvalidCursor)
        ));
    }
    let first_events = first
        .events(
            &owner,
            &parent.id,
            None,
            &PageRequest::new(1, None).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(first_events.items[0].revision, 1);
    let next_events = replica
        .events(
            &owner,
            &parent.id,
            None,
            &PageRequest::new(200, first_events.next).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        next_events
            .items
            .iter()
            .map(|e| e.revision)
            .collect::<Vec<_>>(),
        [2, 3]
    );
    assert!(next_events.next.is_none());
    assert_eq!(replica.read(&owner, &parent.id).await.unwrap(), before);
    client.database(&name).drop().await.unwrap();
}
