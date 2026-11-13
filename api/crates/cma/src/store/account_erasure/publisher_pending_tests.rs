//! Temporary C1 Mongo contracts; remove this module with publisher_pending in C2.
use super::{tests::*, *};
use mongodb::bson::{doc, Document};

fn profile(owner: &str, id: &str) -> Document {
    doc! {
        "_id":id,"document_schema_version":1_i64,"owner":owner,
        "draft_version":1_i64,"last_revision":1_i64,
        "record":{"id":id,"owner":owner,"draft_version":1_i64,
            "draft":{"private_authoring":"must survive the pending refusal"}},
    }
}

fn revision(id: &str) -> Document {
    doc! {
        "_id":format!("{id}:0000000001"),"document_schema_version":1_i64,
        "profile_id":id,"revision":1_i64,
        "record":{"profile_id":id,"revision":1_i64,"content":{}},
    }
}

fn user(subject: &str) -> Document {
    doc! {"_id":subject,"name":"must survive until the publisher protocol is present"}
}

async fn unchanged(db: &Database, rows: &[(&str, Document)]) {
    for (collection, expected) in rows {
        let actual = db
            .collection::<Document>(collection)
            .find_one(doc! {"_id":expected.get("_id").unwrap().clone()})
            .await
            .unwrap();
        assert_eq!(actual, Some(expected.clone()), "changed {collection}");
    }
}

fn pending(error: Error, subject: &str, collection: &str) {
    let Error::Blocked(block) = error else {
        panic!("publisher preflight must permanently block before effects: {error:?}");
    };
    assert_eq!(
        block.subject_hash,
        layer2_core::account_deletion::subject_hash(&SubjectId::new(subject))
    );
    assert_eq!(
        block.reason,
        ErasureBlock::RetainedReferencePending {
            collection: collection.into()
        }
    );
}

