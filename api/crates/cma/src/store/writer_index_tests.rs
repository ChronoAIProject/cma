//! Real additive-index outcomes on an owned Mongo fixture, including refusal.
use super::*;
use crate::store::writer_fixture::*;
use base64::{engine::general_purpose::STANDARD, Engine};
use futures_util::TryStreamExt;
use mongodb::bson::Document;
use serde_json::json;

fn key() -> AuthEncryptionKey {
    let mut bytes = Vec::from(*uuid::Uuid::new_v4().as_bytes());
    bytes.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
    AuthEncryptionKey::from_base64(&STANDARD.encode(bytes)).unwrap()
}

async fn indexes(collection: &Collection<Document>) -> Vec<Document> {
    let models: Vec<IndexModel> = collection
        .list_indexes()
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    let mut docs: Vec<Document> = models
        .iter()
        .map(|m| mongodb::bson::serialize_to_document(m).unwrap())
        .collect();
    docs.sort_by_key(|d| d.get_str("name").unwrap().to_owned());
    docs
}

#[tokio::test]
#[ignore = "requires isolated cross-version writer fixture"]
async fn index_round() {
    let client = client().await;
    let database = database_name();
    let action = input("CMA_WRITER_ACTION");
    let key = key();
    let mut store = MongoAuthStore::open(&client, &database, "writer-fixture", &key)
        .await
        .unwrap();
    let collection = client
        .database(&database)
        .collection::<Document>(API_BEARERS);
    let name = "auth_api_bearers_subject";
    let expected = indexes(&collection).await;
    let index = expected
        .iter()
        .find(|d| d.get_str("name").ok() == Some(name))
        .unwrap();
    assert_eq!(
        index.get_document("key").unwrap(),
        &doc! {"subject":1,"_id":1}
    );
    assert!(!index.contains_key("unique") && !index.contains_key("expireAfterSeconds"));
    let started = std::time::Instant::now();
    let mut outcome = "accepted";
    let mut rows = 0;
    match action.as_str() {
        "reopen" => {
            for _ in 0..2 {
                MongoAuthStore::open(&client, &database, "writer-fixture", &key)
                    .await
                    .unwrap();
            }
            let (a, b) = tokio::join!(
                store.ensure_bearer_subject_index(),
                store.ensure_bearer_subject_index()
            );
            a.unwrap();
            b.unwrap();
            assert_eq!(expected, indexes(&collection).await);
        }
        "conflicting_keys" | "conflicting_options" => {
            collection.drop_index(name).await.unwrap();
            let keys = if action == "conflicting_keys" {
                doc! {"subject":-1,"_id":1}
            } else {
                doc! {"subject":1,"_id":1}
            };
            let mut options = IndexOptions::builder().name(name.to_owned()).build();
            if action == "conflicting_options" {
                options.unique = Some(true);
            }
            collection
                .create_index(IndexModel::builder().keys(keys).options(options).build())
                .await
                .unwrap();
            let before = indexes(&collection).await;
            assert!(matches!(store.ensure_bearer_subject_index().await,
                Err(StoreError::Unavailable(message)) if message=="API bearer subject index"));
            assert!(
                matches!(MongoAuthStore::open(&client,&database,"writer-fixture",&key).await,
                Err(StoreError::Unavailable(message)) if message=="API bearer subject index")
            );
            assert_eq!(before, indexes(&collection).await);
            outcome = "refused_without_repair";
        }
        "permission_denied" => {
            collection.drop_index(name).await.unwrap();
            let user = format!("reader_{}", uuid::Uuid::new_v4().simple());
            let password = uuid::Uuid::new_v4().to_string();
            let created = client
                .database(&database)
                .run_command(doc! {
                    "createUser":&user,"pwd":&password,"roles":[{"role":"read","db":&database}]
                })
                .await;
            assert!(
                created.is_ok(),
                "local read-only fixture principal must be created"
            );
            let mut options =
                mongodb::options::ClientOptions::parse(input("CHRONO_TEST_MONGODB_URI"))
                    .await
                    .unwrap();
            options.credential = Some(
                mongodb::options::Credential::builder()
                    .username(user)
                    .password(password)
                    .source(database.clone())
                    .build(),
            );
            let restricted = Client::with_options(options).unwrap();
            store.api_bearers = restricted.database(&database).collection(API_BEARERS);
            let before = indexes(&collection).await;
            assert!(matches!(store.ensure_bearer_subject_index().await,
                Err(StoreError::Unavailable(message)) if message=="API bearer subject index"));
            assert_eq!(before, indexes(&collection).await);
            outcome = "permission_refused_without_repair";
        }
        "build_cost" => {
            collection.drop_index(name).await.unwrap();
            rows = 20_000;
            for batch in 0..20 {
                let docs: Vec<_> = (0..1000)
                    .map(|i| {
                        doc! {"_id":batch*1000+i,
                        "document_schema_version":1_i64,"subject":format!("subject-{}",i%100),
                        "fixture_padding":"x".repeat(256)}
                    })
                    .collect();
                collection.insert_many(docs).await.unwrap();
            }
            let start = std::time::Instant::now();
            store.ensure_bearer_subject_index().await.unwrap();
            let build_ms = start.elapsed().as_millis();
            assert_eq!(collection.count_documents(doc! {}).await.unwrap(), rows);
            assert_eq!(expected, indexes(&collection).await);
            emit(
                json!({"action":action,"rows":rows,"index_build_ms":build_ms,
                "index_hash":hash(&expected),"scope":"synthetic_cost_not_production_cardinality"}),
            );
            return;
        }
        _ => panic!("unsupported additive-index case"),
    }
    assert_eq!(collection.count_documents(doc! {}).await.unwrap(), rows);
    emit(
        json!({"action":action,"outcome":outcome,"elapsed_ms":started.elapsed().as_millis(),
        "index_hash":hash(&indexes(&collection).await),"row_count":rows}),
    );
}
