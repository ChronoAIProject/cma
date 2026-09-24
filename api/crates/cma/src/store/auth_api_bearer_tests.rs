use std::time::{Duration, SystemTime};

use base64::Engine as _;
use futures_util::TryStreamExt;
use mongodb::bson::Document;

use super::*;

fn custody() -> ApiBearerCustody {
    ApiBearerCustody::new(
        "https://identity.example".into(),
        SubjectId::new("api-owner"),
        "a".repeat(64),
        SecretValue::new("fixture-independent-bearer".into()),
        SystemTime::UNIX_EPOCH + Duration::from_secs(2_000_000_000),
    )
    .unwrap()
}

#[tokio::test]
async fn nonexpiring_custody_round_trips_and_cannot_replace_verified_expiry() {
    let store = tests::store().await;
    let mut original = custody();
    original.expires_at = None;
    let document = store.api_bearer_to_doc(original.clone()).unwrap();
    let bson = mongodb::bson::serialize_to_document(&document).unwrap();
    assert!(bson.get("expires_at").unwrap().as_null().is_some());
    assert_eq!(store.api_bearer_from_doc(document).unwrap(), original);
    let mut finite = store.api_bearer_to_doc(custody()).unwrap();
    finite.expires_at = None;
    assert!(store.api_bearer_from_doc(finite).is_err());
}

#[tokio::test]
async fn api_bearer_ciphertext_round_trips_without_refresh_material_or_plaintext() {
    let store = tests::store().await;
    let original = custody();
    let document = store.api_bearer_to_doc(original.clone()).unwrap();
    let bson = mongodb::bson::serialize_to_document(&document).unwrap();
    assert!(!format!("{document:?} {bson:?}").contains(original.access_token.expose()));
    assert!(!bson.contains_key("refresh_token"));
    assert!(!bson.contains_key("authorization"));
    assert_eq!(document.document_schema_version, DOCUMENT_SCHEMA_VERSION);
    assert_eq!(
        document.expires_at.map(|value| value.to_system_time()),
        original.expires_at
    );
    assert_eq!(store.api_bearer_from_doc(document).unwrap(), original);
}

#[tokio::test]
async fn api_bearer_ciphertext_authenticates_every_custody_binding() {
    let store = tests::store().await;
    for changed in 0..7 {
        let mut document = store.api_bearer_to_doc(custody()).unwrap();
        match changed {
            0 => document.id = binary([17; 32]),
            1 => document.issuer = "https://another-identity.example".into(),
            2 => document.subject = "another-owner".into(),
            3 => document.credential_id = "b".repeat(64),
            4 => {
                document.expires_at = Some(DateTime::from_millis(
                    document.expires_at.unwrap().timestamp_millis() + 1000,
                ))
            }
            5 => document.document_schema_version += 1,
            _ => {
                let mut other = custody();
                other.credential_id = "c".repeat(64);
                other.id =
                    ApiBearerCustody::identity(&other.issuer, &other.subject, &other.credential_id)
                        .unwrap();
                document.access_token = store.api_bearer_to_doc(other).unwrap().access_token;
            }
        }
        assert!(
            matches!(
                store.api_bearer_from_doc(document),
                Err(StoreError::Malformed(_))
            ),
            "binding {changed}"
        );
    }
}

#[tokio::test]
async fn api_bearer_ciphertext_cannot_move_to_another_deployment_or_key() {
    let store = tests::store().await;
    let mut other = tests::store().await;
    let key =
        AuthEncryptionKey::from_base64("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=").unwrap();
    other.cipher = AuthCipher::new("other-deployment", &key).unwrap();
    assert!(matches!(
        other.api_bearer_from_doc(store.api_bearer_to_doc(custody()).unwrap()),
        Err(StoreError::Malformed(_))
    ));
    let key = AuthEncryptionKey::from_base64(
        &base64::engine::general_purpose::STANDARD.encode([1_u8; 32]),
    )
    .unwrap();
    other.cipher = AuthCipher::new("test", &key).unwrap();
    assert!(matches!(
        other.api_bearer_from_doc(store.api_bearer_to_doc(custody()).unwrap()),
        Err(StoreError::Malformed(_))
    ));
}

