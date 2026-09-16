use super::*;
use crate::auth::keys::bridge::test_fixture::{HUMAN_SECRET, RUNTIME_SECRET};
use crate::sandbox::{
    channel_provisioning::{
        home_prepare_evidence::ChannelHomePrepareEvidence, home_provenance::ChannelHomeProvenance,
    },
    channel_retirement::assessment::{
        FinalRetentionAssessment, FinalRetentionHoldReason as Hold, RetainedHome,
    },
};
use layer2_core::{
    bridge::allocation::AllocationError,
    sandbox::{store::SandboxEventStore, SandboxEventKind},
};
use mongodb::bson::Bson;
use std::time::Duration;
#[path = "final_retention_fixture.rs"]
mod fixture;
use fixture::*;

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_final_retention_reports_original_debt_and_keeps_exact_storage_audit_and_history() {
    let f = retained_home().await;
    let original = document(&f).await;
    let audits = rows(&f, "channel_retirement_audits").await;
    let history = rows(&f, "agent_log").await;
    let storage = f.f.original.adapter.home_volumes().unwrap();
    let inventory = storage.inventory().await.unwrap();
    let assessment = assessed(&f).await;
    assert_eq!(assessment.home, RetainedHome::Retained);
    assert_eq!(
        assessment.reasons,
        vec![
            Hold::HomePrepareDispatchUnresolved,
            Hold::HomeGenerationHistoryUnresolved,
            Hold::ProviderIncarnationUnproven,
            Hold::BackingStorageAbsenceUnproven,
            Hold::PhysicalHomeCleanupUnavailable,
            Hold::ChannelHistoryPolicyRequired,
            Hold::ReaderJanitorCompatibilityRequired,
        ]
    );
    assert_eq!(
        assessment.identity.storage_retention_sha256,
        retention(&f).await.unwrap().digest().unwrap()
    );
    assert_eq!(assessment.identity.allocation_id, f.f.original.record.id);
    assert_eq!(assessment.identity.sandbox_id, f.sandbox().await.id);
    assert!(assessment
        .identity
        .original_home_provenance_sha256
        .is_some());
    assert!(assessment.identity.home_prepare_evidence_sha256.is_some());
    let audit = audits
        .iter()
        .find(|audit| audit.get_str("action").ok() == Some("storage_retained"))
        .unwrap();
    assert_eq!(
        assessment.identity.storage_audit.id,
        audit.get_str("_id").unwrap()
    );
    assert_eq!(
        assessed(&f).await,
        assessment,
        "new worker claims preserve immutable assessment identity"
    );
    f.retry().await;
    f.step().await;
    let events =
        f.f.original
            .resources
            .sandbox_events(&f.sandbox().await.id, 0, 100)
            .await
            .unwrap();
    let failure = events
        .iter()
        .rev()
        .find_map(|event| match &event.kind {
            SandboxEventKind::WorkStepFailed { failure, .. } => Some(failure),
            _ => None,
        })
        .unwrap();
    assert_eq!(failure.class, "channel_retention_held");
    assert_eq!(failure.detail, assessment.hold_detail());
    assert!(failure.detail.len() <= 256);
    assert!(!failure.detail.contains(&assessment.identity.allocation_id));
    assert!(!failure.detail.contains(HUMAN_SECRET));
    assert!(!failure.detail.contains(RUNTIME_SECRET));
    assert_preserved(&f, &original, &audits).await;
    assert_eq!(rows(&f, "agent_log").await, history);
    assert_eq!(storage.inventory().await.unwrap(), inventory);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_final_retention_allows_legacy_runtime_cleanup_and_reports_missing_evidence() {
    let original =
        crate::store::bridge_definitions::mongo_integration_tests::allocations::channel_provisioning::home::seed().await;
    let f = RetirementFixture::from_renewal(RenewalFixture::from_original(original).await);
    unset(
        &f,
        doc! {"channel_home_provenance":"", "channel_home_prepare_evidence":""},
    )
    .await;
    f.start().await;
    for _ in 0..6 {
        f.step().await;
    }
    assert_eq!(f.f.state.revoke_count.load(Ordering::SeqCst), 1);
    let original = document(&f).await;
    let audits = rows(&f, "channel_retirement_audits").await;
    let assessment = assessed(&f).await;
    assert_eq!(
        assessment.reasons,
        vec![
            Hold::OriginalHomeProvenanceMissing,
            Hold::HomePrepareEvidenceMissing,
            Hold::ChannelHistoryPolicyRequired,
            Hold::ReaderJanitorCompatibilityRequired,
        ]
    );
    assert!(assessment
        .identity
        .original_home_provenance_sha256
        .is_none());
    assert!(assessment.identity.home_prepare_evidence_sha256.is_none());
    assert_preserved(&f, &original, &audits).await;
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_final_retention_distinguishes_missing_prepare_and_late_bound_original_receipt() {
    let f = retained_home().await;
    let original = document(&f).await;
    let before = assessed(&f).await;
    let mut provenance: ChannelHomeProvenance = mongodb::bson::deserialize_from_bson(
        original.get("channel_home_provenance").unwrap().clone(),
    )
    .unwrap();
    provenance.first_receipt = None;
    set(&f, doc! {"channel_home_provenance":crate::store::sandbox_mongo::record_bson(&provenance).unwrap()}).await;
    let unbound = assessed(&f).await;
    assert_eq!(unbound.reasons[0], Hold::OriginalHomeReceiptUnbound);
    assert!(unbound
        .reasons
        .contains(&Hold::HomePrepareDispatchUnresolved));
    assert_eq!(
        unbound.identity.home_prepare_evidence_sha256,
        before.identity.home_prepare_evidence_sha256
    );
    assert_ne!(
        unbound.identity.original_home_provenance_sha256,
        before.identity.original_home_provenance_sha256
    );
    assert_eq!(
        unbound.identity.storage_audit,
        before.identity.storage_audit
    );
    assert!(unbound.hold_detail().len() <= 256);
    unset(&f, doc! {"channel_home_prepare_evidence":""}).await;
    let missing = assessed(&f).await;
    assert_eq!(
        missing.reasons,
        vec![
            Hold::OriginalHomeReceiptUnbound,
            Hold::HomePrepareEvidenceMissing,
            Hold::ChannelHistoryPolicyRequired,
            Hold::ReaderJanitorCompatibilityRequired,
        ]
    );
    // Restore fixture evidence exactly as an old worker's later receipt bind
    // would leave it. The permanent original dispatch debt remains unresolved.
    set(&f, doc! {
        "channel_home_provenance":original.get("channel_home_provenance").unwrap().clone(),
        "channel_home_prepare_evidence":original.get("channel_home_prepare_evidence").unwrap().clone(),
    }).await;
    assert_eq!(assessed(&f).await, before);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_final_retention_refuses_corrupt_swapped_and_orphan_original_home_evidence() {
    let f = retained_home().await;
    let original = document(&f).await;
    let provenance: ChannelHomeProvenance = mongodb::bson::deserialize_from_bson(
        original.get("channel_home_provenance").unwrap().clone(),
    )
    .unwrap();
    let debt: ChannelHomePrepareEvidence = mongodb::bson::deserialize_from_bson(
        original
            .get("channel_home_prepare_evidence")
            .unwrap()
            .clone(),
    )
    .unwrap();
    let audits = rows(&f, "channel_retirement_audits").await;
    for field in [
        "allocation",
        "root",
        "created_at",
        "owner",
        "approver",
        "request",
        "receipt",
        "scope",
        "schema",
        "null",
        "orphan",
    ] {
        let mut changed = provenance.clone();
        match field {
            "allocation" => changed.allocation_id.push_str("-foreign"),
            "root" => changed.sandbox_id.0.push_str("-foreign"),
            "created_at" => changed.sandbox_created_at = "2001-01-01T00:00:00Z".into(),
            "owner" => {
                changed.owner = layer2_core::resource_access::ResourceOwner::Person {
                    subject: layer2_core::SubjectId("foreign".into()),
                }
            }
            "approver" => {
                changed.creation_approver = layer2_core::resource_access::ResourceActor::Human {
                    subject: layer2_core::SubjectId("foreign".into()),
                }
            }
            "request" => changed.request.volume_id.push_str("-foreign"),
            "receipt" => changed
                .first_receipt
                .as_mut()
                .unwrap()
                .storage_generation
                .push_str("-foreign"),
            "scope" => {
                changed.scope = serde_json::from_value(serde_json::json!("invalid")).unwrap()
            }
            "schema" => changed.schema_version = 2,
            _ => (),
        }
        if field == "orphan" {
            unset(&f, doc! {"channel_home_provenance":""}).await;
        } else {
            set(&f, doc! {"channel_home_provenance":if field == "null" {Bson::Null} else {crate::store::sandbox_mongo::record_bson(&changed).unwrap()}}).await;
        }
        assert!(read(&f).await.is_err(), "{field}");
        set(&f, doc! {"channel_home_provenance":original.get("channel_home_provenance").unwrap().clone()}).await;
    }
    for field in [
        "allocation",
        "provenance_digest",
        "request_digest",
        "schema",
        "null",
        "unknown_finality",
    ] {
        let mut changed = debt.clone();
        match field {
            "allocation" => changed.allocation_id.push_str("-foreign"),
            "provenance_digest" => changed.original_provenance_sha256 = "0".repeat(64),
            "request_digest" => changed.request_sha256 = "0".repeat(64),
            "schema" => changed.schema_version = 2,
            _ => (),
        }
        let mut value = crate::store::sandbox_mongo::record_bson(&changed).unwrap();
        if field == "null" {
            value = Bson::Null;
        }
        if field == "unknown_finality" {
            value
                .as_document_mut()
                .unwrap()
                .insert("backing_storage_absence", "proven");
        }
        set(&f, doc! {"channel_home_prepare_evidence":value}).await;
        assert!(read(&f).await.is_err(), "{field}");
    }
    set(&f, doc! {"channel_home_prepare_evidence":original.get("channel_home_prepare_evidence").unwrap().clone()}).await;
    assert_preserved(&f, &original, &audits).await;
    assert!(assessed(&f)
        .await
        .reasons
        .contains(&Hold::BackingStorageAbsenceUnproven));
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_final_retention_requires_unique_exact_permanent_storage_audit() {
    let f = retained_home().await;
    let original = document(&f).await;
    let audits = rows(&f, "channel_retirement_audits").await;
    let collection = f.db().collection::<Document>("channel_retirement_audits");
    let audit = collection
        .find_one(doc! {"action":"storage_retained"})
        .await
        .unwrap()
        .unwrap();
    let key = audit.get_str("_id").unwrap();
    for field in [
        "missing",
        "duplicate",
        "allocation_id",
        "sandbox_id",
        "sandbox_created_at",
        "tree_fingerprint",
        "operation_id",
        "action",
        "human_actor",
        "receipt",
        "storage_retention",
        "malformed",
        "appended_at",
        "fence",
        "document_schema_version",
        "id_revision",
        "id_future",
        "id_noncanonical",
    ] {
        collection.delete_one(doc! {"_id":key}).await.unwrap();
        let mut changed = audit.clone();
        match field {
            "missing" | "duplicate" => (),
            "human_actor" => {
                changed.insert(
                    field,
                    crate::store::sandbox_mongo::record_bson(
                        &layer2_core::resource_access::ResourceActor::Human {
                            subject: layer2_core::SubjectId("foreign-human".into()),
                        },
                    )
                    .unwrap(),
                );
            }
            "receipt" | "storage_retention" => {
                changed
                    .get_document_mut(field)
                    .unwrap()
                    .insert("tree_fingerprint", "foreign-tree");
            }
            "malformed" => {
                changed.insert("storage_retention", Bson::Null);
            }
            "fence" => {
                changed.insert(field, 0_i64);
            }
            "document_schema_version" => {
                changed.insert(field, 99_i64);
            }
            "id_revision" => {
                changed.insert(
                    "_id",
                    format!("{}:0:storage_retained", f.f.original.record.id),
                );
            }
            "id_future" => {
                changed.insert(
                    "_id",
                    format!("{}:{}:storage_retained", f.f.original.record.id, u64::MAX),
                );
            }
            "id_noncanonical" => {
                changed.insert(
                    "_id",
                    format!("{}:01:storage_retained", f.f.original.record.id),
                );
            }
            _ => {
                changed.insert(field, "foreign");
            }
        }
        if field != "missing" {
            collection.insert_one(changed.clone()).await.unwrap();
        }
        if field == "duplicate" {
            let mut duplicate = audit.clone();
            duplicate.insert("_id", "foreign-storage-retention-audit");
            duplicate.insert("sandbox_id", "foreign-root");
            collection.insert_one(duplicate).await.unwrap();
        }
        assert!(read(&f).await.is_err(), "{field}");
        collection
            .delete_one(doc! {"_id":changed.get("_id").unwrap().clone()})
            .await
            .unwrap();
        collection
            .delete_one(doc! {"_id":"foreign-storage-retention-audit"})
            .await
            .unwrap();
        collection.insert_one(audit.clone()).await.unwrap();
    }
    assert_preserved(&f, &original, &audits).await;
    assert_eq!(assessed(&f).await.identity.storage_audit.id, key);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_final_retention_unconfigured_home_still_requires_history_and_reader_proof() {
    let f = with_checkpoint().await;
    f.step().await;
    let original = document(&f).await;
    let audits = rows(&f, "channel_retirement_audits").await;
    let archives = rows(&f, "checkpoint_objects").await;
    let assessment = assessed(&f).await;
    assert_eq!(assessment.home, RetainedHome::NotConfigured);
    assert_eq!(
        assessment.reasons,
        vec![
            Hold::ChannelHistoryPolicyRequired,
            Hold::ReaderJanitorCompatibilityRequired
        ]
    );
    assert!(assessment
        .identity
        .original_home_provenance_sha256
        .is_none());
    assert!(assessment.identity.home_prepare_evidence_sha256.is_none());
    f.retry().await;
    f.step().await;
    assert_preserved(&f, &original, &audits).await;
    assert_eq!(rows(&f, "checkpoint_objects").await, archives);
    assert_eq!(
        f.f.original
            .resources
            .checkpoint_objects
            .cleanup_due(SystemTime::now())
            .await
            .unwrap(),
        0
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_final_retention_lost_prepare_then_current_volume_absence_never_clears_debt() {
    let (f, homes) = lost_prepare_home().await;
    let original = document(&f).await;
    let audits = rows(&f, "channel_retirement_audits").await;
    let before = assessed(&f).await;
    assert!(!before.reasons.contains(&Hold::OriginalHomeReceiptUnbound));
    assert!(before
        .reasons
        .contains(&Hold::HomeGenerationHistoryUnresolved));
    assert_eq!(homes.prepares.load(Ordering::SeqCst), 2);
    assert_eq!(homes.observed.lock().await.len(), 1);
    homes.observed.lock().await.clear();
    let reads = homes.reads.load(Ordering::SeqCst);
    assert_eq!(assessed(&f).await, before);
    f.retry().await;
    f.step().await;
    assert_eq!(
        homes.reads.load(Ordering::SeqCst),
        reads,
        "assessment cannot treat a current provider observation as finality"
    );
    assert_eq!(homes.prepares.load(Ordering::SeqCst), 2);
    assert_preserved(&f, &original, &audits).await;
    assert!(!f.sandbox().await.lifecycle.home.unwrap().released);
    f.close().await;
}
