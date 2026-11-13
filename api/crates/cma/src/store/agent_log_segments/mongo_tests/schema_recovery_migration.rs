//! Recovery indexes are deliberately nonunique; existing segment keys stay unique.
use super::*;
use mongodb::{options::Collation, IndexModel};

async fn named_index(db: &mongodb::Database, collection: &str, name: &str) -> IndexModel {
    db.collection::<Document>(collection)
        .list_indexes()
        .await
        .unwrap()
        .try_collect::<Vec<_>>()
        .await
        .unwrap()
        .into_iter()
        .find(|index| index.options.as_ref().and_then(|o| o.name.as_deref()) == Some(name))
        .unwrap()
}

async fn refuse_drift(
    db: &mongodb::Database,
    collection: &str,
    original: &IndexModel,
    changed: IndexModel,
) {
    let original_name = original.options.as_ref().unwrap().name.as_ref().unwrap();
    let changed_name = changed.options.as_ref().unwrap().name.clone().unwrap();
    let rows = db.collection::<Document>(collection);
    rows.drop_index(original_name).await.unwrap();
    rows.create_index(changed).await.unwrap();
    db.collection::<Document>(HEADS).drop().await.unwrap();
    // Inspect every existing contract before the next additive effect.
    assert!(schema::apply_next(db).await.is_err());
    assert!(!db
        .list_collection_names()
        .await
        .unwrap()
        .iter()
        .any(|name| name == HEADS));
    rows.drop_index(changed_name).await.unwrap();
    rows.create_index(original.clone()).await.unwrap();
    schema::migrate(db).await.unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn recovery_index_drift_refuses_before_effects_without_relaxing_unique_contracts() {
    let (client, name) = database("recovery_indexes").await;
    let db = client.database(&name);
    schema::migrate(&db).await.unwrap();
    for index_name in [
        "agent_capacity_recovery_due",
        "agent_capacity_recovery_expired",
        "agent_capacity_recovery_agent",
    ] {
        let original = named_index(&db, RECOVERY_WORK, index_name).await;
        assert!(!original.options.as_ref().unwrap().unique.unwrap_or(false));
        let mut changed = original.clone();
        changed.options.as_mut().unwrap().unique = Some(true);
        refuse_drift(&db, RECOVERY_WORK, &original, changed).await;
    }
    for (collection, index_name) in [
        (SEGMENTS, "agent_log_segment_ordinal"),
        (writer_admission::COLLECTION, "agent_log_admission_key"),
    ] {
        let original = named_index(&db, collection, index_name).await;
        assert_eq!(original.options.as_ref().unwrap().unique, Some(true));
        let mut changed = original.clone();
        changed.options.as_mut().unwrap().unique = Some(false);
        refuse_drift(&db, collection, &original, changed).await;
    }

    let original = named_index(&db, RECOVERY_WORK, "agent_capacity_recovery_due").await;
    for case in 0..6 {
        let mut changed = original.clone();
        let options = changed.options.as_mut().unwrap();
        match case {
            0 => options.sparse = Some(true),
            1 => options.hidden = Some(true),
            2 => options.partial_filter_expression = Some(doc! {"work.phase":"admitted"}),
            3 => options.collation = Some(Collation::builder().locale("en").build()),
            4 => options.name = Some("unreviewed-recovery-index".into()),
            _ => {
                changed.keys = doc! {"work.phase":1,"format_version":1,"work.ready_at":1,"_id":1};
            }
        }
        refuse_drift(&db, RECOVERY_WORK, &original, changed).await;
    }
    let primary = named_index(&db, RECOVERY_WORK, "_id_").await;
    assert_eq!(primary.keys, doc! {"_id":1});
    assert_eq!(schema::pending_count(&db).await.unwrap(), 0);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn unregistered_recovery_carriers_refuse_migration_without_relabeling() {
    for version in [1_i64, 3_i64] {
        let (client, name) = database("recovery_old").await;
        let db = client.database(&name);
        let rows = db.collection::<Document>(RECOVERY_WORK);
        let old = doc! {"_id":"unregistered-carrier-fixture","format_version":version};
        rows.insert_one(old.clone()).await.unwrap();
        assert!(schema::apply_next(&db).await.is_err());
        assert_eq!(rows.find_one(doc! {}).await.unwrap().unwrap(), old);
        assert_eq!(
            db.list_collection_names().await.unwrap(),
            vec![RECOVERY_WORK]
        );
        db.drop().await.unwrap();
    }
}
