//! Work-carrier storage only: these fixtures grant no recovery authority and
//! create no Agent, segment or Publication. End-to-end admission remains separate.
use super::*;
use crate::store::agent_capacity_recovery::records;
use crate::store::agent_log_integrity::TransactionLogError;
use crate::store::agent_log_segments::transaction_scope::TransactionScope;
use layer2_core::sandbox::agent_capacity_recovery::state::{
    RecoveryPhase, RecoveryRequest, RecoveryTarget, RecoveryWork,
};
use layer2_core::sandbox::RuntimeBinding;

fn work() -> RecoveryWork {
    let request = RecoveryRequest {
        agent_id: AgentId::new("carrier-fixture"),
        creation_key: "original-creation".into(),
        creation_fingerprint: "a".repeat(64),
        failure_seq: 10_001,
        requested_at: NOW.into(),
    };
    RecoveryWork {
        format_version: 1,
        target: RecoveryTarget {
            agent_id: request.agent_id.clone(),
            creation_key: request.creation_key.clone(),
            creation_fingerprint: request.creation_fingerprint.clone(),
            conversation_id: "retained-native".into(),
            conversation_generation: 1,
            runtime: RuntimeBinding {
                boot_id: Some("b".repeat(32)),
                runtime_id: "original-runtime".into(),
                runtime_version: "test".into(),
                provision_generation: Some(3),
                bound_at: NOW.into(),
            },
            home_identity: "original-home".into(),
            original_open: AgentWorkReceipt {
                step: AgentWorkStep::OpenConversation,
                operation_id: format!("agent:{}:open:v1", request.agent_id),
                request_sha256: "c".repeat(64),
                completed_at: NOW.into(),
            },
            active_turn: None,
            input_sha256: "d".repeat(64),
        },
        request,
        phase: RecoveryPhase::Admitted,
        revision: 1,
        attempts: 0,
        next_fence: 8,
        lease: None,
        ready_at: NOW.into(),
        completed_at: None,
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn capacity_carrier_insert_and_replace_obey_commit_abort_and_exact_cas() {
    let (client, name) = database("capacity_carrier_cas").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    client
        .database(&name)
        .create_collection(records::COLLECTION)
        .await
        .unwrap();
    let original = work();
    let key = original.request.key().unwrap();
    let mut session = client.start_session().await.unwrap();
    let mut scope = TransactionScope::start(&mut session).await.unwrap();
    records::insert(&store, &original, &mut scope)
        .await
        .unwrap();
    scope.release().abort_transaction().await.unwrap();
    assert_eq!(
        records::collection(&store)
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );

    let mut scope = TransactionScope::start(&mut session).await.unwrap();
    records::insert(&store, &original, &mut scope)
        .await
        .unwrap();
    scope.release().commit_transaction().await.unwrap();
    let mut next = original.clone();
    next.revision += 1;
    next.phase = RecoveryPhase::Superseded;
    let mut scope = TransactionScope::start(&mut session).await.unwrap();
    records::replace(&store, &original, &next, &mut scope)
        .await
        .unwrap();
    assert_eq!(
        records::read(&store, &key, &mut scope).await.unwrap(),
        Some(next.clone())
    );
    scope.release().abort_transaction().await.unwrap();

    let mut scope = TransactionScope::start(&mut session).await.unwrap();
    assert_eq!(
        records::read(&store, &key, &mut scope).await.unwrap(),
        Some(original.clone())
    );
    records::replace(&store, &original, &next, &mut scope)
        .await
        .unwrap();
    scope.release().commit_transaction().await.unwrap();
    let mut scope = TransactionScope::start(&mut session).await.unwrap();
    assert!(matches!(
        records::replace(&store, &original, &next, &mut scope).await,
        Err(TransactionLogError::Domain(SandboxStoreError::Conflict))
    ));
    scope.release().abort_transaction().await.unwrap();
    let mut scope = TransactionScope::start(&mut session).await.unwrap();
    assert_eq!(
        records::read(&store, &key, &mut scope).await.unwrap(),
        Some(next)
    );
    scope.release().abort_transaction().await.unwrap();
    assert_eq!(store.agents.count_documents(doc! {}).await.unwrap(), 0);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn capacity_carrier_cannot_change_original_request_or_native_target() {
    let (client, name) = database("capacity_carrier_identity").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    client
        .database(&name)
        .create_collection(records::COLLECTION)
        .await
        .unwrap();
    let original = work();
    let key = original.request.key().unwrap();
    let mut session = client.start_session().await.unwrap();
    let mut scope = TransactionScope::start(&mut session).await.unwrap();
    records::insert(&store, &original, &mut scope)
        .await
        .unwrap();
    scope.release().commit_transaction().await.unwrap();
    for variant in 0..4 {
        let mut changed = original.clone();
        changed.revision += 1;
        match variant {
            0 => changed.target.conversation_id = "replacement".into(),
            1 => changed.target.input_sha256 = "e".repeat(64),
            2 => changed.request.requested_at = MINUTE.into(),
            _ => changed.revision += 1,
        }
        let mut scope = TransactionScope::start(&mut session).await.unwrap();
        assert!(matches!(
            records::replace(&store, &original, &changed, &mut scope).await,
            Err(TransactionLogError::Domain(SandboxStoreError::Conflict))
        ));
        scope.release().abort_transaction().await.unwrap();
    }
    let mut scope = TransactionScope::start(&mut session).await.unwrap();
    assert_eq!(
        records::read(&store, &key, &mut scope).await.unwrap(),
        Some(original)
    );
    scope.release().abort_transaction().await.unwrap();
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn capacity_carrier_corrupt_projection_and_unknown_fields_fail_closed() {
    let (client, name) = database("capacity_carrier_corrupt").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    client
        .database(&name)
        .create_collection(records::COLLECTION)
        .await
        .unwrap();
    let original = work();
    let key = original.request.key().unwrap();
    let mut session = client.start_session().await.unwrap();
    let mut scope = TransactionScope::start(&mut session).await.unwrap();
    records::insert(&store, &original, &mut scope)
        .await
        .unwrap();
    scope.release().commit_transaction().await.unwrap();
    let collection = records::collection(&store);
    let valid = collection
        .find_one(doc! {"_id": &key})
        .await
        .unwrap()
        .unwrap();
    for variant in 0..4 {
        let mut corrupt = valid.clone();
        match variant {
            0 => {
                corrupt.insert("agent_id", "different-agent");
            }
            1 => {
                corrupt.insert("format_version", 2_i64);
            }
            2 => {
                corrupt.insert("unknown_authority", true);
            }
            _ => {
                corrupt.insert("unexpected_padding", "x".repeat(32 * 1024));
            }
        }
        collection
            .replace_one(doc! {"_id": &key}, corrupt)
            .await
            .unwrap();
        let mut scope = TransactionScope::start(&mut session).await.unwrap();
        assert!(matches!(
            records::read(&store, &key, &mut scope).await,
            Err(TransactionLogError::Domain(SandboxStoreError::Corrupt(_)))
        ));
        scope.release().abort_transaction().await.unwrap();
    }
    client.database(&name).drop().await.unwrap();
}
