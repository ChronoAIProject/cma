use super::*;

const OLD_BYTES: usize = 12 * 1024 * 1024;

// Valid historical receipts reach the two independent old boundaries without
// thousands of network round trips. All writes/reads use the production codec.
fn saturate(record: &mut StoredDocument, bytes: bool) {
    let count = MAX_RECEIPTS - usize::from(bytes);
    let receipt = record.receipts.values().next().unwrap().clone();
    for n in record.receipts.len()..count {
        record.receipts.insert(format!("{n:064x}"), receipt.clone());
    }
    if bytes {
        for receipt in record.receipts.values_mut() {
            receipt.outcome.comment = Some(DocumentComment {
                id: format!("dcm_{}", "a".repeat(32)),
                author_subject: "historical-reader".into(),
                body: "\n".repeat(8191) + "x",
                document_revision: 1,
                created_at: record.metadata.created_at.clone(),
            });
        }
        let mut excess = serde_json::to_vec(record).unwrap().len() - OLD_BYTES;
        for receipt in record.receipts.values_mut() {
            let body = &mut receipt.outcome.comment.as_mut().unwrap().body;
            let remove = (excess / 2).min(body.len() - 1);
            body.drain(..remove);
            excess -= remove * 2;
        }
        if excess != 0 {
            let body = &mut record
                .receipts
                .values_mut()
                .find(|r| r.outcome.comment.as_ref().unwrap().body.len() > 1)
                .unwrap()
                .outcome
                .comment
                .as_mut()
                .unwrap()
                .body;
            body.replace_range(..1, "x");
        }
        assert_eq!(serde_json::to_vec(record).unwrap().len(), OLD_BYTES);
    }
    assert_eq!(record.receipts.len(), count);
    record.validate().unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable MongoDB replica set"]
async fn expanded_writes_reopen_through_flag_off_rollback_reader() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").expect("disposable replica set required");
    let client = Client::with_uri_str(uri).await.unwrap();
    let database = client.database(&format!(
        "cma_documents_rollout_{}",
        uuid::Uuid::new_v4().simple()
    ));
    let store = Arc::new(MongoDocumentStore::new(database.clone()));
    store.ensure_indexes().await.unwrap();
    let blobs = Arc::new(memory::MemoryDocumentBlobs::default());
    let writer = DocumentService::with_revocation_reserve(store.clone(), blobs.clone(), true);
    let owner = caller("owner");
    let guest = caller("guest");
    let remaining = caller("remaining");
    let linked = caller("linked-reader");
    let token = "ab".repeat(32);
    for bytes in [false, true] {
        for revoke_link in [false, true] {
            let key = format!("create-{bytes}-{revoke_link}");
            let document = writer
                .create(
                    &owner,
                    &key,
                    CreateDocument {
                        title: "Rollout fixture".into(),
                        markdown: "Private source".into(),
                        origin_agent_id: None,
                    },
                )
                .await
                .unwrap();
            for (subject, revision) in [("guest", 1), ("remaining", 2)] {
                writer
                    .grant(
                        &owner,
                        &document.id,
                        subject,
                        subject,
                        GrantDocument {
                            expected_revision: revision,
                            permission: DocumentPermission::Read,
                        },
                    )
                    .await
                    .unwrap();
            }
            let link = writer
                .create_link(
                    &owner,
                    &document.id,
                    "link",
                    CreateDocumentLink {
                        expected_revision: 3,
                        permission: DocumentPermission::Read,
                        token: token.clone(),
                    },
                )
                .await
                .unwrap()
                .link
                .unwrap();
            let mut record = store.get(&document.id).await.unwrap().unwrap();
            let previous = record.metadata.revision;
            record.metadata.revision += 1;
            saturate(&mut record, bytes);
            assert!(store
                .compare_exchange(previous, &record, None)
                .await
                .unwrap());
            let revision = record.metadata.revision;

            let baseline = DocumentService::new(store.clone(), blobs.clone());
            let denied = if revoke_link {
                baseline
                    .revoke_link(&owner, &document.id, "revoke", &link.id, revision)
                    .await
            } else {
                baseline
                    .revoke(&owner, &document.id, "revoke", "guest", revision)
                    .await
            };
            assert_eq!(denied, Err(DocumentError::Capacity));
            assert_eq!(
                store
                    .get(&document.id)
                    .await
                    .unwrap()
                    .unwrap()
                    .metadata
                    .revision,
                revision
            );
            let result = if revoke_link {
                writer
                    .revoke_link(&owner, &document.id, "revoke", &link.id, revision)
                    .await
            } else {
                writer
                    .revoke(&owner, &document.id, "revoke", "guest", revision)
                    .await
            }
            .unwrap();
            let expanded = store.get(&document.id).await.unwrap().unwrap();
            if bytes {
                assert!(serde_json::to_vec(&expanded).unwrap().len() > OLD_BYTES);
                assert_eq!(expanded.receipts.len(), MAX_RECEIPTS);
            } else {
                assert_eq!(expanded.receipts.len(), MAX_RECEIPTS + 1);
            }

            // This default-off image is the designated one-image rollback floor.
            // Reopening the real store exercises BSON decode and validation;
            // memory-only fixture reads would not prove reader compatibility.
            let reopened = Arc::new(MongoDocumentStore::new(database.clone()));
            let rollback = DocumentService::new(reopened.clone(), blobs.clone());
            for allowed in [&owner, &remaining] {
                assert_eq!(
                    rollback
                        .source(allowed, &document.id, None, None)
                        .await
                        .unwrap()
                        .markdown,
                    "Private source"
                );
            }
            let retry = if revoke_link {
                assert_eq!(
                    rollback
                        .source(&linked, &document.id, Some(&token), None)
                        .await,
                    Err(DocumentError::NotFound)
                );
                assert!(rollback
                    .source(&guest, &document.id, None, None)
                    .await
                    .is_ok());
                rollback
                    .revoke_link(&owner, &document.id, "revoke", &link.id, revision)
                    .await
            } else {
                assert_eq!(
                    rollback.source(&guest, &document.id, None, None).await,
                    Err(DocumentError::NotFound)
                );
                assert!(rollback
                    .source(&linked, &document.id, Some(&token), None)
                    .await
                    .is_ok());
                rollback
                    .revoke(&owner, &document.id, "revoke", "guest", revision)
                    .await
            };
            assert_eq!(retry, Ok(result));
            // Replaying the original grant/link receipt cannot restore access.
            if revoke_link {
                rollback
                    .create_link(
                        &owner,
                        &document.id,
                        "link",
                        CreateDocumentLink {
                            expected_revision: 3,
                            permission: DocumentPermission::Read,
                            token: token.clone(),
                        },
                    )
                    .await
                    .unwrap();
                assert_eq!(
                    rollback
                        .source(&linked, &document.id, Some(&token), None)
                        .await,
                    Err(DocumentError::NotFound)
                );
            } else {
                rollback
                    .grant(
                        &owner,
                        &document.id,
                        "guest",
                        "guest",
                        GrantDocument {
                            expected_revision: 1,
                            permission: DocumentPermission::Read,
                        },
                    )
                    .await
                    .unwrap();
                assert_eq!(
                    rollback.source(&guest, &document.id, None, None).await,
                    Err(DocumentError::NotFound)
                );
            }
            assert_eq!(
                rollback
                    .revoke(
                        &remaining,
                        &document.id,
                        "foreign",
                        "remaining",
                        revision + 1
                    )
                    .await,
                Err(DocumentError::NotFound)
            );
            assert_eq!(
                rollback
                    .revoke(&owner, &document.id, "new", "remaining", revision + 1)
                    .await,
                Err(DocumentError::Capacity)
            );
            let after = reopened.get(&document.id).await.unwrap().unwrap();
            assert_eq!(
                serde_json::to_vec(&after).unwrap(),
                serde_json::to_vec(&expanded).unwrap()
            );
        }
    }
    database.drop().await.unwrap();
}
