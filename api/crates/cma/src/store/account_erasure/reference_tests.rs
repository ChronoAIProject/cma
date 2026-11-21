//! Real-store compatibility and interleaving contracts for account redaction.
mod log_gate_tests;

use super::references::{redact, Redaction};
use crate::document_artifacts::MongoDocumentStore;
use layer2_core::{document_artifacts::*, trigger_domain::*, SubjectId};
use mongodb::{
    bson::{doc, Document},
    Client, Database,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

pub(super) mod fixtures;
use fixtures::*;

async fn fixture() -> Database {
    let client = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    client.database(&format!(
        "cma_erasure_references_{}",
        uuid::Uuid::new_v4().simple()
    ))
}
fn subjects() -> BTreeSet<String> {
    BTreeSet::from(["erased".into()])
}
async fn row(db: &Database, collection: &str, id: &str) -> Document {
    db.collection::<Document>(collection)
        .find_one(doc! {"_id":id})
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn references_are_redacted_in_other_owners_documents_and_shares() {
    let db = fixture().await;
    let store = MongoDocumentStore::new(db.clone());
    let document = document();
    store.insert(&document).await.unwrap();
    let original = row(&db, "cma_document_artifacts", &document.metadata.id).await;
    assert_eq!(
        redact(&db, "cma_document_artifacts", &original, &subjects())
            .await
            .unwrap(),
        Redaction::Updated
    );
    let cleaned = store.get(&document.metadata.id).await.unwrap().unwrap();
    assert_eq!(cleaned.metadata.owner_subject, "survivor");
    assert_eq!(cleaned.metadata.revision, document.metadata.revision + 1);
    assert_eq!(
        cleaned
            .shares
            .iter()
            .map(|s| s.subject.as_str())
            .collect::<Vec<_>>(),
        ["other"]
    );
    assert_eq!(
        cleaned
            .comments
            .iter()
            .map(|c| c.author_subject.as_str())
            .collect::<Vec<_>>(),
        ["other"]
    );
    let receipt = cleaned
        .receipts
        .get("comment-receipt")
        .unwrap()
        .outcome
        .comment
        .as_ref()
        .unwrap();
    assert_eq!(receipt.author_subject, "deleted-account");
    assert!(receipt.body.is_empty());
    let encoded = row(&db, "cma_document_artifacts", &document.metadata.id).await;
    assert_eq!(
        encoded.get_array("recipient_subjects").unwrap(),
        &vec!["other".into()]
    );
    let share = share();
    db.collection::<Document>("resource_shares")
        .insert_one(share.clone())
        .await
        .unwrap();
    assert_eq!(
        redact(&db, "resource_shares", &share, &subjects())
            .await
            .unwrap(),
        Redaction::Updated
    );
    let cleaned = row(&db, "resource_shares", "share-one").await;
    let parsed = crate::store::resource_access::records::read_share(
        &layer2_core::SandboxId("sbx-other".into()),
        &cleaned,
    )
    .unwrap();
    assert_eq!(
        parsed.link.unwrap().accepted_subjects,
        [SubjectId::new("other")]
    );
    assert_eq!(parsed.revision, 2);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn redaction_uses_revision_cas() {
    let db = fixture().await;
    let store = MongoDocumentStore::new(db.clone());
    let original = document();
    store.insert(&original).await.unwrap();
    let stale = row(&db, "cma_document_artifacts", &original.metadata.id).await;
    let mut concurrent = original.clone();
    concurrent.metadata.revision += 1;
    concurrent.shares.push(DocumentShare {
        subject: "new-recipient".into(),
        permission: DocumentPermission::Read,
    });
    assert!(store
        .compare_exchange(original.metadata.revision, &concurrent)
        .await
        .unwrap());
    assert_eq!(
        redact(&db, "cma_document_artifacts", &stale, &subjects())
            .await
            .unwrap(),
        Redaction::Conflict
    );
    let current = store.get(&original.metadata.id).await.unwrap().unwrap();
    assert_eq!(current.metadata.revision, concurrent.metadata.revision);
    assert!(current.shares.iter().any(|s| s.subject == "new-recipient"));
    let fresh = row(&db, "cma_document_artifacts", &original.metadata.id).await;
    assert_eq!(
        redact(&db, "cma_document_artifacts", &fresh, &subjects())
            .await
            .unwrap(),
        Redaction::Updated
    );
    let cleaned = store.get(&original.metadata.id).await.unwrap().unwrap();
    assert!(cleaned.shares.iter().any(|s| s.subject == "new-recipient"));
    assert!(!cleaned.shares.iter().any(|s| s.subject == "erased"));
    assert_eq!(cleaned.metadata.revision, concurrent.metadata.revision + 1);
    let mut stale_write = concurrent.clone();
    stale_write.metadata.revision += 1;
    assert!(!store
        .compare_exchange(concurrent.metadata.revision, &stale_write)
        .await
        .unwrap());
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn redacted_document_and_revision_still_parse_with_current_readers() {
    let db = fixture().await;
    let documents = MongoDocumentStore::new(db.clone());
    let original = document();
    documents.insert(&original).await.unwrap();
    let original_row = row(&db, "cma_document_artifacts", &original.metadata.id).await;
    redact(&db, "cma_document_artifacts", &original_row, &subjects())
        .await
        .unwrap();
    assert!(documents
        .get(&original.metadata.id)
        .await
        .unwrap()
        .is_some());
    assert_eq!(
        documents
            .list("other", &DocumentQuery::default())
            .await
            .unwrap()
            .len(),
        1
    );
    let (revision_row, revision, trigger_id) = revision();
    db.collection::<Document>("trigger_definition_revisions")
        .insert_one(revision_row.clone())
        .await
        .unwrap();
    let profiles = Arc::new(
        crate::store::sandbox_mongo::MongoProfileStore::open(db.client(), db.name())
            .await
            .unwrap(),
    );
    let triggers =
        crate::store::MongoTriggerDefinitionStore::open(db.client(), db.name(), profiles)
            .await
            .unwrap();
    let launch = launch(&trigger_id, &revision.digest);
    let launch_row = encode_launch(&launch);
    db.collection::<Document>("trigger_launches")
        .insert_one(launch_row.clone())
        .await
        .unwrap();
    assert!(triggers
        .launch(&launch.id, &launch.owner)
        .await
        .unwrap()
        .is_some());
    assert_eq!(
        redact(
            &db,
            "trigger_definition_revisions",
            &revision_row,
            &subjects()
        )
        .await
        .unwrap(),
        Redaction::Updated
    );
    let tombstone = triggers
        .trigger_revision(&trigger_id, &revision.digest)
        .await
        .unwrap()
        .unwrap();
    tombstone.validate().unwrap();
    assert_eq!(tombstone.digest, revision.digest);
    assert_eq!(tombstone.published_at, revision.published_at);
    assert_eq!(tombstone.revision, revision.revision);
    assert!(tombstone.content.presentation.title.is_empty());
    assert!(tombstone.content.presentation.description.is_empty());
    assert!(tombstone.content.first_message.is_empty());
    let mut corrupt = tombstone.clone();
    corrupt.content.first_message = "Restored publisher message".into();
    assert!(corrupt.validate().is_err());
    let mut corrupt = tombstone.clone();
    corrupt.content = revision.content.clone();
    assert!(corrupt.validate().is_err());
    let stored = row(
        &db,
        "trigger_definition_revisions",
        revision_row.get_str("_id").unwrap(),
    )
    .await;
    assert_eq!(stored.get_str("owner").unwrap(), "deleted-account");
    assert_eq!(stored.get_i64("mutation_version").unwrap(), 1);
    assert_eq!(
        triggers
            .launch(&launch.id, &launch.owner)
            .await
            .unwrap()
            .unwrap(),
        launch
    );
    assert_eq!(
        row(&db, "trigger_launches", launch.id.as_str()).await,
        launch_row
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn shared_receipts_and_global_budget_overrides_are_redacted() {
    use layer2_core::{
        budget::{BudgetPolicy, OwnerBudgetPolicy},
        resource_access::{ResourceActor, ShareMutationReceipt},
        PlatformSettings, PlatformSettingsStore,
    };
    let db = fixture().await;
    let share_row = share();
    let share =
        mongodb::bson::deserialize_from_bson(share_row.get("record").unwrap().clone()).unwrap();
    let receipt = ShareMutationReceipt {
        operation_id: digest(b"operation"),
        command_fingerprint: digest(b"share-command"),
        sandbox_id: layer2_core::SandboxId("sbx-other".into()),
        sandbox_created_at: AT.into(),
        share,
        actor: ResourceActor::Human {
            subject: SubjectId::new("survivor"),
        },
        at: AT.into(),
        policy_revision: 1,
    };
    receipt.validate().unwrap();
    let receipt_row = doc! {"_id":&receipt.operation_id,"document_schema_version":1_i64,"share_id":&receipt.share.id,"sandbox_id":"sbx-other","record":mongodb::bson::serialize_to_bson(&receipt).unwrap()};
    db.collection::<Document>("resource_share_commands")
        .insert_one(receipt_row.clone())
        .await
        .unwrap();
    assert_eq!(
        redact(&db, "resource_share_commands", &receipt_row, &subjects())
            .await
            .unwrap(),
        Redaction::Updated
    );
    let stored = row(&db, "resource_share_commands", &receipt.operation_id).await;
    let parsed: ShareMutationReceipt =
        mongodb::bson::deserialize_from_bson(stored.get("record").unwrap().clone()).unwrap();
    parsed.validate().unwrap();
    assert_eq!(
        parsed.share.link.unwrap().accepted_subjects,
        [SubjectId::new("other")]
    );
    assert_eq!(stored.get_i64("mutation_version").unwrap(), 1);
    let settings_store = crate::store::MongoPlatformSettingsStore::open(db.client(), db.name())
        .await
        .unwrap();
    let policy = BudgetPolicy {
        owner_day_limit: 100,
        turn_limit: 10,
        max_calls_per_turn: 10,
        day_timezone: "UTC".into(),
        input_token_units: 1,
        output_token_units: 1,
        max_unobserved_units: 1,
        usage_timeout_seconds: 30,
    };
    let mut settings = PlatformSettings::default();
    settings.model_budget.owners = vec![
        OwnerBudgetPolicy {
            owner: SubjectId::new("erased"),
            policy: policy.clone(),
        },
        OwnerBudgetPolicy {
            owner: SubjectId::new("other"),
            policy,
        },
    ];
    settings_store.replace(0, &settings).await.unwrap();
    let settings_row = row(&db, "platform_settings", "platform").await;
    assert_eq!(
        redact(&db, "platform_settings", &settings_row, &subjects())
            .await
            .unwrap(),
        Redaction::Updated
    );
    let cleaned = settings_store.read().await.unwrap();
    assert_eq!(cleaned.revision, 2);
    assert_eq!(cleaned.settings.model_budget.owners.len(), 1);
    assert_eq!(
        cleaned.settings.model_budget.owners[0].owner,
        SubjectId::new("other")
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn erasing_last_explicit_share_recipient_preserves_a_revoked_readable_link() {
    use layer2_core::resource_access::{AgentShareAudience, ResourceShare};
    let db = fixture().await;
    let mut original = share();
    let mut record: ResourceShare =
        mongodb::bson::deserialize_from_bson(original.get("record").unwrap().clone()).unwrap();
    let link = record.link.as_mut().unwrap();
    link.audience = AgentShareAudience::Users {
        subjects: vec![SubjectId::new("erased")],
    };
    link.accepted_subjects = vec![SubjectId::new("erased")];
    assert!(record.validate());
    original.insert("accepted_subjects", vec!["erased"]);
    original.insert("record", mongodb::bson::serialize_to_bson(&record).unwrap());
    db.collection::<Document>("resource_shares")
        .insert_one(original.clone())
        .await
        .unwrap();
    assert_eq!(
        redact(&db, "resource_shares", &original, &subjects())
            .await
            .unwrap(),
        Redaction::Updated
    );
    let cleaned = row(&db, "resource_shares", "share-one").await;
    let parsed = crate::store::resource_access::records::read_share(
        &layer2_core::SandboxId("sbx-other".into()),
        &cleaned,
    )
    .unwrap();
    assert!(parsed.revoked_at.is_some());
    let link = parsed.link.as_ref().unwrap();
    assert!(matches!(&link.audience,AgentShareAudience::Users {subjects} if subjects.is_empty()));
    assert!(link.accepted_subjects.is_empty());
    assert!(!parsed.incoming_for(&SubjectId::new("erased"), &[]));
    assert!(!parsed.incoming_for(&SubjectId::new("other"), &[]));
    let mut invalid_active = parsed;
    invalid_active.revoked_at = None;
    assert!(!invalid_active.validate());
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn corrupted_reference_projection_is_refused_without_overwriting_the_row() {
    let db = fixture().await;
    let store = MongoDocumentStore::new(db.clone());
    let original = document();
    store.insert(&original).await.unwrap();
    db.collection::<Document>("cma_document_artifacts")
        .update_one(
            doc! {"_id":&original.metadata.id},
            doc! {"$set":{"revision":99_i64}},
        )
        .await
        .unwrap();
    let corrupted = row(&db, "cma_document_artifacts", &original.metadata.id).await;
    assert!(matches!(
        redact(&db, "cma_document_artifacts", &corrupted, &subjects()).await,
        Err(super::Error::InvalidRecord("cma_document_artifacts"))
    ));
    assert_eq!(
        row(&db, "cma_document_artifacts", &original.metadata.id).await,
        corrupted
    );
    let (mut invalid, _, _) = revision();
    invalid
        .get_document_mut("record")
        .unwrap()
        .get_document_mut("content")
        .unwrap()
        .insert("first_message", "Changed without digest");
    db.collection::<Document>("trigger_definition_revisions")
        .insert_one(invalid.clone())
        .await
        .unwrap();
    assert!(matches!(
        redact(&db, "trigger_definition_revisions", &invalid, &subjects()).await,
        Err(super::Error::InvalidRecord("trigger_definition_revisions"))
    ));
    assert_eq!(
        row(
            &db,
            "trigger_definition_revisions",
            invalid.get_str("_id").unwrap()
        )
        .await,
        invalid
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn last_budget_owner_redaction_preserves_a_readable_disabled_policy() {
    use layer2_core::{
        budget::{BudgetPolicy, OwnerBudgetPolicy, PlatformBudgetPolicy},
        PlatformSettings, PlatformSettingsStore,
    };
    let db = fixture().await;
    let store = crate::store::MongoPlatformSettingsStore::open(db.client(), db.name())
        .await
        .unwrap();
    let mut settings = PlatformSettings::default();
    settings.model_budget.enabled = true;
    settings.model_budget.platform = Some(PlatformBudgetPolicy {
        period_cap: 10000,
        period_start: "2026-11-01T00:00:00Z".into(),
        period_end: "2026-12-01T00:00:00Z".into(),
        warning_percent: 80,
        stop_percent: 100,
        protected_reserve: 0,
        lag_allowance: 0,
        cloud: None,
    });
    settings.model_budget.owners.push(OwnerBudgetPolicy {
        owner: SubjectId::new("erased"),
        policy: BudgetPolicy {
            owner_day_limit: 100,
            turn_limit: 10,
            max_calls_per_turn: 10,
            day_timezone: "UTC".into(),
            input_token_units: 1,
            output_token_units: 1,
            max_unobserved_units: 1,
            usage_timeout_seconds: 30,
        },
    });
    settings.validate().unwrap();
    store.replace(0, &settings).await.unwrap();
    let original = row(&db, "platform_settings", "platform").await;
    assert_eq!(
        redact(&db, "platform_settings", &original, &subjects())
            .await
            .unwrap(),
        Redaction::Updated
    );
    let cleaned = store.read().await.unwrap();
    cleaned.settings.validate().unwrap();
    assert!(!cleaned.settings.model_budget.enabled);
    assert!(cleaned.settings.model_budget.owners.is_empty());
    assert_eq!(
        cleaned.settings.model_budget.platform,
        settings.model_budget.platform
    );
    assert_eq!(cleaned.settings.limits, settings.limits);
    assert_eq!(cleaned.revision, 2);
    db.drop().await.unwrap();
}