async fn refuse(db: &Database, subject: &str, collection: &str) {
    let subject_id = SubjectId::new(subject);
    let inventory = ErasureInventory::default();
    let targets = [Target {
        subject: &subject_id,
        inventory: &inventory,
    }];
    let mut cursor = Cursor::default();
    for _ in 0..3 {
        let error = erase_records(db, &targets, &mut cursor, NOW)
            .await
            .unwrap_err();
        pending(error, subject, collection);
        cursor = serde_json::from_str(&serde_json::to_string(&cursor).unwrap()).unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn c1_publisher_unrelated_owner_profile_does_not_block_ordinary_erasure() {
    let db = database().await;
    sandbox(&db, "a", "sbx_a").await;
    put(
        &db,
        "agents",
        doc! {"_id":"agt_a","owner":"a","sandbox_id":"sbx_a"},
    )
    .await;
    put(
        &db,
        "agent_log",
        doc! {"_id":"owned-log","agent_id":"agt_a"},
    )
    .await;
    put(&db, "users", user("a")).await;
    let survivors = [
        ("agent_profiles", profile("b", "agp_b")),
        ("agent_profile_revisions", revision("agp_b")),
        (
            "agents",
            doc! {"_id":"agt_b","owner":"b","record":{
                "profile":{"id":"agp_b","revision":1_i64}
            }},
        ),
    ];
    for (collection, row) in &survivors {
        put(&db, collection, row.clone()).await;
    }
    let mut inventory = ErasureInventory::default();
    let deleted = drain(&db, "a", &mut inventory).await;
    assert_eq!(deleted.get("sandboxes"), Some(&1));
    assert_eq!(deleted.get("agents"), Some(&1));
    assert_eq!(deleted.get("agent_log"), Some(&1));
    assert_eq!(deleted.get("users"), Some(&1));
    unchanged(&db, &survivors).await;
    assert!(drain(&db, "a", &mut inventory)
        .await
        .values()
        .all(|count| *count == 0));
    unchanged(&db, &survivors).await;
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn c1_publisher_owned_profile_with_foreign_pin_is_permanently_refused_before_effects() {
    for (aggregates, revisions, id) in [
        ("agent_profiles", "agent_profile_revisions", "agp_a"),
        ("workspace_profiles", "workspace_profile_revisions", "wkp_a"),
        ("sandbox_profiles", "sandbox_profile_revisions", "sbp_a"),
    ] {
        let db = database().await;
        let rows = [
            (aggregates, profile("a", id)),
            (revisions, revision(id)),
            (
                "sandboxes",
                doc! {"_id":"sbx_survivor","owner":"visitor","record":{
                    "profile":{"id":id,"revision":1_i64}
                }},
            ),
            ("users", user("a")),
        ];
        for (collection, row) in &rows {
            put(&db, collection, row.clone()).await;
        }
        refuse(&db, "a", aggregates).await;
        unchanged(&db, &rows).await;
        db.drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn c1_publisher_inline_trigger_revision_with_foreign_launch_blocks_without_profiles() {
    let db = database().await;
    let rows = [
        (
            "trigger_definition_revisions",
            doc! {
                "_id":"trg_inline:0000000001","document_schema_version":1_i64,
                "trigger_id":"trg_inline","public_id":"pub_inline","owner":"a",
                "revision":1_i64,"sandbox_profile_id":"","workspace_profile_id":"",
                "agent_profile_id":"","published_at":NOW,
                "record":{"revision":1_i64,"digest":"original-digest",
                    "published_at":NOW,"content":{"target":{"inline":{
                        "agent":{"system_prompt":"publisher secret"}
                    }},"first_message":"publisher message"}},
            },
        ),
        (
            "trigger_launches",
            doc! {
                "_id":"launch_foreign","owner":"visitor","trigger_id":"trg_inline",
                "record":{"owner":"visitor","trigger_id":"trg_inline",
                    "revision_digest":"original-digest","consent":"retained"},
            },
        ),
        ("users", user("a")),
    ];
    for (collection, row) in &rows {
        put(&db, collection, row.clone()).await;
    }
    for collection in ["agent_profiles", "workspace_profiles", "sandbox_profiles"] {
        assert_eq!(count(&db, collection).await, 0);
    }
    refuse(&db, "a", "trigger_definition_revisions").await;
    unchanged(&db, &rows).await;
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn c1_publisher_owned_trigger_definition_is_not_deleted_while_support_is_pending() {
    let db = database().await;
    let rows = [
        (
            "trigger_definitions",
            doc! {"_id":"trg_owned","owner":"a","record":{
                "id":"trg_owned","owner":"a","draft":{"private":"draft"}
            }},
        ),
        ("users", user("a")),
    ];
    for (collection, row) in &rows {
        put(&db, collection, row.clone()).await;
    }
    refuse(&db, "a", "trigger_definitions").await;
    unchanged(&db, &rows).await;
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn c1_publisher_unused_owned_profile_is_conservatively_refused() {
    let db = database().await;
    let rows = [
        ("agent_profiles", profile("a", "agp_unused")),
        ("agent_profile_revisions", revision("agp_unused")),
        ("users", user("a")),
    ];
    for (collection, row) in &rows {
        put(&db, collection, row.clone()).await;
    }
    refuse(&db, "a", "agent_profiles").await;
    unchanged(&db, &rows).await;
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn c1_publisher_resumed_cohort_attributes_new_residue_to_its_current_owner() {
    let db = database().await;
    let rows = [("users", user("a")), ("users", user("b"))];
    for (collection, row) in &rows {
        put(&db, collection, row.clone()).await;
    }
    let a = SubjectId::new("a");
    let b = SubjectId::new("b");
    let inventory = ErasureInventory::default();
    let targets = [
        Target {
            subject: &a,
            inventory: &inventory,
        },
        Target {
            subject: &b,
            inventory: &inventory,
        },
    ];
    let mut cursor = Cursor::default();
    let first = erase_records(&db, &targets, &mut cursor, NOW)
        .await
        .unwrap();
    assert!(
        first.has_more,
        "billing preflight must leave a resumable page"
    );
    assert!(first.deleted.is_empty() && first.redacted.is_empty());
    let publisher = profile("b", "agp_new_b");
    put(&db, "agent_profiles", publisher.clone()).await;
    for _ in 0..3 {
        cursor = serde_json::from_str(&serde_json::to_string(&cursor).unwrap()).unwrap();
        let error = erase_records(&db, &targets, &mut cursor, NOW)
            .await
            .unwrap_err();
        pending(error, "b", "agent_profiles");
        unchanged(&db, &rows).await;
        unchanged(&db, &[("agent_profiles", publisher.clone())]).await;
    }
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn c1_publisher_revision_operation_guard_uses_the_live_parent_owner() {
    let db = database().await;
    let aggregate = profile("b", "agp_parent");
    let row = revision("agp_parent");
    put(&db, "agent_profiles", aggregate).await;
    put(&db, "agent_profile_revisions", row.clone()).await;
    let subjects = BTreeSet::from(["a".to_owned()]);
    publisher_pending::guard_row(&db, "agent_profile_revisions", &row, &subjects)
        .await
        .unwrap();
    db.collection::<Document>("agent_profiles")
        .update_one(
            doc! {"_id":"agp_parent"},
            doc! {"$set":{"owner":"a","record.owner":"a"}},
        )
        .await
        .unwrap();
    let error = publisher_pending::guard_row(&db, "agent_profile_revisions", &row, &subjects)
        .await
        .unwrap_err();
    pending(error, "a", "agent_profile_revisions");
    unchanged(&db, &[("agent_profile_revisions", row)]).await;
    db.drop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn c1_publisher_selected_definition_transferred_to_foreign_owner_is_never_deleted() {
    use mongodb::{
        event::{command::CommandEvent, EventHandler},
        options::ClientOptions,
        Client,
    };
    use std::sync::{
        atomic::{AtomicI32, AtomicU8, Ordering},
        mpsc, Arc, Mutex,
    };

    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let notice = Arc::new(tokio::sync::Notify::new());
    let notify = notice.clone();
    let (release, receiver) = mpsc::channel();
    let receiver = Mutex::new(receiver);
    let stage = AtomicU8::new(0);
    let request = AtomicI32::new(0);
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        let pause = match event {
            CommandEvent::Started(event) => {
                let selected_page = event.command.get_str("find").ok()
                    == Some("trigger_definitions")
                    && event
                        .command
                        .get_document("filter")
                        .is_ok_and(|filter| filter.get_str("owner").ok() == Some("a"))
                    && event
                        .command
                        .get_document("hint")
                        .is_ok_and(|hint| *hint == doc! {"owner":1,"_id":1});
                if selected_page
                    && stage
                        .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
                        .is_ok()
                {
                    request.store(event.request_id, Ordering::SeqCst);
                    true
                } else {
                    false
                }
            }
            CommandEvent::Succeeded(event)
                if event.request_id == request.load(Ordering::SeqCst)
                    && stage
                        .compare_exchange(1, 2, Ordering::SeqCst, Ordering::SeqCst)
                        .is_ok() =>
            {
                let rows = event
                    .reply
                    .get_document("cursor")
                    .unwrap()
                    .get_array("firstBatch")
                    .unwrap();
                assert!(rows.iter().any(|row| {
                    row.as_document().is_some_and(|row| {
                        row.get_str("_id").ok() == Some("trg_transfer")
                            && row.get_str("owner").ok() == Some("a")
                    })
                }));
                true
            }
            _ => false,
        };
        if pause {
            notify.notify_one();
            receiver
                .lock()
                .unwrap()
                .recv_timeout(std::time::Duration::from_secs(30))
                .unwrap();
        }
    }));
    let name = format!(
        "cma_c1_publisher_transfer_{}",
        uuid::Uuid::new_v4().simple()
    );
    let observed = Client::with_options(options).unwrap().database(&name);
    let independent = Client::with_uri_str(&uri).await.unwrap().database(&name);
    let original = doc! {"_id":"trg_transfer","owner":"b","record":{
        "id":"trg_transfer","owner":"b","draft":{"private":"foreign data"}
    }};
    put(&independent, "trigger_definitions", original.clone()).await;
    put(&independent, "users", user("a")).await;
    let task = tokio::spawn(async move {
        let subject = SubjectId::new("a");
        let mut inventory = ErasureInventory::default();
        let mut cursor = Cursor::default();
        let mut deleted = BTreeMap::<String, u64>::new();
        for _ in 0..1000 {
            let batch = erase_records(
                &observed,
                &[Target {
                    subject: &subject,
                    inventory: &inventory,
                }],
                &mut cursor,
                NOW,
            )
            .await?;
            progress::acknowledge(&mut inventory, &batch, "a");
            for (name, count) in batch.deleted {
                *deleted.entry(name).or_default() += count;
            }
            if !batch.has_more {
                return Ok(deleted);
            }
        }
        Err(Error::Store(
            "publisher transfer test failed to converge".into(),
        ))
    });

    // All preflight and ownership reads saw B. Make A visible only to the
    // already-issued effect page, then restore B after that page was read.
    tokio::time::timeout(std::time::Duration::from_secs(20), notice.notified())
        .await
        .unwrap();
    independent
        .collection::<Document>("trigger_definitions")
        .update_one(
            doc! {"_id":"trg_transfer"},
            doc! {"$set":{"owner":"a","record.owner":"a"}},
        )
        .await
        .unwrap();
    release.send(()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(20), notice.notified())
        .await
        .unwrap();
    independent
        .collection::<Document>("trigger_definitions")
        .replace_one(doc! {"_id":"trg_transfer"}, original.clone())
        .await
        .unwrap();
    release.send(()).unwrap();

    let deleted = task.await.unwrap().unwrap();
    assert_eq!(deleted.get("trigger_definitions"), None);
    assert_eq!(deleted.get("users"), Some(&1));
    unchanged(&independent, &[("trigger_definitions", original)]).await;
    independent.drop().await.unwrap();
}
