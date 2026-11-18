//! Each version must reopen additive indexes without replacing their definitions.
use super::*;
use crate::store::writer_fixture::*;
use base64::{engine::general_purpose::STANDARD, Engine};
use futures_util::TryStreamExt;
use mongodb::bson::Document;
use serde_json::json;

#[tokio::test]
#[ignore = "requires isolated cross-version writer fixture"]
async fn auth_open_round() {
    let client = client().await;
    let database = database_name();
    let collection = client
        .database(&database)
        .collection::<Document>(API_BEARERS);
    let before: Vec<IndexModel> = collection
        .list_indexes()
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    let mut bytes = Vec::from(*uuid::Uuid::new_v4().as_bytes());
    bytes.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
    let key = AuthEncryptionKey::from_base64(&STANDARD.encode(bytes)).unwrap();
    MongoAuthStore::open(&client, &database, "writer-fixture", &key)
        .await
        .unwrap();
    let after: Vec<IndexModel> = collection
        .list_indexes()
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    assert_eq!(hash(&before), hash(&after));
    emit(json!({"indexes_preserved":true,"index_hash":hash(&after)}));
}
