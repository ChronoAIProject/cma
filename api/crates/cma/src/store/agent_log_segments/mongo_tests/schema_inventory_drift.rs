//! Runtime verification refuses incomplete or unreviewed control inventories.
use super::*;
use mongodb::{options::IndexOptions, IndexModel};

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn missing_control_collections_refuse_verification_without_startup_repair() {
    let (client, name) = database("missing_controls").await;
    let db = client.database(&name);
    schema::migrate(&db).await.unwrap();
    for collection in [
        writer_gate::COLLECTION,
        writer_admission::COLLECTION,
        RECOVERY_WORK,
    ] {
        db.collection::<Document>(collection).drop().await.unwrap();
        assert!(schema::verify(&db).await.is_err());
        assert!(schema::pending_count(&db).await.unwrap() > 0);
        assert!(!db
            .list_collection_names()
            .await
            .unwrap()
            .iter()
            .any(|n| n == collection));
        // Only the explicit additive migration restores a partial inventory.
        schema::migrate(&db).await.unwrap();
        assert_eq!(schema::pending_count(&db).await.unwrap(), 0);
        assert_eq!(
            db.collection::<Document>(collection)
                .count_documents(doc! {})
                .await
                .unwrap(),
            0
        );
    }
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn missing_control_indexes_refuse_verification_and_resume_only_explicitly() {
    let (client, name) = database("missing_control_indexes").await;
    let db = client.database(&name);
    schema::migrate(&db).await.unwrap();
    for (collection, index) in [
        (writer_admission::COLLECTION, "agent_log_admission_key"),
        (RECOVERY_WORK, "agent_capacity_recovery_due"),
        (RECOVERY_WORK, "agent_capacity_recovery_expired"),
        (RECOVERY_WORK, "agent_capacity_recovery_agent"),
    ] {
        let rows = db.collection::<Document>(collection);
        rows.drop_index(index).await.unwrap();
        assert!(schema::verify(&db).await.is_err());
        assert_eq!(schema::pending_count(&db).await.unwrap(), 1);
        let names: Vec<_> = rows.list_index_names().await.unwrap();
        assert!(!names.iter().any(|name| name == index));
        assert!(schema::apply_next(&db).await.unwrap());
        schema::verify(&db).await.unwrap();
        assert!(!schema::apply_next(&db).await.unwrap());
    }
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn extra_indexes_refuse_before_additive_effects_on_every_registered_collection() {
    let (client, name) = database("extra_control_indexes").await;
    let db = client.database(&name);
    schema::migrate(&db).await.unwrap();
    for collection in COLLECTIONS {
        for unique in [false, true] {
            let rows = db.collection::<Document>(collection);
            rows.create_index(
                IndexModel::builder()
                    .keys(doc! {"unreviewed_field":1})
                    .options(
                        IndexOptions::builder()
                            .name("unreviewed_extra".to_owned())
                            .unique(unique)
                            .build(),
                    )
                    .build(),
            )
            .await
            .unwrap();
            assert!(schema::verify(&db).await.is_err());
            assert!(schema::pending_count(&db).await.is_err());
            // A missing earlier collection must not conceal drift elsewhere.
            let missing = if *collection == HEADS {
                SEGMENTS
            } else {
                HEADS
            };
            db.collection::<Document>(missing).drop().await.unwrap();
            assert!(schema::apply_next(&db).await.is_err());
            assert!(!db
                .list_collection_names()
                .await
                .unwrap()
                .iter()
                .any(|n| n == missing));
            assert!(rows
                .list_index_names()
                .await
                .unwrap()
                .iter()
                .any(|n| n == "unreviewed_extra"));
            rows.drop_index("unreviewed_extra").await.unwrap();
            schema::migrate(&db).await.unwrap();
        }
    }
    db.drop().await.unwrap();
}
