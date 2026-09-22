use super::mongo_tests::{binding, fixture, NOW};
use super::*;
use crate::sandbox::SandboxCredentialCustody;
use crate::store::{AuthEncryptionKey, MongoSandboxCredentialStore};
use base64::Engine;
use layer2_core::sandbox::binding::{
    BindingCredentialCustody, BindingState, BindingWork, ProvidedCredential,
};
use ring::rand::{SecureRandom, SystemRandom};
use std::sync::Arc;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn binding_custody_cas_retirement_and_late_request_cannot_resurrect_key() {
    let (client, database, store) = fixture().await;
    let mut bytes = [0; 32];
    SystemRandom::new().fill(&mut bytes).unwrap();
    let encryption =
        AuthEncryptionKey::from_base64(&base64::engine::general_purpose::STANDARD.encode(bytes))
            .unwrap();
    let custody = Arc::new(
        MongoSandboxCredentialStore::open(&client, &database, "binding-custody-test", &encryption)
            .await
            .unwrap(),
    );
    let store = Arc::new(store);
    let controller = MongoBindingCredentials::new(store.clone(), custody.clone());
    let key = ProvidedCredential::new(format!("nyx_{}", uuid::Uuid::new_v4().simple())).unwrap();
    let mut value = binding();
    value.key_sha256 = key.sha256();
    store.save_binding(None, value.clone()).await.unwrap();
    controller.retain_binding(&value, &key).await.unwrap();
    let original = custody.get(&value.sandbox_id, 1).await.unwrap().unwrap();
    controller.retain_binding(&value, &key).await.unwrap();
    assert_eq!(
        custody
            .get(&value.sandbox_id, 1)
            .await
            .unwrap()
            .unwrap()
            .authority,
        original.authority
    );
    let wrong = ProvidedCredential::new(format!("nyx_{}", uuid::Uuid::new_v4().simple())).unwrap();
    assert!(controller.retain_binding(&value, &wrong).await.is_err());
    assert!(controller
        .forget_binding(&value, &value.sandbox_id)
        .await
        .is_err());
    assert_eq!(
        custody
            .get(&value.sandbox_id, 1)
            .await
            .unwrap()
            .unwrap()
            .value
            .expose(),
        key.expose()
    );
    let serialized = client
        .database(&database)
        .collection::<mongodb::bson::Document>("sandbox_credentials")
        .find_one(doc! {})
        .await
        .unwrap()
        .unwrap();
    assert!(!format!("{serialized:?}").contains(key.expose()));
    let mut retired = value.clone();
    retired.state = BindingState::Retired;
    retired.work = BindingWork::RetireCurrentSandbox;
    retired.advance_revision(NOW.into()).unwrap();
    store
        .save_binding(Some(value.revision), retired.clone())
        .await
        .unwrap();
    controller
        .forget_binding(&retired, &retired.sandbox_id)
        .await
        .unwrap();
    assert!(controller.retain_binding(&value, &key).await.is_err());
    assert!(controller.retain_binding(&retired, &key).await.is_err());
    assert!(custody.get(&value.sandbox_id, 1).await.unwrap().is_none());
    // No Sandbox aggregate ever existed: cleanup still removed its reservation's key.
    controller
        .forget_binding(&retired, &retired.sandbox_id)
        .await
        .unwrap();
    client.database(&database).drop().await.unwrap();
}
