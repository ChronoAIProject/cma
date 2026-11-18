use super::*;
use layer2_core::{PlatformSettingsStore, StoredPlatformSettings};

pub(super) async fn enable(
    client: &Client,
    name: &str,
) -> (
    crate::store::settings::MongoPlatformSettingsStore,
    StoredPlatformSettings,
) {
    let settings = crate::store::settings::MongoPlatformSettingsStore::open(client, name)
        .await
        .unwrap();
    let current = settings.read().await.unwrap();
    let mut enabled = current.settings;
    enabled.inactivity.auto_pause_enabled = true;
    enabled.inactivity.grace_started_at = Some(IDLE_AT.into());
    let stored = settings.replace(current.revision, &enabled).await.unwrap();
    (settings, stored)
}

pub(super) fn global(id: &SandboxId) -> SandboxInactivityPauseRequest {
    SandboxInactivityPauseRequest {
        auto_pause_enabled: true,
        expected_policy_revision: None,
        ..pause(id)
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn always_on_approval_after_preview_fences_both_automatic_and_operator_pause() {
    use layer2_core::sandbox::store::InactivityPauseCause;
    for cause in [
        InactivityPauseCause::Inactivity,
        InactivityPauseCause::OperatorInactivity,
    ] {
        let (client, name, store) = database().await;
        let (sandbox, ready) = fixture(&store, "sbx_approval_race").await;
        let (_, policy) = enable(&client, &name).await;
        let replica = MongoSandboxStore::open(&client, &name).await.unwrap();
        let request = SandboxInactivityPauseRequest {
            cause,
            expected_policy_revision: Some(policy.revision),
            ..global(&sandbox.id)
        };
        assert_eq!(
            store
                .preview_inactivity_pause(request.clone())
                .await
                .unwrap(),
            SandboxMaintenanceOutcome::Scheduled
        );
        let mut session = client.start_session().await.unwrap();
        start_transaction(&mut session).await.unwrap();
        evidence::live_activities(&store, &[ready], &mut session, LATER)
            .await
            .unwrap();
        let approved = replica
            .set_sandbox_always_on(sandbox.id.clone(), sandbox.revision, true)
            .await
            .unwrap();
        let result = transaction::apply(
            &store,
            &MaintenanceRequest::Inactivity(request.clone()),
            &mut session,
        )
        .await;
        assert!(matches!(result, Err(ApplyError::Driver(error)) if is_transient(&error)));
        let _ = session.abort_transaction().await;
        assert_eq!(
            store
                .schedule_inactivity_pause(request.clone())
                .await
                .unwrap(),
            SandboxMaintenanceOutcome::NotEligible
        );
        assert_eq!(
            store.get_sandbox(&sandbox.id).await.unwrap(),
            Some(approved.clone())
        );
        assert!(store
            .sandbox_events(&sandbox.id, 0, 100)
            .await
            .unwrap()
            .is_empty());
        replica
            .set_sandbox_always_on(sandbox.id.clone(), approved.revision, false)
            .await
            .unwrap();
        assert_eq!(
            store.schedule_inactivity_pause(request).await.unwrap(),
            SandboxMaintenanceOutcome::Scheduled
        );
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn historical_availability_survives_enablement_grace_and_store_restart() {
    use crate::store::schema::DOCUMENT_SCHEMA_VERSION;
    use mongodb::bson::Document;
    for preference in [None, Some(false), Some(true)] {
        let (client, name, store) = database().await;
        let mut sandbox = active_sandbox("sbx_historical");
        sandbox.profile.always_on = preference;
        store.create_sandbox(sandbox.clone()).await.unwrap();
        let ready = ready_agent(&sandbox, "agt_historical");
        insert_agent(&store, &ready, IDLE_AT).await;
        let rows = client
            .database(&name)
            .collection::<Document>("sandbox_retained_availability");
        let historical = doc! {
            "_id": sandbox.id.as_str(), "document_schema_version": DOCUMENT_SCHEMA_VERSION,
            "owner": sandbox.owner.as_str(), "sandbox_created_at": &sandbox.created_at,
            "profile_id": sandbox.profile.id.as_str(), "always_keep_alive": true,
        };
        rows.insert_one(historical.clone()).await.unwrap();
        for request in [pause(&sandbox.id), global(&sandbox.id)] {
            assert_eq!(
                store.schedule_inactivity_pause(request).await.unwrap(),
                SandboxMaintenanceOutcome::NotEligible
            );
        }
        let (settings, mut policy) = enable(&client, &name).await;
        policy.settings.inactivity.grace_started_at = Some(LATER.into());
        let policy = settings
            .replace(policy.revision, &policy.settings)
            .await
            .unwrap();
        let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
        let reopened_settings =
            crate::store::settings::MongoPlatformSettingsStore::open(&client, &name)
                .await
                .unwrap();
        assert_eq!(reopened_settings.read().await.unwrap(), policy);
        let before_grace = SandboxInactivityPauseRequest {
            requested_at: "2026-09-05T00:44:59Z".into(),
            ..global(&sandbox.id)
        };
        assert_eq!(
            reopened
                .schedule_inactivity_pause(before_grace)
                .await
                .unwrap(),
            SandboxMaintenanceOutcome::NotEligible
        );
        assert_eq!(
            reopened.get_sandbox(&sandbox.id).await.unwrap(),
            Some(sandbox.clone())
        );
        let after_grace = SandboxInactivityPauseRequest {
            requested_at: "2026-09-05T00:45:00Z".into(),
            ..global(&sandbox.id)
        };
        let expected = if preference == Some(true) {
            SandboxMaintenanceOutcome::NotEligible
        } else {
            SandboxMaintenanceOutcome::Scheduled
        };
        assert_eq!(
            reopened
                .schedule_inactivity_pause(after_grace)
                .await
                .unwrap(),
            expected
        );
        let after = reopened.get_sandbox(&sandbox.id).await.unwrap().unwrap();
        assert_eq!(after.profile, sandbox.profile);
        assert_eq!(after.runtime, sandbox.runtime);
        assert_eq!(after.id, sandbox.id);
        assert_eq!(reopened.get_agent(&ready.id).await.unwrap(), Some(ready));
        assert_eq!(rows.find_one(doc! {}).await.unwrap(), Some(historical));
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn preview_preserves_all_documents_for_eligible_and_corrupt_legacy_evidence() {
    use mongodb::bson::Document;
    async fn snapshot(
        client: &Client,
        name: &str,
    ) -> std::collections::BTreeMap<String, Vec<Document>> {
        let db = client.database(name);
        let mut snapshot = std::collections::BTreeMap::new();
        for collection in db.list_collection_names().await.unwrap() {
            let documents = db
                .collection::<Document>(&collection)
                .find(doc! {})
                .sort(doc! {"_id": 1})
                .await
                .unwrap()
                .try_collect()
                .await
                .unwrap();
            snapshot.insert(collection, documents);
        }
        snapshot
    }
    let (client, name, store) = database().await;
    let (sandbox, ready) = fixture(&store, "sbx_preview_integrity").await;
    enable(&client, &name).await;
    for corrupt in [false, true] {
        if corrupt {
            store
                .agent_log
                .delete_one(doc! {"agent_id": ready.id.as_str()})
                .await
                .unwrap();
        }
        let before = snapshot(&client, &name).await;
        let result = store.preview_inactivity_pause(global(&sandbox.id)).await;
        if corrupt {
            assert!(matches!(
                result,
                Err(SandboxStoreError::SequenceConflict | SandboxStoreError::Corrupt(_))
            ));
        } else {
            assert_eq!(result.unwrap(), SandboxMaintenanceOutcome::Scheduled);
        }
        assert_eq!(snapshot(&client, &name).await, before);
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn corrupt_evidence_is_read_only_in_preview_and_quarantined_by_scheduling() {
    use crate::store::agent_log_integrity::{INTEGRITY_FIELD, QUARANTINE_FIELD};
    use layer2_core::sandbox::store::InactivityPauseCause;
    use mongodb::bson::Document;
    for invalid_metadata in [false, true] {
        for cause in [
            InactivityPauseCause::Inactivity,
            InactivityPauseCause::OperatorInactivity,
        ] {
            let (client, name, store) = database().await;
            let (sandbox, ready) = fixture(&store, "sbx_corrupt_evidence").await;
            enable(&client, &name).await;
            let filter = doc! {"_id": ready.id.as_str()};
            if invalid_metadata {
                store
                    .agents
                    .update_one(
                        filter.clone(),
                        doc! {"$set": {INTEGRITY_FIELD: {"format": 1_i64}}},
                    )
                    .await
                    .unwrap();
            } else {
                store
                    .agent_log
                    .delete_one(doc! {"agent_id": ready.id.as_str()})
                    .await
                    .unwrap();
            }
            let db = client.database(&name);
            let mut before = std::collections::BTreeMap::new();
            for collection in db.list_collection_names().await.unwrap() {
                let documents: Vec<Document> = db
                    .collection(&collection)
                    .find(doc! {})
                    .sort(doc! {"_id": 1})
                    .await
                    .unwrap()
                    .try_collect()
                    .await
                    .unwrap();
                before.insert(collection, documents);
            }
            let request = SandboxInactivityPauseRequest {
                cause,
                ..global(&sandbox.id)
            };
            let preview = store.preview_inactivity_pause(request.clone()).await;
            assert!(matches!(
                preview,
                Err(SandboxStoreError::SequenceConflict | SandboxStoreError::Corrupt(_))
            ));
            assert_eq!(
                db.list_collection_names().await.unwrap().len(),
                before.len()
            );
            for (collection, documents) in &before {
                let after: Vec<Document> = db
                    .collection(collection)
                    .find(doc! {})
                    .sort(doc! {"_id": 1})
                    .await
                    .unwrap()
                    .try_collect()
                    .await
                    .unwrap();
                assert_eq!(&after, documents, "preview mutated {collection}");
            }
            let result = store.schedule_inactivity_pause(request).await;
            assert_eq!(result, preview);
            let mut quarantined = store.agents.find_one(filter).await.unwrap().unwrap();
            assert_eq!(quarantined.remove(QUARANTINE_FIELD), Some(doc! {
                "format": 1_i64, "reason": "integrity_validation_failed", "observed_head": 1_i64,
            }.into()));
            assert_eq!(
                &quarantined,
                before.get(store.agents.name()).unwrap().first().unwrap()
            );
            assert_eq!(store.get_sandbox(&sandbox.id).await.unwrap(), Some(sandbox));
            assert!(store
                .sandbox_events(&ready.sandbox_id, 0, 100)
                .await
                .unwrap()
                .is_empty());
            client.database(&name).drop().await.unwrap();
        }
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn global_policy_revision_fences_disable_threshold_and_grace_races() {
    for change in ["disable", "threshold", "grace"] {
        let (client, name, store) = database().await;
        let (sandbox, ready) = fixture(&store, "sbx_policy_race").await;
        let (settings, mut policy) = enable(&client, &name).await;
        let mut session = client.start_session().await.unwrap();
        start_transaction(&mut session).await.unwrap();
        evidence::live_activities(&store, &[ready], &mut session, LATER)
            .await
            .unwrap();
        match change {
            "disable" => policy.settings.inactivity.auto_pause_enabled = false,
            "threshold" => policy.settings.inactivity.pause_after_seconds = 3600,
            _ => policy.settings.inactivity.grace_started_at = Some(LATER.into()),
        }
        settings
            .replace(policy.revision, &policy.settings)
            .await
            .unwrap();
        let result = transaction::apply(
            &store,
            &MaintenanceRequest::Inactivity(global(&sandbox.id)),
            &mut session,
        )
        .await;
        assert!(
            matches!(result, Err(ApplyError::Driver(error)) if is_transient(&error)),
            "{change}"
        );
        let _ = session.abort_transaction().await;
        assert_eq!(
            store
                .schedule_inactivity_pause(global(&sandbox.id))
                .await
                .unwrap(),
            SandboxMaintenanceOutcome::NotEligible
        );
        assert_eq!(store.get_sandbox(&sandbox.id).await.unwrap(), Some(sandbox));
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn global_preview_aborts_fences_and_missing_disabled_or_unknown_policy_fails_closed() {
    let (client, name, store) = database().await;
    let (sandbox, _) = fixture(&store, "sbx_policy_preview").await;
    assert_eq!(
        store
            .schedule_inactivity_pause(global(&sandbox.id))
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::NotEligible
    );
    let (settings, policy) = enable(&client, &name).await;
    let before = store
        .budget_settings
        .find_one(doc! {"_id":"platform"})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        store
            .preview_inactivity_pause(global(&sandbox.id))
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::Scheduled
    );
    assert_eq!(
        store
            .budget_settings
            .find_one(doc! {"_id":"platform"})
            .await
            .unwrap()
            .unwrap(),
        before
    );
    assert_eq!(settings.read().await.unwrap(), policy);
    assert_eq!(
        store.get_sandbox(&sandbox.id).await.unwrap(),
        Some(sandbox.clone())
    );
    assert_eq!(
        store
            .schedule_inactivity_pause(global(&sandbox.id))
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::Scheduled
    );
    let after = store
        .budget_settings
        .find_one(doc! {"_id":"platform"})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.get_i64("inactivity_admission_fence").unwrap(), 1);
    assert_eq!(settings.read().await.unwrap(), policy);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn operator_bypasses_only_grace_and_refuses_stale_or_corrupt_policy() {
    use layer2_core::sandbox::store::InactivityPauseCause;
    let (client, name, store) = database().await;
    let (sandbox, _) = fixture(&store, "sbx_operator_policy").await;
    let (settings, mut policy) = enable(&client, &name).await;
    policy.settings.inactivity.grace_started_at = Some(LATER.into());
    let policy = settings
        .replace(policy.revision, &policy.settings)
        .await
        .unwrap();
    let ordinary = global(&sandbox.id);
    let operator = SandboxInactivityPauseRequest {
        cause: InactivityPauseCause::OperatorInactivity,
        expected_policy_revision: Some(policy.revision),
        ..ordinary.clone()
    };
    assert_eq!(
        store.preview_inactivity_pause(ordinary).await.unwrap(),
        SandboxMaintenanceOutcome::NotEligible
    );
    assert_eq!(
        store
            .preview_inactivity_pause(operator.clone())
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::Scheduled
    );
    let stale = SandboxInactivityPauseRequest {
        expected_policy_revision: Some(policy.revision - 1),
        ..operator.clone()
    };
    assert_eq!(
        store.schedule_inactivity_pause(stale).await.unwrap(),
        SandboxMaintenanceOutcome::NotEligible
    );
    let mut updated = policy.settings.clone();
    updated.inactivity.auto_pause_enabled = false;
    settings.replace(policy.revision, &updated).await.unwrap();
    assert_eq!(
        store
            .schedule_inactivity_pause(operator.clone())
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::NotEligible
    );
    store
        .budget_settings
        .update_one(
            doc! {"_id":"platform"},
            doc! {"$set":{"settings.inactivity.pause_after_seconds":"unknown"}},
        )
        .await
        .unwrap();
    assert!(store.schedule_inactivity_pause(operator).await.is_err());
    assert_eq!(store.get_sandbox(&sandbox.id).await.unwrap(), Some(sandbox));
    client.database(&name).drop().await.unwrap();
}
