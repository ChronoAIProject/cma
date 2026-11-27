use super::super::schema::{
    LEGACY_COLLECTIONS, LEGACY_SHA256, PRE_LEGAL_CONSENT_COLLECTIONS, PRE_LEGAL_CONSENT_SHA256,
};
use super::*;

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn pre_capture_archive_preserves_owned_backups_and_refuses_new_billing_evidence() {
    use super::super::schema::{PRE_CAPTURE_BACKUP_COLLECTIONS, PRE_CAPTURE_BACKUP_SHA256};

    let (temp, store, request, bytes) = fixture().await;
    store.admit(&request, &bytes).await.unwrap();
    store
        .database
        .collection::<Document>(crate::store::home_backups::BACKUPS_COLLECTION)
        .insert_many([
            doc! {"_id":"retained-backup", "sandbox_id":&request.sandbox_id,
            "document_schema_version":1_i64, "payload":"retained"},
            doc! {"_id":"other-backup", "sandbox_id":"sbx_other",
            "document_schema_version":1_i64, "payload":"excluded"},
        ])
        .await
        .unwrap();
    let custody = Custody::default();
    let current = export::build(&store, &request, &bytes, &custody, None)
        .await
        .unwrap();
    let mut previous = current.clone();
    previous.schema_sha256 = PRE_CAPTURE_BACKUP_SHA256.into();
    previous
        .database
        .retain(|row| PRE_CAPTURE_BACKUP_COLLECTIONS.contains(&row.collection.as_str()));
    let receipt = custody
        .retain(
            &request.operation_id,
            serde_json::to_vec(&previous).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        verify::load(&custody, &receipt, &request, &bytes)
            .await
            .unwrap(),
        previous
    );
    let restored = temp.0.join("pre-capture-restore");
    verify::restore(&custody, &receipt, &request, &bytes, &restored)
        .await
        .unwrap();
    let retained = std::fs::read(restored.join("database/home_backups.bson")).unwrap();
    let retained: Document = mongodb::bson::deserialize_from_slice(&retained).unwrap();
    assert_eq!(retained.get_str("_id").unwrap(), "retained-backup");
    assert!(super::super::matches_current(&previous, current).unwrap());
    let held = store.inspect(&request.sandbox_id).await.unwrap();
    for name in crate::store::sandbox_usage_export::COLLECTIONS {
        let collection = store.database.collection::<Document>(name);
        collection
            .insert_one(doc! {"_id":"retained-billing-evidence"})
            .await
            .unwrap();
        assert!(export::build(&store, &request, &bytes, &custody, None)
            .await
            .is_err());
        assert_eq!(store.inspect(&request.sandbox_id).await.unwrap(), held);
        collection.delete_many(doc! {}).await.unwrap();
    }
    assert_eq!(
        verify::load(&custody, &receipt, &request, &bytes)
            .await
            .unwrap(),
        previous
    );
    store.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn legacy_archive_readback_survives_upgrade_without_omitting_new_owned_data() {
    let (temp, store, request, bytes) = fixture().await;
    store.admit(&request, &bytes).await.unwrap();
    let custody = Custody::default();
    let current = export::build(&store, &request, &bytes, &custody, None)
        .await
        .unwrap();
    let audit: Vec<_> = super::super::census_catalog::AUDIT
        .iter()
        .copied()
        .filter(|(name, _)| LEGACY_COLLECTIONS.contains(name))
        .collect();
    let names: Vec<_> = crate::store::retirement_archive::manifest_collections()
        .iter()
        .copied()
        .filter(|name| LEGACY_COLLECTIONS.contains(name))
        .collect();
    let mut old = current.clone();
    old.schema_sha256 =
        digest(format!("archive-census-v2\n{audit:?}\n{}", names.join("\n")).as_bytes());
    assert_eq!(old.schema_sha256, LEGACY_SHA256);
    old.database
        .retain(|r| LEGACY_COLLECTIONS.contains(&r.collection.as_str()));
    let receipt = custody
        .retain(&request.operation_id, serde_json::to_vec(&old).unwrap())
        .await
        .unwrap();
    assert_eq!(
        verify::load(&custody, &receipt, &request, &bytes)
            .await
            .unwrap(),
        old
    );
    let restored = verify::restore(
        &custody,
        &receipt,
        &request,
        &bytes,
        &temp.0.join("old-restore"),
    )
    .await
    .unwrap();
    assert_eq!(
        restored.documents,
        old.database.iter().map(|r| r.documents).sum::<u64>()
    );
    assert!(super::super::matches_current(&old, current).unwrap());
    let held = store.inspect(&request.sandbox_id).await.unwrap();
    let collection = store
        .database
        .collection::<Document>(crate::store::allocation_buckets::COLLECTION);
    collection.insert_many([
        doc! {"_id":"owned-day","document_schema_version":1_i64,"sandbox_id":&request.sandbox_id,"payload":"retained"},
        doc! {"_id":"other-day","document_schema_version":1_i64,"sandbox_id":"sbx_other","payload":"excluded"},
    ]).await.unwrap();
    let fresh = export::build(&store, &request, &bytes, &custody, None)
        .await
        .unwrap();
    let row = fresh
        .database
        .iter()
        .find(|r| r.collection == crate::store::allocation_buckets::COLLECTION)
        .unwrap();
    assert_eq!(row.documents, 1);
    assert!(!super::super::matches_current(&old, fresh.clone()).unwrap());
    assert_eq!(store.inspect(&request.sandbox_id).await.unwrap(), held);
    let fresh_receipt = custody
        .retain(&request.operation_id, serde_json::to_vec(&fresh).unwrap())
        .await
        .unwrap();
    verify::restore(
        &custody,
        &fresh_receipt,
        &request,
        &bytes,
        &temp.0.join("new-restore"),
    )
    .await
    .unwrap();
    let retained = std::fs::read(
        temp.0
            .join("new-restore/database/sandbox_usage_allocation_buckets_v1.bson"),
    )
    .unwrap();
    let retained: Document = mongodb::bson::deserialize_from_slice(&retained).unwrap();
    assert_eq!(retained.get_str("_id").unwrap(), "owned-day");
    // The legacy manifest remains valid offline history, never authority to omit
    // the new row in a live completion census.
    verify::load(&custody, &receipt, &request, &bytes)
        .await
        .unwrap();
    store.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn populated_release_lanes_preserve_pre_pointer_archive_readback() {
    let (_temp, store, request, bytes) = fixture().await;
    store.admit(&request, &bytes).await.unwrap();
    let custody = Custody::default();
    let predecessor = export::build(&store, &request, &bytes, &custody, None)
        .await
        .unwrap();
    let release_collections = [
        crate::store::released_runtimes::COLLECTION,
        crate::store::released_runtimes::TRANSITIONS_COLLECTION,
    ];
    for name in release_collections {
        store
            .database
            .collection::<Document>(name)
            .insert_one(doc! {"_id":"deployment-release"})
            .await
            .unwrap();
    }
    let written = export::publish(&store, &request, &bytes, &custody, None)
        .await
        .unwrap();
    let current = verify::load(&custody, &written, &request, &bytes)
        .await
        .unwrap();
    // These are the preceding reader's exact schema and namespace checks.
    assert_eq!(current.schema_sha256, PRE_LEGAL_CONSENT_SHA256);
    assert_eq!(
        current
            .database
            .iter()
            .map(|row| row.collection.as_str())
            .collect::<Vec<_>>(),
        PRE_LEGAL_CONSENT_COLLECTIONS,
    );
    assert!(super::super::matches_current(&predecessor, current).unwrap());
    for name in release_collections {
        assert_eq!(
            store
                .database
                .collection::<Document>(name)
                .count_documents(doc! {})
                .await
                .unwrap(),
            1,
        );
    }
    store.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn pre_proof_manifest_reconciles_without_capturing_live_creation_evidence() {
    use crate::config::CreationOutcomesConfig;
    use crate::store::creation_outcomes::{CreationOutcomes, COLLECTION};
    use layer1_public::rest::creation_outcomes::{
        hash_fields, wire_digest, Algorithm, Backend, LookupRequest, Operation, Outcome,
        RequestFingerprint,
    };
    use layer2_core::SubjectId;

    let (temp, store, request, bytes) = fixture().await;
    let parent = store
        .parents()
        .find_one(doc! {"_id": &request.sandbox_id})
        .await
        .unwrap()
        .unwrap();
    let owner = super::super::super::sandboxes::read_sandbox(&parent)
        .unwrap()
        .owner;
    let custody = Custody::default();
    store.admit(&request, &bytes).await.unwrap();
    let pre_proof = export::build(&store, &request, &bytes, &custody, None)
        .await
        .unwrap();
    assert_eq!(
        pre_proof.schema_sha256,
        "c75c5dc57ddbcab6d18543510bf2ddbf5beb4387622c48914909bed80df10a8a"
    );
    assert!(!pre_proof
        .database
        .iter()
        .any(|row| row.collection == COLLECTION));
    let receipt = custody
        .retain(
            &request.operation_id,
            serde_json::to_vec(&pre_proof).unwrap(),
        )
        .await
        .unwrap();

    let deployment = "a".repeat(64);
    let epoch = "b".repeat(32);
    let nonce = "c".repeat(32);
    let now = mongodb::bson::DateTime::now();
    let day = now.timestamp_millis().div_euclid(86_400_000);
    let wire = wire_digest(b"{}");
    let path = Operation::Sandbox.path();
    let binding = hash_fields(&[
        "cma-creation-key-v1",
        &epoch,
        &day.to_string(),
        &nonce,
        owner.as_str(),
        &deployment,
        "POST",
        path,
        &wire,
    ]);
    let lookup = LookupRequest {
        version: 1,
        deployment: deployment.clone(),
        method: "POST".into(),
        path: path.into(),
        idempotency_key: format!("cma1.{epoch}.{day}.{nonce}.{binding}"),
        fingerprint: RequestFingerprint {
            algorithm: Algorithm::NyxidWire,
            digest: wire,
        },
        claim_id: None,
        operation_id: None,
    };
    let identity = lookup.identity(&owner, &deployment).unwrap();
    let record_id = hash_fields(&[
        "cma-creation-record-v1",
        &deployment,
        owner.as_str(),
        path,
        &lookup.idempotency_key,
    ]);
    let proofs = store.database.collection::<Document>(COLLECTION);
    proofs.insert_many([
        doc! {"_id":format!("coverage:{deployment}:{epoch}"), "document_schema_version":1_i64,
            "first_day":day, "closed_day":day-1, "admissions":1_i64},
        doc! {"_id": &record_id, "document_schema_version":1_i64,
            "owner": owner.as_str(), "deployment": &deployment, "operation":"sandbox", "epoch": &epoch,
            "day":day, "identity":identity.digest(), "generation":"d".repeat(32), "lease_until":now,
            "state":{"kind":"created", "sandbox_id":&request.sandbox_id}},
    ]).await.unwrap();
    let backend = CreationOutcomes::open(
        store.database.clone(),
        CreationOutcomesConfig {
            deployment,
            epoch,
            first_day: day,
            legacy_custody: false,
            terminal_settlement: true,
        },
    )
    .await
    .unwrap();
    let created = backend.lookup(&identity, &lookup).await.unwrap();
    assert!(
        matches!(&created, Outcome::Created {sandbox_id, ..} if sandbox_id == &request.sandbox_id)
    );

    let loaded = verify::load(&custody, &receipt, &request, &bytes)
        .await
        .unwrap();
    let fresh = export::build(&store, &request, &bytes, &custody, None)
        .await
        .unwrap();
    assert!(super::super::matches_current(&loaded, fresh.clone()).unwrap());
    let mut incomplete = loaded.clone();
    incomplete.database.pop();
    assert!(super::super::matches_current(&incomplete, fresh).is_err());
    verify::restore(
        &custody,
        &receipt,
        &request,
        &bytes,
        &temp.0.join("pre-proof-restore"),
    )
    .await
    .unwrap();
    assert!(!temp
        .0
        .join("pre-proof-restore/database")
        .join(format!("{COLLECTION}.bson"))
        .exists());
    assert_eq!(backend.lookup(&identity, &lookup).await.unwrap(), created);
    assert_eq!(
        proofs
            .count_documents(doc! {"owner":owner.as_str()})
            .await
            .unwrap(),
        1
    );
    assert!(backend
        .lookup(
            &lookup
                .identity(&SubjectId::new("other-owner"), backend.deployment())
                .unwrap(),
            &lookup
        )
        .await
        .is_err());
    store.database.drop().await.unwrap();
}
