use super::*;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use cmaeg_core::seal::seal_connection;

#[tokio::test]
#[ignore = "requires a disposable Mongo replica set"]
async fn creator_cancellation_and_initial_reservation_share_a_permanent_fence() {
    let uri = std::env::var("CMA_IM_TEST_MONGODB_URI").unwrap();
    let first = mongodb::Client::with_uri_str(&uri).await.unwrap();
    let second = mongodb::Client::with_uri_str(&uri).await.unwrap();
    let db_name = format!("create_fence_{}", uuid::Uuid::new_v4().simple());
    let db = first.database(&db_name);
    super::super::migrate(&db).await.unwrap();
    let issuer = "https://issuer.example";
    let (floor, ordinary) = tokio::join!(
        super::super::migrate_create_fence_floor(&db, issuer),
        super::super::migrate(&db),
    );
    ordinary.unwrap();
    if let Err(error) = floor {
        assert_eq!(
            error,
            RecordError::Unavailable(
                "Channel validator migration is in progress or unresolved".into()
            )
        );
        super::super::migrate_create_fence_floor(&db, issuer)
            .await
            .unwrap();
    }
    assert!(super::super::create_fences_floor::verify(&db, issuer)
        .await
        .unwrap());
    let keys = MasterKeyRing::parse(&format!("1:{}", STANDARD.encode([42; 32]))).unwrap();
    let a = MongoRecords::read_failure_fixture(db.clone());
    let b = MongoRecords::read_failure_fixture(second.database(&db_name));
    let legacy_record = uuid::Uuid::new_v4().to_string();
    assert!(matches!(
        a.put(&legacy_record, "alice", b"old writer", None).await,
        Err(RecordError::Unavailable(_)),
    ));
    assert_eq!(b.get(&legacy_record).await, Err(RecordError::NotFound));
    let missing = uuid::Uuid::new_v4().to_string();
    let alice = CreateFenceIdentity {
        issuer,
        kind: "human",
        subject: "alice",
        record: &missing,
    };
    let bob = CreateFenceIdentity {
        subject: "bob",
        ..alice
    };
    assert_eq!(
        a.cancel_channel_create(&alice, &keys).await.unwrap(),
        CancelCreate::FencedAbsent,
    );
    let connection = uuid::Uuid::new_v4().to_string();
    let connection_rows = a.for_kind(RecordKind::Connection);
    let before = seal_connection(&keys, &connection, br#"{"fixture":"before claim"}"#).unwrap();
    let after = seal_connection(&keys, &connection, br#"{"fixture":"after claim"}"#).unwrap();
    connection_rows
        .put(&connection, "alice", &before, None)
        .await
        .unwrap();
    let claim = CreateConnectionClaim {
        record_id: connection.clone(),
        expected_revision: 1,
        sealed_payload: after.clone(),
    };
    assert_eq!(
        b.reserve_channel_create(&alice, b"fixture", Some(&claim))
            .await,
        Err(RecordError::GuardConflict {
            scope: "channel_create",
            expected: 0
        }),
    );
    assert_eq!(connection_rows.get(&connection).await.unwrap().version, 1);
    assert_eq!(
        b.reserve_channel_create(&bob, b"fixture", None)
            .await
            .unwrap(),
        1
    );
    assert!(matches!(
        b.reserve_channel_create(&alice, b"fixture", Some(&claim))
            .await,
        Err(RecordError::GuardConflict {
            scope: "channel_create",
            expected: 0
        }),
    ));
    assert_eq!(connection_rows.get(&connection).await.unwrap().version, 1);
    assert_eq!(
        a.cancel_channel_create(&bob, &keys).await.unwrap(),
        CancelCreate::RecordPresent,
    );
    assert_eq!(
        b.cancel_channel_create(&alice, &keys).await.unwrap(),
        CancelCreate::FencedAbsent,
    );
    assert!(b.delete(&missing, 1).await.unwrap());
    assert_eq!(
        a.cancel_channel_create(&bob, &keys).await.unwrap(),
        CancelCreate::FencedAbsent,
    );
    assert_eq!(
        b.reserve_channel_create(&bob, b"fixture", None).await,
        Err(RecordError::GuardConflict {
            scope: "channel_create",
            expected: 0
        }),
    );
    let live = uuid::Uuid::new_v4().to_string();
    let admitted = CreateFenceIdentity {
        record: &live,
        ..alice
    };
    assert_eq!(
        a.reserve_channel_create(&admitted, b"fixture", Some(&claim))
            .await
            .unwrap(),
        1,
    );
    assert_eq!(connection_rows.get(&connection).await.unwrap().blob, after);
    let another = uuid::Uuid::new_v4().to_string();
    let stale = CreateFenceIdentity {
        record: &another,
        ..alice
    };
    assert_eq!(
        b.reserve_channel_create(&stale, b"fixture", Some(&claim))
            .await,
        Err(RecordError::GuardConflict {
            scope: "connection",
            expected: 1
        }),
    );
    assert_eq!(a.get(&another).await, Err(RecordError::NotFound));

    for _ in 0..16 {
        let id = uuid::Uuid::new_v4().to_string();
        let racing = CreateFenceIdentity {
            record: &id,
            ..alice
        };
        let (reserved, cancelled) = tokio::join!(
            a.reserve_channel_create(&racing, b"racing create", None),
            b.cancel_channel_create(&racing, &keys),
        );
        match cancelled.unwrap() {
            CancelCreate::FencedAbsent => {
                assert_eq!(
                    reserved,
                    Err(RecordError::GuardConflict {
                        scope: "channel_create",
                        expected: 0,
                    }),
                );
                assert_eq!(a.get(&id).await, Err(RecordError::NotFound));
            }
            CancelCreate::CreatePending => panic!("no claim for racing identity"),
            CancelCreate::RecordPresent => {
                assert_eq!(reserved.unwrap(), 1);
                assert_eq!(a.get(&id).await.unwrap().blob, b"racing create");
            }
        }
    }
    let orphan_id = uuid::Uuid::new_v4().to_string();
    let orphan = CreateFenceIdentity {
        record: &orphan_id,
        ..alice
    };
    let mut foreign = crate::connections::fixture_connection("bob", false);
    foreign.channel_record = Some(orphan_id.clone());
    let foreign_id = foreign.id.clone();
    let stored = crate::connections::model::Stored::Connection(Box::new(foreign));
    let sealed =
        seal_connection(&keys, &foreign_id, &serde_json::to_vec(&stored).unwrap()).unwrap();
    connection_rows
        .put(&foreign_id, "bob", &sealed, None)
        .await
        .unwrap();
    assert_eq!(
        a.cancel_channel_create(&orphan, &keys).await.unwrap(),
        CancelCreate::FencedAbsent
    );
    let owned_id = uuid::Uuid::new_v4().to_string();
    let owned = CreateFenceIdentity {
        record: &owned_id,
        ..alice
    };
    let mut claim = crate::connections::fixture_connection("alice", false);
    claim.channel_record = Some(owned_id.clone());
    let claim_id = claim.id.clone();
    let stored = crate::connections::model::Stored::Connection(Box::new(claim));
    let sealed = seal_connection(&keys, &claim_id, &serde_json::to_vec(&stored).unwrap()).unwrap();
    connection_rows
        .put(&claim_id, "alice", &sealed, None)
        .await
        .unwrap();
    assert_eq!(
        b.cancel_channel_create(&owned, &keys).await.unwrap(),
        CancelCreate::CreatePending
    );
    assert_eq!(a.get(&owned_id).await, Err(RecordError::NotFound));
    let mut unrelated = Vec::new();
    for _ in 0..257 {
        let id = uuid::Uuid::new_v4().to_string();
        let sealed = seal_connection(&keys, &id, br#"{"fixture":"unrelated"}"#).unwrap();
        unrelated.push(doc! {
            "_id":id, "owner":"alice", "version":1_i64, "schema_version":1,
            "blob": Binary {subtype: BinarySubtype::Generic, bytes: sealed},
        });
    }
    db.collection::<Document>(CONNECTIONS)
        .insert_many(unrelated)
        .await
        .unwrap();
    assert_eq!(
        b.cancel_channel_create(&alice, &keys).await.unwrap(),
        CancelCreate::FencedAbsent,
    );
    let new_id = uuid::Uuid::new_v4().to_string();
    let never_cancelled = CreateFenceIdentity {
        record: &new_id,
        ..alice
    };
    assert!(matches!(
        b.cancel_channel_create(&never_cancelled, &keys).await,
        Err(RecordError::Unavailable(_)),
    ));
    db.run_command(doc! {"collMod": super::super::RECORDS, "validationAction": "warn"})
        .await
        .unwrap();
    assert!(super::super::create_fences_floor::verify(&db, issuer)
        .await
        .is_err());
    assert!(matches!(
        b.cancel_channel_create(&alice, &keys).await,
        Err(RecordError::Unavailable(_))
    ));
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable Mongo replica set"]
async fn create_fence_upgrades_populated_channel_records_from_the_old_validator() {
    let uri = std::env::var("CMA_IM_TEST_MONGODB_URI").unwrap();
    let client = mongodb::Client::with_uri_str(&uri).await.unwrap();
    let db = client.database(&format!(
        "old_channel_validator_{}",
        uuid::Uuid::new_v4().simple()
    ));
    let records = MongoRecords::read_failure_fixture(db.clone());
    super::super::migrate(&db).await.unwrap();
    let mut old_validator = super::super::schema::record_validator(false);
    old_validator
        .get_document_mut("$jsonSchema")
        .unwrap()
        .get_document_mut("properties")
        .unwrap()
        .remove("guard_revision");
    db.run_command(doc! {"collMod": super::super::RECORDS, "validator": old_validator})
        .await
        .unwrap();

    let existing = uuid::Uuid::new_v4().to_string();
    records
        .put(&existing, "alice", b"retained", None)
        .await
        .unwrap();
    super::super::migrate_create_fence_floor(&db, "https://issuer.example")
        .await
        .unwrap();
    assert_eq!(records.get(&existing).await.unwrap().blob, b"retained");
    let row = db
        .collection::<Document>(super::super::RECORDS)
        .find_one(doc! {"_id": existing})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.get_i64("guard_revision").unwrap(), 1);
    assert!(
        super::super::create_fences_floor::verify(&db, "https://issuer.example")
            .await
            .unwrap()
    );
    db.drop().await.unwrap();
}
