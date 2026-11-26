use super::*;

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set with test failpoints"]
async fn document_deletion_retains_ordered_revision_and_mutation_audit() {
    let database = database().await;
    let admin = database.client().database("admin");
    let store = MongoDocumentStore::new(database.clone()).with_source_commits(true);
    store.ensure_indexes().await.unwrap();
    let quota = store.quota(true);
    activate(&quota, &database, &[]).await;
    let service = service(&store, Arc::new(Blobs::default()), true)
        .with_directory(memory::recipient_directory(&["guest"]))
        .with_deletion_enabled(true);
    let owner = owner();
    let guest = Caller::new(
        SubjectId::new("guest"),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    let created = service.create(&owner, "create", create()).await.unwrap();
    let id = &created.id;
    let initial = store.get(id).await.unwrap().unwrap();
    service
        .update(&owner, id, "second", edit(1, "Second", Some("replacement")))
        .await
        .unwrap();
    let second = store.get(id).await.unwrap().unwrap();
    let second_key = second.receipts.keys().next().unwrap().clone();
    service
        .update(&owner, id, "third", edit(2, "Third", Some("original")))
        .await
        .unwrap();
    let third = store.get(id).await.unwrap().unwrap();
    let third_key = third
        .receipts
        .keys()
        .find(|key| key.as_str() != second_key.as_str())
        .unwrap()
        .clone();
    let shared = service
        .grant(
            &owner,
            id,
            "share",
            "guest",
            GrantDocument {
                expected_revision: 3,
                permission: DocumentPermission::Read,
            },
        )
        .await
        .unwrap();
    assert!(service.read(&guest, id, None).await.is_ok());
    let live = store.get(id).await.unwrap().unwrap();
    assert_eq!(
        live.revisions
            .iter()
            .map(|r| r.metadata.revision)
            .collect::<Vec<_>>(),
        [1, 2, 3]
    );
    assert_eq!(
        live.revisions[0].metadata.sha256,
        live.revisions[2].metadata.sha256
    );
    assert_ne!(
        live.revisions[0].metadata.sha256,
        live.revisions[1].metadata.sha256
    );
    let charged = usage(&quota, "owner").await;
    admin.run_command(doc! {"configureFailPoint": "failCommand", "mode": "alwaysOn", "data": {"failCommands": ["commitTransaction"], "writeConcernError": {"code": 64, "errmsg": "fixture lost acknowledgement"}}}).await.unwrap();
    let uncertain = service.delete(&owner, id, "delete", shared.revision).await;
    settle_injected_commit(&admin, &database).await;
    let row = store
        .rows
        .find_one(doc! {"_id": id})
        .await
        .unwrap()
        .unwrap();
    let (tombstone, audit) = crate::document_artifacts::deletion::decode_audit(&row)
        .unwrap()
        .unwrap();
    let receipt = service
        .delete(&owner, id, "delete", shared.revision)
        .await
        .unwrap();
    match uncertain {
        Ok(first) => assert_eq!(first, receipt),
        Err(error) => assert_eq!(error, DocumentError::Unavailable),
    }
    assert_eq!(tombstone.receipt, receipt);
    assert_eq!(
        mongodb::bson::serialize_to_bson(&audit).unwrap(),
        mongodb::bson::serialize_to_bson(&live).unwrap()
    );
    for (record, body, edit) in [
        (&initial, "original", None),
        (&second, "replacement", Some((1, second_key.as_str()))),
        (&third, "original", Some((2, third_key.as_str()))),
    ] {
        assert!(
            !reconcile_after_majority_read(&store, record, &reservation(id, body), edit)
                .await
                .unwrap()
        );
    }
    for caller in [&owner, &guest] {
        assert_eq!(
            service.read(caller, id, None).await,
            Err(DocumentError::NotFound)
        );
        assert_eq!(
            service.source(caller, id, None, Some(1)).await,
            Err(DocumentError::NotFound)
        );
        assert!(matches!(
            service.revisions(caller, id, None, 0, 25).await,
            Err(DocumentError::NotFound)
        ));
        assert!(service
            .list(caller, &DocumentQuery::default())
            .await
            .unwrap()
            .items
            .is_empty());
    }
    assert_eq!(usage(&quota, "owner").await, charged);
    database.drop().await.unwrap();
}
