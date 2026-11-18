use super::mongo_tests::{binding, fixture, NOW};
use super::*;
use layer2_core::sandbox::binding::{BindingState, BindingWork};

async fn restore_index(database: &Database, collection: &str, name: &str) {
    let index = schema::indexes(collection)
        .into_iter()
        .find(|index| index.get_str("name").ok() == Some(name))
        .unwrap();
    database
        .run_command(doc! {"createIndexes":collection,"indexes":[index]})
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn startup_schema_guard_refuses_unverified_key_authority_without_losing_work() {
    let (client, name, store) = fixture().await;
    let db = client.database(&name);
    let retained = binding();
    store.save_binding(None, retained.clone()).await.unwrap();
    db.collection::<Document>(BINDINGS)
        .drop_index("record.key_sha256_1")
        .await
        .unwrap();
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    let mut contender = retained.clone();
    contender.binding_id = "second-binding".into();
    let write = reopened.save_binding(None, contender.clone()).await;
    let lookup = reopened.find_by_key_hash(&retained.key_sha256).await;
    let classify = reopened
        .classify_worker_key_hash(&retained.key_sha256)
        .await;
    let saved = reopened.binding(&retained.binding_id).await.unwrap();
    let count = db
        .collection::<Document>(BINDINGS)
        .count_documents(doc! {})
        .await
        .unwrap();
    // Restore only the synthetic fixture's original constraint, then reassess.
    if write.is_ok() {
        db.collection::<Document>(BINDINGS)
            .delete_one(doc! {"_id":"second-binding"})
            .await
            .unwrap();
    }
    restore_index(&db, BINDINGS, "record.key_sha256_1").await;
    let recovered = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(
        recovered
            .find_by_key_hash(&retained.key_sha256)
            .await
            .unwrap(),
        Some(retained.clone())
    );
    assert_eq!(
        recovered.save_binding(None, contender).await,
        Err(SandboxStoreError::Conflict)
    );
    db.drop().await.unwrap();
    assert!(
        matches!(write, Err(SandboxStoreError::Unavailable(_))),
        "{write:?}"
    );
    assert!(
        matches!(lookup, Err(SandboxStoreError::Unavailable(_))),
        "{lookup:?}"
    );
    assert!(
        matches!(classify, Err(SandboxStoreError::Unavailable(_))),
        "{classify:?}"
    );
    assert_eq!(count, 1);
    assert_eq!(saved, Some(retained));
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn startup_schema_guard_refuses_unverified_associations_but_keeps_identity_reads() {
    let (client, name, store) = fixture().await;
    let db = client.database(&name);
    let mut parent = binding();
    store.save_binding(None, parent.clone()).await.unwrap();
    parent.state = BindingState::Ready;
    parent.work = BindingWork::Idle;
    parent.advance_revision(NOW.into()).unwrap();
    store.save_binding(Some(1), parent.clone()).await.unwrap();
    let first = BindingConversation::new(&parent, "conversation_first".into(), NOW.into()).unwrap();
    store
        .save_binding_conversation(parent.revision, None, first.clone())
        .await
        .unwrap();
    db.collection::<Document>(CONVERSATIONS)
        .drop_index("binding_generation_agent")
        .await
        .unwrap();
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    let mut duplicate = first.clone();
    duplicate.conversation_id = "conversation_second".into();
    let write = reopened
        .save_binding_conversation(parent.revision, None, duplicate)
        .await;
    let lookup = reopened
        .binding_conversation_for_agent(&parent.binding_id, parent.generation, &first.agent_id)
        .await;
    let saved = reopened
        .binding_conversation(
            &parent.binding_id,
            parent.generation,
            &first.conversation_id,
        )
        .await
        .unwrap();
    let count = db
        .collection::<Document>(CONVERSATIONS)
        .count_documents(doc! {})
        .await
        .unwrap();
    let proof = layer2_core::sandbox::binding::BindingConversationAuthority {
        binding: parent.clone(),
        conversation: first.clone(),
    };
    let before = reopened.bindings.records.find_one(doc! {}).await.unwrap();
    let mut session = client.start_session().await.unwrap();
    let authority = reopened
        .bindings
        .conversation_authority(Some(&proof), &mut session)
        .await;
    let after = reopened.bindings.records.find_one(doc! {}).await.unwrap();
    assert!(reopened
        .bindings
        .conversation_authority(None, &mut session)
        .await
        .is_ok());
    restore_index(&db, CONVERSATIONS, "binding_generation_agent").await;
    let recovered = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(
        recovered
            .binding_conversation_for_agent(&parent.binding_id, parent.generation, &first.agent_id)
            .await
            .unwrap(),
        Some(first.clone())
    );
    db.drop().await.unwrap();
    assert_eq!(before, after, "unavailable command authority wrote a fence");
    let error = match authority {
        Err(error) => crate::store::agent_commands::ApplyError::from(error),
        Ok(()) => panic!("unverified association authorized a command"),
    };
    assert!(matches!(
        error,
        crate::store::agent_commands::ApplyError::Domain(
            layer2_core::sandbox::agent_io::CommandLedgerError::Unavailable
        )
    ));
    assert!(
        matches!(write, Err(SandboxStoreError::Unavailable(_))),
        "{write:?}"
    );
    assert!(
        matches!(lookup, Err(SandboxStoreError::Unavailable(_))),
        "{lookup:?}"
    );
    assert_eq!(count, 1);
    assert_eq!(saved, Some(first));
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn startup_schema_guard_query_drift_does_not_disable_independent_writes() {
    let (client, name, _) = fixture().await;
    let db = client.database(&name);
    db.collection::<Document>(BINDINGS)
        .drop_index("pending_1__id_1")
        .await
        .unwrap();
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    let value = binding();
    reopened.save_binding(None, value.clone()).await.unwrap();
    assert_eq!(
        reopened.find_by_key_hash(&value.key_sha256).await.unwrap(),
        Some(value)
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn startup_schema_guard_refuses_credential_admission_and_rotation_bypasses() {
    use crate::sandbox::SandboxCredentialCustody;
    use crate::store::{AuthEncryptionKey, MongoSandboxCredentialStore};
    use base64::Engine as _;
    use layer2_core::sandbox::binding::{BindingCredentialCustody, ProvidedCredential};
    use std::sync::Arc;

    let (client, name, store) = fixture().await;
    let db = client.database(&name);
    let key = ProvidedCredential::new(format!("nyx_{}", uuid::Uuid::new_v4().simple())).unwrap();
    let mut value = binding();
    value.key_sha256 = key.sha256();
    store.save_binding(None, value.clone()).await.unwrap();
    let encryption = AuthEncryptionKey::from_base64(
        &base64::engine::general_purpose::STANDARD.encode([17_u8; 32]),
    )
    .unwrap();
    let custody = Arc::new(
        MongoSandboxCredentialStore::open(&client, &name, "schema-guard-fixture", &encryption)
            .await
            .unwrap(),
    );
    db.collection::<Document>(BINDINGS)
        .drop_index("record.key_sha256_1")
        .await
        .unwrap();
    let reopened = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    let controller = MongoBindingCredentials::new(reopened.clone(), custody.clone());
    let admission = controller.retain_binding(&value, &key).await;
    let rotation = controller.rotate_binding(&value, &key).await;
    let saved = reopened.binding(&value.binding_id).await.unwrap();
    let retained = custody.get(&value.sandbox_id, 1).await.unwrap();
    db.drop().await.unwrap();
    assert!(matches!(
        admission,
        Err(layer2_core::CoreError::RuntimeUnavailable(_))
    ));
    assert!(matches!(
        rotation,
        Err(layer2_core::CoreError::RuntimeUnavailable(_))
    ));
    assert!(retained.is_none());
    assert_eq!(saved, Some(value));
}
