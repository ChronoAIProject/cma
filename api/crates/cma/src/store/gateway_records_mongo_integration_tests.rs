use layer2_core::{GatewayRecordStore, StoreError, SubjectId};
use mongodb::{bson::doc, Client};

use super::gateway_records::{MongoGatewayRecordStore, GATEWAY_CHANNEL_RECORDS};

async fn fixture() -> (Client, String, MongoGatewayRecordStore) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("ignored test requires CHRONO_TEST_MONGODB_URI");
    let client = Client::with_uri_str(uri)
        .await
        .expect("test Mongo connects");
    let database = format!("cma_gateway_records_test_{}", uuid::Uuid::new_v4().simple());
    let store = MongoGatewayRecordStore::open(&client, &database)
        .await
        .expect("gateway record store opens");
    (client, database, store)
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable MongoDB replica set"]
async fn mongo_round_trip_cas_delete_and_stable_listing() {
    let (client, database, store) = fixture().await;
    let owner = SubjectId::new("gateway-owner");
    let first = "123e4567-e89b-42d3-a456-426614174001";
    let second = "123e4567-e89b-42d3-a456-426614174002";

    let created = store.put(first, &owner, vec![1, 2, 3], None).await.unwrap();
    assert_eq!(created.version, 1);
    let raw = client
        .database(&database)
        .collection::<mongodb::bson::Document>(GATEWAY_CHANNEL_RECORDS)
        .find_one(doc! { "_id": first })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(raw.get_binary_generic("blob").unwrap(), &[1, 2, 3]);

    let updated = store
        .put(first, &owner, vec![4, 5], Some(created.version))
        .await
        .unwrap();
    assert_eq!(updated.version, 2);
    assert!(matches!(
        store.put(first, &owner, vec![6], Some(1)).await,
        Err(StoreError::RevisionConflict {
            expected: 1,
            actual: 2
        })
    ));

    store.put(second, &owner, vec![7], None).await.unwrap();
    let page = store.list(Some(&owner), None, 1).await.unwrap();
    assert_eq!(page.len(), 1);
    let next = page[0].record_id.clone();
    let page = store.list(Some(&owner), Some(&next), 1).await.unwrap();
    assert_eq!(
        page.iter()
            .map(|head| head.record_id.as_str())
            .collect::<Vec<_>>(),
        [second]
    );

    assert!(matches!(
        store.delete(first, Some(1)).await,
        Err(StoreError::RevisionConflict {
            expected: 1,
            actual: 2
        })
    ));
    assert!(store.delete(first, Some(2)).await.unwrap());
    assert!(store.get(first).await.unwrap().is_none());

    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable MongoDB replica set"]
async fn mongo_channel_and_connection_namespaces_keep_same_uuid_and_cas_independent() {
    let (client, database, channels) = fixture().await;
    let connections = MongoGatewayRecordStore::open_connections(&client, &database)
        .await
        .unwrap();
    let id = "123e4567-e89b-42d3-a456-426614174001";
    let channel_owner = SubjectId::new("channel-owner");
    let connection_owner = SubjectId::new("connection-owner");
    let channel = channels
        .put(id, &channel_owner, vec![1], None)
        .await
        .unwrap();
    let connection = connections
        .put(id, &connection_owner, vec![2], None)
        .await
        .unwrap();
    assert_eq!(channel.version, 1);
    assert_eq!(connection.version, 1);

    // These are the same UUID and CAS version in different durable namespaces.
    let (channel, connection) = tokio::join!(
        channels.put(id, &channel_owner, vec![3], Some(1)),
        connections.put(id, &connection_owner, vec![4], Some(1)),
    );
    let channel = channel.unwrap();
    let connection = connection.unwrap();
    assert_eq!(channel.version, 2);
    assert_eq!(connection.version, 2);
    assert!(matches!(
        connections
            .put(id, &connection_owner, vec![5], Some(1))
            .await,
        Err(StoreError::RevisionConflict {
            expected: 1,
            actual: 2
        })
    ));
    assert_eq!(channels.get(id).await.unwrap(), Some(channel.clone()));
    assert_eq!(connections.get(id).await.unwrap(), Some(connection.clone()));

    for (collection, owner, bytes) in [
        (GATEWAY_CHANNEL_RECORDS, &channel_owner, &[3_u8][..]),
        ("gateway_connection_records", &connection_owner, &[4_u8][..]),
    ] {
        let raw = client
            .database(&database)
            .collection::<mongodb::bson::Document>(collection)
            .find_one(doc! { "_id": id })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(raw.get_str("owner").unwrap(), owner.as_str());
        assert_eq!(raw.get_binary_generic("blob").unwrap(), bytes);
        assert_eq!(raw.get_i64("version").unwrap(), 2);
    }

    // Replica restart must reopen the same distinct collections and inventories.
    let channels = MongoGatewayRecordStore::open(&client, &database)
        .await
        .unwrap();
    let connections = MongoGatewayRecordStore::open_connections(&client, &database)
        .await
        .unwrap();
    assert_eq!(channels.get(id).await.unwrap(), Some(channel));
    assert_eq!(connections.get(id).await.unwrap(), Some(connection.clone()));
    assert!(channels
        .list(Some(&connection_owner), None, 100)
        .await
        .unwrap()
        .is_empty());
    assert!(connections
        .list(Some(&channel_owner), None, 100)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        channels
            .list(Some(&channel_owner), None, 100)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        connections
            .list(Some(&connection_owner), None, 100)
            .await
            .unwrap()
            .len(),
        1
    );

    assert!(channels.delete(id, Some(2)).await.unwrap());
    assert!(channels.get(id).await.unwrap().is_none());
    assert_eq!(connections.get(id).await.unwrap(), Some(connection));
    assert!(matches!(
        connections.delete(id, Some(1)).await,
        Err(StoreError::RevisionConflict {
            expected: 1,
            actual: 2
        })
    ));
    assert!(connections.delete(id, Some(2)).await.unwrap());
    assert!(connections.get(id).await.unwrap().is_none());
    client.database(&database).drop().await.unwrap();
}
