use super::*;

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_quota_reclamation_binding_refuses_an_already_running_foreign_writer() {
    let database = database().await;
    let a = MongoDocumentStore::new(database.clone())
        .with_source_commits(true)
        .with_storage_namespace(Some(namespace()));
    let b = MongoDocumentStore::new(database.clone())
        .with_storage_namespace(Some(digest(b"foreign-bucket")));
    let original = a.quota(true);
    let foreign = b.quota(true);
    let baseline = activate(&original, &database, &[]).await;
    let first = source("owner", 1, b"first");
    assert_eq!(foreign.reserve_source(&first).await.unwrap(), None);
    original
        .bind_storage(&baseline.activation, &baseline.digest, &namespace())
        .await
        .unwrap();
    // The first binding ended generation-less grants even if the matching
    // AM keeps its reclamation flag off for longer than the historical grace.
    let now = DateTime::now();
    original.rows.update_one(doc! {"_id":STATE}, doc! {"$set":{"storage_namespace_bound_at":DateTime::from_millis(now.timestamp_millis()-7_200_000)}}).await.unwrap();
    let authored = Arc::new(memory::MemoryDocuments::default());
    let owner = Caller::new(
        SubjectId::new("owner"),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    let document = DocumentService::new(
        authored.clone(),
        Arc::new(memory::MemoryDocumentBlobs::default()),
    )
    .create(
        &owner,
        "held-upload",
        CreateDocument {
            title: "Held".into(),
            markdown: "next".into(),
            origin_agent_id: None,
        },
    )
    .await
    .unwrap();
    let record = authored.get(&document.id).await.unwrap().unwrap();
    let next = DocumentSourceReservation {
        owner_hash: digest(b"owner"),
        document_id: document.id,
        sha256: document.sha256,
        bytes: document.size_bytes,
    };
    let permit = original.reserve_source(&next).await.unwrap().unwrap();
    assert_eq!(permit.generation(), 1);
    let reclaimer = ReclamationStore::new(a.clone(), namespace());
    // Upload is held after reservation. Discovery cannot claim its fresh permit.
    reclaimer.discover_page(DateTime::now()).await.unwrap();
    let page = reclaimer
        .accounting_page(None, DateTime::now(), true)
        .await
        .unwrap();
    assert!(page
        .iter()
        .find(|entry| entry.key == object_row(&next).unwrap().get_str("_id").unwrap())
        .unwrap()
        .candidate
        .is_none());
    assert!(a
        .insert_source(
            &record,
            &DocumentSourceWrite {
                source: next.clone(),
                fence: Some(permit.clone())
            }
        )
        .await
        .unwrap());
    let enabled = a.with_generation_writes(true).quota(true);
    assert_eq!(enabled.reserve_source(&next).await.unwrap(), Some(permit));
    assert_eq!(
        foreign.reserve_source(&next).await,
        Err(DocumentError::Unavailable)
    );
    assert_eq!(
        foreign.reserve_source(&source("owner", 2, b"wrong")).await,
        Err(DocumentError::Unavailable)
    );
    assert_eq!(usage(&original, "owner").await, [2, 9, 2]);
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_quota_reclamation_resume_cursor_reaches_the_last_charge() {
    use crate::document_artifacts::object_authority::{self, Authority, State};
    let database = database().await;
    let writer = MongoDocumentStore::new(database.clone())
        .with_generation_writes(true)
        .with_storage_namespace(Some(namespace()));
    let quota = writer.quota(true);
    let count = crate::document_artifacts::reclamation_store::ACCOUNTING_LIMIT as u32 + 1;
    let sources: Vec<_> = (1..=count).map(|id| source("owner", id, b"body")).collect();
    let last = sources.last().unwrap();
    let baseline = activate(&quota, &database, &sources).await;
    quota
        .bind_storage(&baseline.activation, &baseline.digest, &namespace())
        .await
        .unwrap();
    quota.reserve(last).await.unwrap();
    quota
        .rows
        .update_one(
            doc! {"_id":object_authority::key(last)},
            doc! {"$set":{"updated_at":DateTime::from_millis(1)}},
        )
        .await
        .unwrap();
    let broken = object_row(&sources[0]).unwrap();
    quota
        .rows
        .update_one(doc! {"_id":&broken["_id"]}, doc! {"$set":{"bytes":-1_i64}})
        .await
        .unwrap();
    let now = DateTime::now();
    let worker = ReclamationStore::new(writer.clone(), namespace());
    worker.discover_page(now).await.unwrap();
    let authority = quota
        .rows
        .find_one(doc! {"_id":object_authority::key(last)})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        Authority::decode(&authority).unwrap().state,
        State::Reserved
    );
    assert!(quota
        .rows
        .find_one(doc! {"kind":"reclamation_repair","row_key":&broken["_id"],"lane":"discovery"})
        .await
        .unwrap()
        .is_some());
    let restarted = ReclamationStore::new(writer, namespace());
    restarted.discover_page(now).await.unwrap();
    let authority = quota
        .rows
        .find_one(doc! {"_id":object_authority::key(last)})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        Authority::decode(&authority).unwrap().state,
        State::DeletePending
    );
    assert_eq!(
        usage(&quota, "owner").await,
        [u64::from(count), u64::from(count) * 4, u64::from(count)]
    );
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_quota_reclamation_requires_terminal_proof_for_contradictory_published_history() {
    use crate::document_artifacts::object_authority::{self, Authority, State};
    let database = database().await;
    let writer = MongoDocumentStore::new(database.clone())
        .with_source_commits(true)
        .with_generation_writes(true)
        .with_storage_namespace(Some(namespace()));
    let quota = writer.quota(true);
    let baseline = activate(&quota, &database, &[]).await;
    quota
        .bind_storage(&baseline.activation, &baseline.digest, &namespace())
        .await
        .unwrap();
    let service = DocumentService::with_storage_quota(
        Arc::new(writer.clone()),
        Arc::new(memory::MemoryDocumentBlobs::default()),
        false,
        Some(Arc::new(writer.quota(true))),
    )
    .with_deletion_enabled(true);
    let owner = Caller::new(
        SubjectId::new("owner"),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    let created = service
        .create(
            &owner,
            "create",
            CreateDocument {
                title: "First".into(),
                markdown: "body".into(),
                origin_agent_id: None,
            },
        )
        .await
        .unwrap();
    service
        .update(
            &owner,
            &created.id,
            "update",
            UpdateDocument {
                expected_revision: 1,
                title: Some("Second".into()),
                markdown: Some("replacement".into()),
            },
        )
        .await
        .unwrap();
    let mut contradictory = writer.get(&created.id).await.unwrap().unwrap();
    contradictory.revisions.remove(0);
    writer
        .rows
        .replace_one(
            doc! {"_id":&created.id},
            crate::document_artifacts::mongo::encode(&contradictory).unwrap(),
        )
        .await
        .unwrap();
    let first = DocumentSourceReservation {
        owner_hash: digest(b"owner"),
        document_id: created.id.clone(),
        sha256: created.sha256,
        bytes: created.size_bytes,
    };
    quota
        .rows
        .update_one(
            doc! {"_id":object_authority::key(&first)},
            doc! {"$set":{"updated_at":DateTime::from_millis(1)}},
        )
        .await
        .unwrap();
    let reclaimer = ReclamationStore::new(writer, namespace());
    let now = DateTime::now();
    let page = reclaimer.accounting_page(None, now, false).await.unwrap();
    let candidate = page
        .iter()
        .find_map(|entry| {
            entry
                .candidate
                .as_ref()
                .filter(|candidate| candidate.source == first)
        })
        .unwrap();
    assert!(reclaimer.claim(candidate, now).await.unwrap().is_none());
    let marker = quota
        .rows
        .find_one(doc! {"_id":object_authority::key(&first)})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(Authority::decode(&marker).unwrap().state, State::Published);
    service
        .delete(&owner, &created.id, "delete", 2)
        .await
        .unwrap();
    let claim = reclaimer.claim(candidate, now).await.unwrap().unwrap();
    reclaimer.settle(&claim, confirmed(&claim)).await.unwrap();
    assert_eq!(usage(&quota, "owner").await, [1, 11, 1]);
    database.drop().await.unwrap();
}
