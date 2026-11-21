use super::*;
use crate::store::migrations::{activity_page_index as page, steps::Step};
use futures_util::TryStreamExt as _;

#[test]
fn activity_page_index_is_explicit_nonunique_and_refuses_drift() {
    let index = page::indexes()[0].clone();
    assert_eq!(page::missing(&[]).unwrap(), vec![index.clone()]);
    assert!(page::missing(std::slice::from_ref(&index))
        .unwrap()
        .is_empty());
    let changes: [(&str, mongodb::bson::Bson); 8] = [
        ("unique", true.into()),
        ("sparse", true.into()),
        ("hidden", true.into()),
        ("expireAfterSeconds", 1_i64.into()),
        ("partialFilterExpression", doc! {"live":true}.into()),
        ("collation", doc! {"locale":"en"}.into()),
        ("name", "different-name".into()),
        ("key", doc! {"_id":-1,"recorded_at":-1}.into()),
    ];
    for (key, value) in changes {
        let mut changed = index.clone();
        changed.insert(key, value);
        assert!(page::missing(&[changed]).is_err(), "{key}");
    }
    assert!(page::missing(&[index.clone(), index]).is_err());
    assert!(
        page::missing(&[doc! {"name":"_id_","key":{"_id":1},"collation":{"locale":"en"}}]).is_err()
    );
    assert_eq!(Step::parse(page::ID).unwrap(), Step::ActivityPage);
    assert_eq!(Step::ActivityPage.max_changes(), 1);
    let role = authority::role_for("fixture", true, Step::ActivityPage);
    let activity = role.get_array("privileges").unwrap()[1]
        .as_document()
        .unwrap();
    assert_eq!(
        activity.get_array("actions").unwrap(),
        &vec![
            mongodb::bson::Bson::from("listIndexes"),
            "createIndex".into()
        ]
    );
}

#[tokio::test]
#[ignore = "requires disposable replica-set MongoDB"]
async fn activity_page_migration_is_explicit_idempotent_and_retains_prior_indexes() {
    let (client, database, _) = fixture().await;
    assert!(page::inspect(&database).await.is_err());
    database
        .run_command(doc! {"createIndexes":"activity","indexes":[
            {"name":"activity_expiry","key":{"recorded_at":1},"expireAfterSeconds":90_i64 * 86400}
        ]})
        .await
        .unwrap();
    for index in activity_indexes::indexes() {
        activity_indexes::create(&database, index).await.unwrap();
    }
    let store = crate::store::activity::MongoActivityStore::open(
        &client,
        database.name(),
        Duration::from_secs(90 * 86400),
    )
    .await
    .unwrap();
    let collection = database.collection::<Document>("activity");
    let prior: Vec<_> = collection
        .list_indexes()
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    assert_eq!(page::inspect(&database).await.unwrap().len(), 1);
    let ledger = ledger::Ledger::for_step(client, database.clone(), Step::ActivityPage);
    let first = apply_indexes(&ledger, &database, &identity())
        .await
        .unwrap();
    assert_eq!(first["created_indexes"], 1);
    assert_eq!(
        apply_indexes(&ledger, &database, &identity())
            .await
            .unwrap()["outcome"],
        "already_completed"
    );
    let after: Vec<_> = collection
        .list_indexes()
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    assert_eq!(after.len(), prior.len() + 1);
    for old in prior {
        assert!(after
            .iter()
            .any(|new| mongodb::bson::serialize_to_document(new).unwrap()
                == mongodb::bson::serialize_to_document(&old).unwrap()));
    }
    drop(store);
    collection
        .drop_index("activity_recorded_at_id_desc")
        .await
        .unwrap();
    assert!(apply_indexes(&ledger, &database, &identity())
        .await
        .is_err());
    database.drop().await.unwrap();
}
