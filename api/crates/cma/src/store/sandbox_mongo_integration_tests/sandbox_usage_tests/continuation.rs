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
    let page = page.page;
    assert_eq!(page.items[0].sandbox_id, parent.id);
    let token = page.next.unwrap();
    assert!(token.starts_with("u1."));
    let request = PageRequest::new(1, Some(token)).unwrap();
    let page = replica
        .list(&owner, false, None, &request)
        .await
        .unwrap()
        .page;
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
    let last = last.page;
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

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn usage_snapshot_continuations_pin_rows_and_events_and_refuse_erased_evidence() {
    let (client, name, store, parent) = fixture("usage_snapshot").await;
    let add = |id: &'static str| {
        let store = store.clone();
        async move {
            let mut row = sandbox(id);
            row.name = id.into();
            store.create_sandbox(row.clone()).await.unwrap();
            store
                .append_sandbox_event(&row.id, NOW.into(), SandboxEventKind::Created)
                .await
                .unwrap();
        }
    };
    add("usage-z-b").await;
    add("usage-z-c").await;
    store
        .checkpoint_usage_one(parent.id.as_str(), MINUTE)
        .await
        .unwrap();
    let actor = Caller::new(
        parent.owner.clone(),
        Authorization::new([Capability::UseOwnSandboxes, Capability::ReadAllSandboxes]),
    );
    let issuer = SandboxUsageService::new(
        Arc::new(store.clone()),
        UsageCursors::new(&[9; 32], UsageCursorPolicy::Strict).with_snapshot_issuance(true),
    );
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let other_client = Client::with_uri_str(&uri).await.unwrap();
    let replica = SandboxUsageService::new(
        Arc::new(MongoSandboxStore::open(&other_client, &name).await.unwrap()),
        UsageCursors::new(&[9; 32], UsageCursorPolicy::ReaderOnly),
    );
    let start = PageRequest::new(1, None).unwrap();
    let first = issuer.list(&actor, false, None, &start).await.unwrap();
    assert_eq!(first.page.items[0].sandbox_id, parent.id);
    assert!(first.page.next.as_deref().unwrap().starts_with("u2."));
    let request = PageRequest::new(1, first.page.next).unwrap();
    let first_admin = issuer.list(&actor, true, None, &start).await.unwrap();
    let admin_request = PageRequest::new(1, first_admin.page.next).unwrap();
    let events = issuer
        .events(&actor, &parent.id, None, &start)
        .await
        .unwrap();
    assert_eq!(events.items[0].revision, 1);
    let event_request = PageRequest::new(200, events.next).unwrap();

    add("usage-z-a").await;
    store
        .sandbox_usage
        .update_one(
            doc! {"_id":"usage-z-b"},
            doc! {"$set":{"record.name":"updated"}},
        )
        .await
        .unwrap();
    store
        .checkpoint_usage_one(parent.id.as_str(), TWO_MINUTES)
        .await
        .unwrap();

    let second = replica.list(&actor, false, None, &request).await.unwrap();
    assert_eq!(second.as_of, first.as_of);
    assert_eq!(second.page.items[0].sandbox_id.as_str(), "usage-z-b");
    assert_eq!(second.page.items[0].name, "usage-z-b");
    let third = issuer
        .list(
            &actor,
            false,
            None,
            &PageRequest::new(1, second.page.next).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(third.as_of, first.as_of);
    assert_eq!(third.page.items[0].sandbox_id.as_str(), "usage-z-c");
    assert!(third.page.next.is_none());
    let events = replica
        .events(&actor, &parent.id, None, &event_request)
        .await
        .unwrap();
    assert_eq!(
        events
            .items
            .iter()
            .map(|event| event.revision)
            .collect::<Vec<_>>(),
        [2]
    );
    assert!(events.next.is_none());

    let fresh = issuer
        .list(&actor, false, None, &PageRequest::new(200, None).unwrap())
        .await
        .unwrap();
    assert_eq!(fresh.page.items[1].sandbox_id.as_str(), "usage-z-a");
    assert_eq!(fresh.page.items[2].name, "updated");

    // The lookahead influences the next token and carries the same live check.
    store
        .sandbox_usage
        .delete_one(doc! {"_id":"usage-z-c"})
        .await
        .unwrap();
    for (admin, request) in [(false, &request), (true, &admin_request)] {
        assert!(matches!(
            replica.list(&actor, admin, None, request).await,
            Err(UsageReadError::InvalidCursor)
        ));
    }
    store
        .sandbox_usage_events
        .delete_one(doc! {"sandbox_id":parent.id.as_str(), "revision":2_i64})
        .await
        .unwrap();
    assert!(matches!(
        replica
            .events(&actor, &parent.id, None, &event_request)
            .await,
        Err(UsageReadError::InvalidCursor)
    ));
    store
        .sandbox_usage
        .delete_one(doc! {"_id":parent.id.as_str()})
        .await
        .unwrap();
    assert!(matches!(
        replica
            .events(&actor, &parent.id, None, &event_request)
            .await,
        Err(UsageReadError::NotFound)
    ));
    client.database(&name).drop().await.unwrap();
}
