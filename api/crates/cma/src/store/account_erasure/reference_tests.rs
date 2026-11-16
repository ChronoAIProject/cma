//! Real-store compatibility and interleaving contracts for account redaction.
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
async fn document_references_are_redacted_without_changing_the_owner() {
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
async fn global_budget_overrides_are_redacted() {
    use layer2_core::{
        budget::{BudgetPolicy, OwnerBudgetPolicy},
        PlatformSettings, PlatformSettingsStore,
    };
    let db = fixture().await;
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
async fn log_gate_redaction_waits_for_live_slots_and_preserves_other_cohorts() {
    use crate::store::agent_log_segments::writer_gate::{GateName, GateRecord};
    use mongodb::bson::{spec::BinarySubtype, Binary, DateTime};
    let db = fixture().await;
    let agents = BTreeSet::from(["agt-erased".into()]);
    let holder = "10:agt-erased:incarnation:3:key";
    let original = doc! {"_id":"agent_log_segments_v1","format_version":1_i64,"revision":1_i64,"admission_fence":2_i64,"policy":{"enabled":true,"owners":["erased","other"],"agents":["agt-erased","agt-other"],"cohort_seed":"","basis_points":0_i32},"slots":[{"holder":holder,"fingerprint":Binary {subtype:BinarySubtype::Generic,bytes:vec![1;32]},"fence":1_i64,"expires_at":DateTime::from_millis(DateTime::now().timestamp_millis()+300000)}]};
    GateRecord::read(GateName::Segments, original.clone()).unwrap();
    db.collection::<Document>("agent_log_gates")
        .insert_one(original.clone())
        .await
        .unwrap();
    assert_eq!(
        super::references::redact_with_agents(
            &db,
            "agent_log_gates",
            &original,
            &subjects(),
            &agents,
            &layer2_core::now_rfc3339()
        )
        .await
        .unwrap(),
        Redaction::Waiting
    );
    assert_eq!(
        row(&db, "agent_log_gates", "agent_log_segments_v1").await,
        original
    );
    db.collection::<Document>("agent_log_gates")
        .update_one(
            doc! {"_id":"agent_log_segments_v1"},
            doc! {"$set":{"slots.0.expires_at":DateTime::from_millis(1)}},
        )
        .await
        .unwrap();
    let expired = row(&db, "agent_log_gates", "agent_log_segments_v1").await;
    assert_eq!(
        super::references::redact_with_agents(
            &db,
            "agent_log_gates",
            &expired,
            &subjects(),
            &agents,
            &layer2_core::now_rfc3339()
        )
        .await
        .unwrap(),
        Redaction::Updated
    );
    let parsed = GateRecord::read(
        GateName::Segments,
        row(&db, "agent_log_gates", "agent_log_segments_v1").await,
    )
    .unwrap();
    assert_eq!(parsed.revision, 2);
    assert_eq!(parsed.policy.owners, ["other"]);
    assert_eq!(parsed.policy.agents, ["agt-other"]);
    assert!(parsed.slots.is_empty());
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn log_gate_redaction_disables_an_empty_rollout_without_expanding_it() {
    use crate::store::agent_log_segments::writer_gate::{GateName, GateRecord};
    let db = fixture().await;
    let original = doc! {"_id":"agent_log_segments_v1","format_version":1_i64,"revision":1_i64,"admission_fence":0_i64,"policy":{"enabled":true,"owners":["erased"],"agents":[],"cohort_seed":"","basis_points":0_i32},"slots":[]};
    db.collection::<Document>("agent_log_gates")
        .insert_one(original.clone())
        .await
        .unwrap();
    assert_eq!(
        super::references::redact_with_agents(
            &db,
            "agent_log_gates",
            &original,
            &subjects(),
            &BTreeSet::new(),
            &layer2_core::now_rfc3339()
        )
        .await
        .unwrap(),
        Redaction::Updated
    );
    let parsed = GateRecord::read(
        GateName::Segments,
        row(&db, "agent_log_gates", "agent_log_segments_v1").await,
    )
    .unwrap();
    assert!(!parsed.policy.enabled);
    assert!(parsed.policy.owners.is_empty());
    assert_eq!(parsed.revision, 2);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn log_gate_redaction_compares_slots_that_change_without_revision() {
    use crate::store::agent_log_segments::writer_gate::{GateName, GateRecord};
    use mongodb::bson::{spec::BinarySubtype, Binary, DateTime};
    let db = fixture().await;
    let original = doc! {"_id":"agent_log_segments_v1","format_version":1_i64,"revision":1_i64,"admission_fence":1_i64,"policy":{"enabled":true,"owners":["erased","other"],"agents":[],"cohort_seed":"","basis_points":0_i32},"slots":[{"holder":"10:agt-erased:incarnation:3:key","fingerprint":Binary {subtype:BinarySubtype::Generic,bytes:vec![1;32]},"fence":1_i64,"expires_at":DateTime::from_millis(1)}]};
    db.collection::<Document>("agent_log_gates")
        .insert_one(original.clone())
        .await
        .unwrap();
    let mut concurrent = original.clone();
    concurrent.get_array_mut("slots").unwrap()[0]
        .as_document_mut()
        .unwrap()
        .insert(
            "expires_at",
            DateTime::from_millis(DateTime::now().timestamp_millis() + 300000),
        );
    db.collection::<Document>("agent_log_gates")
        .replace_one(doc! {"_id":"agent_log_segments_v1"}, concurrent.clone())
        .await
        .unwrap();
    let agents = BTreeSet::from(["agt-erased".into()]);
    assert_eq!(
        super::references::redact_with_agents(
            &db,
            "agent_log_gates",
            &original,
            &subjects(),
            &agents,
            &layer2_core::now_rfc3339()
        )
        .await
        .unwrap(),
        Redaction::Conflict
    );
    assert_eq!(
        row(&db, "agent_log_gates", "agent_log_segments_v1").await,
        concurrent
    );
    GateRecord::read(GateName::Segments, concurrent).unwrap();
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
