use super::*;

async fn fixture() -> (Client, String, MongoUserDirectoryStore) {
    let client = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let name = format!("directory_order_{}", uuid::Uuid::new_v4().simple());
    let store = MongoUserDirectoryStore::open(&client, &name).await.unwrap();
    (client, name, store)
}

fn identity(name: &str) -> PublicIdentity {
    PublicIdentity {
        subject: SubjectId("subject".into()),
        display_name: Some(name.into()),
        avatar_url: Some(format!("https://example.test/{name}")),
    }
}

async fn observe(store: &MongoUserDirectoryStore, name: &str, millis: i64) {
    store
        .observe_at(
            &identity(name),
            &format!("observation-{millis}"),
            DateTime::from_millis(millis),
        )
        .await
        .unwrap();
}

async fn stored(store: &MongoUserDirectoryStore) -> Document {
    store
        .users
        .clone_with_type::<Document>()
        .find_one(doc! {"_id":"subject"})
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn late_and_equal_observations_preserve_newer_identity_and_paging_time() {
    let (client, name, store) = fixture().await;
    observe(&store, "first", 10).await;
    observe(&store, "$newer", 30).await;
    let newest = stored(&store).await;
    assert_eq!(newest.get_str("first_seen_at").unwrap(), "observation-10");
    assert_eq!(newest.get_str("display_name").unwrap(), "$newer");
    for at in [20, 30] {
        observe(&store, "late", at).await;
        assert_eq!(stored(&store).await, newest);
    }
    let user = store
        .user(&SubjectId("subject".into()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(user.identity, identity("$newer"));
    assert_eq!(user.last_seen_at, "observation-30");
    let cleared = PublicIdentity {
        display_name: None,
        avatar_url: None,
        ..identity("unused")
    };
    store
        .observe_at(&cleared, "observation-40", DateTime::from_millis(40))
        .await
        .unwrap();
    let user = store.user(&cleared.subject).await.unwrap().unwrap();
    assert_eq!(user.identity, cleared);
    assert_eq!(user.first_seen_at, "observation-10");
    assert_eq!(user.last_seen_at, "observation-40");
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn concurrent_first_observations_keep_the_latest_and_refuse_future_schema() {
    let (client, name, store) = fixture().await;
    futures_util::future::join_all((1..=12).rev().map(|n| {
        let store = &store;
        async move { observe(store, &format!("name-{n}"), n).await }
    }))
    .await;
    let latest = stored(&store).await;
    assert_eq!(latest.get_str("display_name").unwrap(), "name-12");
    assert_eq!(
        latest.get_datetime("observed_at").unwrap(),
        &DateTime::from_millis(12)
    );
    assert_eq!(store.users.count_documents(doc! {}).await.unwrap(), 1);
    let collection = store.users.clone_with_type::<Document>();
    collection
        .update_one(
            doc! {"_id":"subject"},
            doc! {"$set":{"document_schema_version":2_i64}},
        )
        .await
        .unwrap();
    let future = stored(&store).await;
    assert!(store
        .observe_at(
            &identity("future"),
            "observation-50",
            DateTime::from_millis(50)
        )
        .await
        .is_err());
    assert_eq!(stored(&store).await, future);
    client.database(&name).drop().await.unwrap();
}
