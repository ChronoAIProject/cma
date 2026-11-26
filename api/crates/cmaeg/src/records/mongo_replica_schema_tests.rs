use super::{migrate, verify, CONNECTIONS, INDEX, INTERACTIONS, MIGRATIONS};
use crate::records::RecordError;
use futures_util::TryStreamExt;
use mongodb::{
    bson::{doc, spec::BinarySubtype, Binary, Document},
    options::IndexOptions,
    Client, Collection, IndexModel,
};

async fn discovery_index(rows: &Collection<Document>) -> IndexModel {
    let indexes: Vec<IndexModel> = rows
        .list_indexes()
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    indexes
        .into_iter()
        .find(|index| {
            index
                .options
                .as_ref()
                .and_then(|options| options.name.as_deref())
                == Some(INDEX)
        })
        .expect("discovery index must exist")
}

async fn assert_discovery_index(rows: &Collection<Document>) {
    let index = discovery_index(rows).await;
    let expected = doc! {"replica.kind":1,"replica.deployment":1,"replica.active":1,"_id":1};
    assert!(index.keys.iter().eq(expected.iter()), "{:?}", index.keys);
    let options = index.options.unwrap();
    assert_eq!(
        options.partial_filter_expression,
        Some(doc! {"replica":{"$exists":true}})
    );
    assert_eq!(options.expire_after, None);
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn replica_migration_recovers_after_second_index_conflict() {
    let uri = std::env::var("CMA_IM_TEST_MONGODB_URI").unwrap();
    let admin = Client::with_uri_str(uri).await.unwrap();
    let db = admin.database(&format!(
        "cma_eg_replica_schema_{}",
        uuid::Uuid::new_v4().simple()
    ));
    super::super::migrate(&db).await.unwrap();

    let connections = db.collection::<Document>(CONNECTIONS);
    let interactions = db.collection::<Document>(INTERACTIONS);
    let migrations = db.collection::<Document>(MIGRATIONS);
    let retained_id = doc! {"_id":"retained-connection"};
    connections
        .insert_one(doc! {
            "_id":"retained-connection",
            "owner":"fixture-owner",
            "blob":Binary {subtype: BinarySubtype::Generic, bytes: b"retained".to_vec()},
            "version":1_i64,
            "schema_version":1,
        })
        .await
        .unwrap();
    let retained = connections
        .find_one(retained_id.clone())
        .await
        .unwrap()
        .unwrap();

    let conflicting_keys = doc! {"version":1};
    interactions
        .create_index(
            IndexModel::builder()
                .keys(conflicting_keys.clone())
                .options(IndexOptions::builder().name(INDEX.to_owned()).build())
                .build(),
        )
        .await
        .unwrap();

    assert_eq!(
        migrate(&db).await,
        Err(RecordError::Unavailable(
            "gateway database operation failed".into()
        ))
    );
    assert_discovery_index(&connections).await;
    assert_eq!(discovery_index(&interactions).await.keys, conflicting_keys);
    let marker = doc! {"_id":"replicas"};
    assert_eq!(migrations.find_one(marker.clone()).await.unwrap(), None);
    assert_eq!(
        verify(&db).await,
        Err(RecordError::Rejected(
            "replica journal migration is required".into()
        ))
    );

    interactions.drop_index(INDEX).await.unwrap();
    for _ in 0..2 {
        migrate(&db).await.unwrap();
        assert_discovery_index(&connections).await;
        assert_discovery_index(&interactions).await;
        assert_eq!(migrations.count_documents(marker.clone()).await.unwrap(), 1);
        assert_eq!(
            migrations.find_one(marker.clone()).await.unwrap(),
            Some(doc! {"_id":"replicas","version":1})
        );
        verify(&db).await.unwrap();
        assert_eq!(
            connections.find_one(retained_id.clone()).await.unwrap(),
            Some(retained.clone())
        );
    }
    db.drop().await.unwrap();
}
