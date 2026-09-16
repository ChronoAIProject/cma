use super::*;
use futures_util::TryStreamExt;

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn retained_named_renewal_index_survives_allocation_open_and_discovery_reopen() {
    let (client, database, bridges) =
        crate::store::bridge_definitions::mongo_integration_tests::fixture().await;
    let rows = client
        .database(&database)
        .collection::<Document>("bridge_allocations");
    // This exact index name/key is written by the earlier renewal initializer.
    // Seed it before the new allocation store has initialized retention lookups.
    rows.create_index(
        mongodb::IndexModel::builder()
            .keys(doc! {"sandbox_admission.sandbox_id":1})
            .options(
                mongodb::options::IndexOptions::builder()
                    .name("channel_sandbox_origin".to_owned())
                    .build(),
            )
            .build(),
    )
    .await
    .unwrap();
    let existing = rows
        .list_indexes()
        .await
        .unwrap()
        .try_collect::<Vec<_>>()
        .await
        .unwrap()
        .into_iter()
        .find(|index| {
            index
                .options
                .as_ref()
                .and_then(|options| options.name.as_deref())
                == Some("channel_sandbox_origin")
        })
        .unwrap();
    for _ in 0..2 {
        let store =
            crate::store::MongoBridgeAllocationStore::open(bridges.clone(), None, &database)
                .await
                .unwrap();
        store.initialize_channel_renewal_discovery().await.unwrap();
        let matches = rows
            .list_indexes()
            .await
            .unwrap()
            .try_collect::<Vec<_>>()
            .await
            .unwrap()
            .into_iter()
            .filter(|index| index.keys == existing.keys)
            .collect::<Vec<_>>();
        assert_eq!(matches.len(), 1);
        assert_eq!(
            mongodb::bson::serialize_to_document(&matches[0]).unwrap(),
            mongodb::bson::serialize_to_document(&existing).unwrap()
        );
    }
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn renewal_discovery_is_due_bounded_and_advances_past_corrupt_rows_without_manual_work() {
    let f = RenewalFixture::new().await;
    f.original
        .allocations
        .initialize_channel_renewal_discovery()
        .await
        .unwrap();
    assert!(f
        .original
        .allocations
        .due_channel_renewal_sandboxes(None, SystemTime::now())
        .await
        .unwrap()
        .is_empty());
    let sandbox_id = f.original.record.placement.target().sandbox_id.as_str();
    assert_eq!(
        f.original
            .allocations
            .due_channel_renewal_sandboxes(None, f.due)
            .await
            .unwrap(),
        vec![sandbox_id.to_owned()]
    );
    assert!(
        f.original
            .allocations
            .due_channel_renewal_record(sandbox_id, f.due)
            .await
            .unwrap()
            .unwrap()
            == f.original.record
    );
    let rows = f
        .original
        .client
        .database(&f.original.database)
        .collection::<Document>("sandboxes");
    let mut corrupt = doc! {"_id":"000-corrupt","owner":"fixture-discovery","name":"corrupt","status":"active","channel_creation":{},"channel_provisioning":{"phase":"ready"},
    "record":{"credential":{"current":{"issued_at":"2000-01-01T00:00:00Z","expires_at":"2000-01-02T00:00:00Z"}}}};
    rows.insert_one(corrupt.clone()).await.unwrap();
    corrupt.insert("_id", "001-manual-not-channel");
    corrupt.insert("name", "manual-not-channel");
    corrupt.remove("channel_creation");
    rows.insert_one(corrupt).await.unwrap();
    let discovered = f
        .original
        .allocations
        .due_channel_renewal_sandboxes(None, f.due)
        .await
        .unwrap();
    assert_eq!(
        discovered,
        vec!["000-corrupt".to_owned(), sandbox_id.to_owned()]
    );
    assert!(f
        .original
        .allocations
        .due_channel_renewal_record(&discovered[0], f.due)
        .await
        .is_err());
    assert_eq!(
        f.original
            .allocations
            .due_channel_renewal_sandboxes(Some(&discovered[0]), f.due)
            .await
            .unwrap(),
        vec![sandbox_id.to_owned()]
    );
    rows.delete_one(doc! {"_id":"000-corrupt"}).await.unwrap();
    rows.delete_one(doc! {"_id":"001-manual-not-channel"})
        .await
        .unwrap();
    f.step().await.unwrap();
    assert_eq!(
        f.original
            .allocations
            .due_channel_renewal_sandboxes(None, SystemTime::now())
            .await
            .unwrap(),
        vec![sandbox_id.to_owned()]
    );
    f.original
        .allocations
        .defer_channel_renewal(sandbox_id, Duration::from_secs(30), "authority_required")
        .await
        .unwrap();
    assert!(f
        .original
        .allocations
        .due_channel_renewal_sandboxes(None, SystemTime::now())
        .await
        .unwrap()
        .is_empty());
    assert!(f
        .original
        .allocations
        .due_channel_renewal_record(sandbox_id, SystemTime::now())
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        f.original
            .allocations
            .due_channel_renewal_sandboxes(None, SystemTime::now() + Duration::from_secs(31))
            .await
            .unwrap(),
        vec![sandbox_id.to_owned()]
    );
    f.close().await;
}

pub(super) async fn require_renewal_audits(f: &RenewalFixture) {
    let rows = f
        .original
        .client
        .database(&f.original.database)
        .collection::<Document>("channel_credential_renewal_audits");
    let mut cursor = rows
        .find(doc! {"sandbox_id":f.original.record.placement.target().sandbox_id.as_str()})
        .sort(doc! {"revision":1})
        .await
        .unwrap();
    let mut actions = Vec::new();
    while let Some(audit) = cursor.try_next().await.unwrap() {
        assert_eq!(
            audit.get_str("allocation_id").unwrap(),
            f.original.record.id
        );
        assert_eq!(
            audit.get_str("human_subject").unwrap(),
            f.original.lease.actor_subject().as_str()
        );
        assert_eq!(
            audit.get_document("human_actor").unwrap(),
            &mongodb::bson::serialize_to_document(&f.original.lease.actor_context().actor).unwrap()
        );
        assert_eq!(audit.get_str("grant_digest").unwrap().len(), 64);
        assert_eq!(audit.get_i64("generation").unwrap(), 2);
        assert_eq!(audit.get_i64("revision").unwrap(), actions.len() as i64 + 1);
        let serialized = format!("{audit:?}");
        assert!(!serialized.contains(crate::auth::keys::bridge::test_fixture::HUMAN_SECRET));
        assert!(!serialized.contains("fixture-runtime-renewed"));
        actions.push(audit.get_str("action").unwrap().to_owned());
    }
    assert_eq!(
        actions,
        [
            "mint_intent",
            "mint_receipt",
            "resume",
            "delivery_receipt",
            "resume",
            "revocation_receipt"
        ]
    );
}