#[tokio::test]
async fn api_bearer_storage_refuses_changed_identity_and_subsecond_expiry() {
    let store = tests::store().await;
    let mut changed = custody();
    changed.subject = SubjectId::new("substituted-owner");
    assert!(store.api_bearer_to_doc(changed).is_err());
    let mut imprecise = custody();
    imprecise.expires_at = imprecise
        .expires_at
        .map(|value| value + Duration::from_nanos(1));
    assert!(store.api_bearer_to_doc(imprecise).is_err());
    let mut expired = custody();
    expired.expires_at = Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1));
    assert_eq!(
        store
            .api_bearer_from_doc(store.api_bearer_to_doc(expired.clone()).unwrap())
            .unwrap(),
        expired
    );
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_api_bearer_custody_is_immutable_independent_and_shared_across_replicas() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").expect("disposable replica set required");
    let client = Client::with_uri_str(&uri).await.unwrap();
    let other_client = Client::with_uri_str(&uri).await.unwrap();
    let name = format!("cma_api_bearer_{}", uuid::Uuid::new_v4().simple());
    let key =
        AuthEncryptionKey::from_base64("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=").unwrap();
    let first = MongoAuthStore::open(&client, &name, "api-bearer-test", &key)
        .await
        .unwrap();
    let second = MongoAuthStore::open(&other_client, &name, "api-bearer-test", &key)
        .await
        .unwrap();
    let original = custody();
    let browser = SubjectAuthState::new(
        original.subject.clone(),
        Authorization::none(),
        Profile::default(),
        Some(DurableSubjectTokens {
            access_token: SecretValue::new("fixture-browser-access".into()),
            refresh_token: Some(SecretValue::new("fixture-browser-refresh".into())),
            expires_at: original.expires_at.unwrap(),
        }),
    );
    assert!(first.insert_subject(browser.clone()).await.unwrap());
    let (one, two) = tokio::join!(
        first.insert_api_bearer(original.clone()),
        second.insert_api_bearer(original.clone())
    );
    assert_ne!(one.unwrap(), two.unwrap());
    assert_eq!(
        first.api_bearer(&original.id).await.unwrap(),
        Some(original.clone())
    );
    let collection = client.database(&name).collection::<Document>(API_BEARERS);
    let encrypted = collection.find_one(doc! {}).await.unwrap().unwrap();
    assert!(!format!("{encrypted:?}").contains(original.access_token.expose()));
    for changed in 0..2 {
        let mut replacement = original.clone();
        if changed == 0 {
            replacement.expires_at = replacement
                .expires_at
                .map(|value| value + Duration::from_secs(3600));
        } else {
            replacement.access_token = SecretValue::new("fixture-replacement-bearer".into());
        }
        assert!(!second.insert_api_bearer(replacement).await.unwrap());
        assert_eq!(
            collection.find_one(doc! {}).await.unwrap().unwrap(),
            encrypted
        );
    }
    let retained_browser = second.subject(&original.subject).await.unwrap().unwrap();
    assert_eq!(retained_browser.incarnation, browser.incarnation);
    assert_eq!(retained_browser.revision, browser.revision);
    assert_eq!(retained_browser.tokens, browser.tokens);
    let indexes: Vec<_> = collection
        .list_indexes()
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    let expiry = indexes
        .iter()
        .find(|index| {
            index
                .options
                .as_ref()
                .and_then(|options| options.name.as_deref())
                == Some(EXPIRY_INDEX)
        })
        .unwrap();
    assert_eq!(expiry.keys, doc! { "expires_at": 1 });
    assert_eq!(
        expiry.options.as_ref().unwrap().expire_after,
        Some(Duration::ZERO)
    );
    drop(first);
    let reopened = MongoAuthStore::open(&client, &name, "api-bearer-test", &key)
        .await
        .unwrap();
    assert_eq!(
        reopened.api_bearer(&original.id).await.unwrap(),
        Some(original.clone())
    );
    assert!(reopened
        .api_bearer(&SecretDigest::from_bytes([19; 32]))
        .await
        .unwrap()
        .is_none());
    collection.update_one(doc! { "_id": binary(*original.id.as_bytes()) }, doc! { "$set": { "expires_at": DateTime::from_system_time(original.expires_at.unwrap() + Duration::from_secs(3600)) } }).await.unwrap();
    assert!(matches!(
        reopened.api_bearer(&original.id).await,
        Err(StoreError::Malformed(_))
    ));
    collection
        .update_one(
            doc! { "_id": binary(*original.id.as_bytes()) },
            doc! { "$set": { "document_schema_version": 2 } },
        )
        .await
        .unwrap();
    assert!(matches!(
        MongoAuthStore::open(&client, &name, "api-bearer-test", &key).await,
        Err(StoreError::Malformed(_))
    ));
    client.database(&name).drop().await.unwrap();
}
