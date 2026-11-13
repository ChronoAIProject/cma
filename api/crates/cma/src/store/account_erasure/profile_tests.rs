//! Cross-owner profile continuity and exact retained revision regression tests.
use super::*;
use layer2_core::ids::AgentProfileId;
use layer2_core::profiles::{
    AgentProfileStore, SandboxProfileSnapshot, SandboxProfileStore, WorkspaceProfileStore,
};
use layer2_core::sandbox::store::{
    SandboxInactivityPauseRequest, SandboxMaintenanceOutcome, SandboxMaintenanceStore, SandboxStore,
};
use layer2_core::trigger_domain::{TriggerId, TriggerProfileReader, TriggerStore};
use mongodb::bson::{doc, Document};

use crate::store::trigger_definitions::integration_tests::{seeded, trigger};
mod launch_tests;
mod publisher_admission_tests;

const NOW: &str = "2026-11-14T00:00:00Z";
async fn erase(db: &mongodb::Database, owner: &SubjectId) {
    let inventory = ErasureInventory::default();
    let mut cursor = Cursor::default();
    for _ in 0..1000 {
        let batch = erase_records(
            db,
            &[Target {
                subject: owner,
                inventory: &inventory,
            }],
            &mut cursor,
            NOW,
        )
        .await
        .unwrap();
        assert!(!batch.waiting);
        if !batch.has_more {
            return;
        }
    }
    panic!("profile erasure did not finish");
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn referenced_profiles_survive_anonymized_and_unreferenced_profiles_are_removed() {
    let seed = seeded("erasure_profiles").await;
    let db = seed.client.database(&seed.database);
    let survivor = SubjectId::new("visitor");
    let trigger_id = TriggerId("trg_other_owner_profile".into());
    seed.store
        .create_trigger(trigger(trigger_id.as_str(), &survivor, &seed.target))
        .await
        .unwrap();
    let public = seed
        .store
        .publish_trigger(&trigger_id, &survivor, 1, NOW.into())
        .await
        .unwrap();
    let original = seed
        .profiles
        .read_trigger_profiles(&seed.target)
        .await
        .unwrap();
    let mut unused = seed
        .profiles
        .agent_profile(&seed.target.agent_profile.id)
        .await
        .unwrap()
        .unwrap();
    unused.id = AgentProfileId("agp_unreferenced_erasure".into());
    seed.profiles
        .create_agent_profile(unused.clone())
        .await
        .unwrap();
    seed.profiles
        .publish_agent_profile(&unused.id, 1, NOW.into())
        .await
        .unwrap();
    let store = crate::store::MongoSandboxStore::open(&seed.client, &seed.database)
        .await
        .unwrap();
    let mut sandbox = crate::store::sandbox_maintenance::mongo_integration_tests::active_sandbox(
        "sbx_profile_visitor",
    );
    sandbox.owner = survivor;
    sandbox.profile = SandboxProfileSnapshot::from(&original.sandbox);
    store.create_sandbox(sandbox.clone()).await.unwrap();
    let agent = crate::store::sandbox_maintenance::mongo_integration_tests::ready_agent(
        &sandbox,
        "agt_profile_visitor",
    );
    crate::store::sandbox_maintenance::mongo_integration_tests::insert_agent(&store, &agent, NOW)
        .await;
    db.collection::<Document>("sandbox_profiles")
        .update_one(
            doc! {"_id":seed.target.sandbox_profile.id.as_str()},
            doc! {"$set":{"record.availability.always_keep_alive":true}},
        )
        .await
        .unwrap();
    erase(&db, &seed.owner).await;
    assert_eq!(
        seed.profiles
            .agent_profile(&seed.target.agent_profile.id)
            .await
            .unwrap()
            .unwrap()
            .owner
            .as_str(),
        "deleted-account"
    );
    assert_eq!(
        seed.profiles
            .workspace_profile(&seed.target.workspace_profile.id)
            .await
            .unwrap()
            .unwrap()
            .owner
            .as_str(),
        "deleted-account"
    );
    assert_eq!(
        seed.profiles
            .sandbox_profile(&seed.target.sandbox_profile.id)
            .await
            .unwrap()
            .unwrap()
            .owner
            .as_str(),
        "deleted-account"
    );
    assert_eq!(
        seed.profiles
            .read_trigger_profiles(&seed.target)
            .await
            .unwrap(),
        original
    );
    assert!(seed
        .store
        .preview_by_public_id(&public.public_id, "Deleted publisher".into())
        .await
        .unwrap()
        .is_some());
    assert!(seed
        .profiles
        .agent_profile(&unused.id)
        .await
        .unwrap()
        .is_none());
    assert!(seed
        .profiles
        .agent_profile_revision(&unused.id, 1)
        .await
        .unwrap()
        .is_none());
    let request = SandboxInactivityPauseRequest {
        sandbox_id: sandbox.id.clone(),
        requested_at: "2026-11-14T01:00:00Z".into(),
        pause_after_seconds: 900,
        auto_pause_enabled: false,
        cause: Default::default(),
    };
    assert_eq!(
        store
            .schedule_inactivity_pause(request.clone())
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::NotEligible
    );
    db.collection::<Document>("sandbox_profiles")
        .update_one(
            doc! {"_id":seed.target.sandbox_profile.id.as_str()},
            doc! {"$set":{"record.availability.always_keep_alive":false}},
        )
        .await
        .unwrap();
    assert_eq!(
        store.schedule_inactivity_pause(request).await.unwrap(),
        SandboxMaintenanceOutcome::Scheduled
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn unreferenced_profile_tree_is_physically_deleted_without_survivor_references() {
    let seed = seeded("erasure_unused_profile_tree").await;
    let db = seed.client.database(&seed.database);
    erase(&db, &seed.owner).await;
    for name in ["agent_profiles", "workspace_profiles", "sandbox_profiles"] {
        assert_eq!(
            db.collection::<Document>(name)
                .count_documents(doc! {"owner":seed.owner.as_str()})
                .await
                .unwrap(),
            0
        );
    }
    for (name, id) in [
        (
            "agent_profile_revisions",
            seed.target.agent_profile.id.as_str(),
        ),
        (
            "workspace_profile_revisions",
            seed.target.workspace_profile.id.as_str(),
        ),
        (
            "sandbox_profile_revisions",
            seed.target.sandbox_profile.id.as_str(),
        ),
    ] {
        assert_eq!(
            db.collection::<Document>(name)
                .count_documents(doc! {"profile_id":id})
                .await
                .unwrap(),
            0
        );
    }
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn publisher_tombstone_intermediate_states_are_unavailable_without_hiding_corruption() {
    let seed = seeded("erasure_trigger_states").await;
    let db = seed.client.database(&seed.database);
    let id = TriggerId("trg_erasure_intermediate".into());
    seed.store
        .create_trigger(trigger(id.as_str(), &seed.owner, &seed.target))
        .await
        .unwrap();
    let public = seed
        .store
        .publish_trigger(&id, &seed.owner, 1, NOW.into())
        .await
        .unwrap();
    assert!(seed
        .store
        .trigger_by_public_id(&public.public_id)
        .await
        .unwrap()
        .is_some());
    assert!(seed
        .store
        .preview_by_public_id(&public.public_id, "Publisher".into())
        .await
        .unwrap()
        .is_some());
    let definitions = db.collection::<Document>("trigger_definitions");
    let definition = definitions
        .find_one(doc! {"_id":id.as_str()})
        .await
        .unwrap()
        .unwrap();
    definitions
        .delete_one(doc! {"_id":id.as_str()})
        .await
        .unwrap();
    assert!(seed
        .store
        .trigger_by_public_id(&public.public_id)
        .await
        .unwrap()
        .is_none());
    assert!(seed
        .store
        .preview_by_public_id(&public.public_id, "Publisher".into())
        .await
        .unwrap()
        .is_none());
    assert!(
        !seed
            .store
            .trigger_revision(&id, &public.revision.digest)
            .await
            .unwrap()
            .unwrap()
            .publisher_deleted
    );
    definitions.insert_one(definition).await.unwrap();
    let revisions = db.collection::<Document>("trigger_definition_revisions");
    let row = revisions
        .find_one(doc! {"trigger_id":id.as_str()})
        .await
        .unwrap()
        .unwrap();
    let subjects = std::collections::BTreeSet::from([seed.owner.to_string()]);
    super::references::redact(&db, "trigger_definition_revisions", &row, &subjects)
        .await
        .unwrap();
    let canonical = revisions
        .find_one(doc! {"trigger_id":id.as_str()})
        .await
        .unwrap()
        .unwrap();
    revisions.update_one(doc! {"trigger_id":id.as_str()}, doc! {"$set":{"published_at":"2026-11-15T00:00:00Z","record.published_at":"2026-11-15T00:00:00Z"}}).await.unwrap();
    assert!(
        matches!(
            seed.store.trigger_by_public_id(&public.public_id).await,
            Err(layer2_core::trigger_domain::TriggerStoreError::Corrupt(_))
        ),
        "a canonical marker cannot authorize changed publication metadata"
    );
    revisions
        .replace_one(doc! {"trigger_id":id.as_str()}, canonical)
        .await
        .unwrap();
    assert!(seed
        .store
        .trigger_by_public_id(&public.public_id)
        .await
        .unwrap()
        .is_none());
    assert!(seed
        .store
        .preview_by_public_id(&public.public_id, "Publisher".into())
        .await
        .unwrap()
        .is_none());
    db.collection::<Document>("trigger_definitions")
        .delete_one(doc! {"_id":id.as_str()})
        .await
        .unwrap();
    assert!(seed
        .store
        .trigger_by_public_id(&public.public_id)
        .await
        .unwrap()
        .is_none());
    assert!(seed
        .store
        .preview_by_public_id(&public.public_id, "Publisher".into())
        .await
        .unwrap()
        .is_none());
    assert!(
        seed.store
            .trigger_revision(&id, &public.revision.digest)
            .await
            .unwrap()
            .unwrap()
            .publisher_deleted
    );
    let corrupt_id = TriggerId("trg_erasure_corrupt".into());
    seed.store
        .create_trigger(trigger(corrupt_id.as_str(), &seed.owner, &seed.target))
        .await
        .unwrap();
    let corrupt_public = seed
        .store
        .publish_trigger(&corrupt_id, &seed.owner, 1, NOW.into())
        .await
        .unwrap();
    revisions
        .update_one(
            doc! {"trigger_id":corrupt_id.as_str()},
            doc! {"$set":{"record.digest":"invalid"}},
        )
        .await
        .unwrap();
    assert!(matches!(
        seed.store
            .trigger_by_public_id(&corrupt_public.public_id)
            .await,
        Err(layer2_core::trigger_domain::TriggerStoreError::Corrupt(_))
    ));
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn private_profile_tree_referenced_only_by_a_survivor_sandbox_is_preserved() {
    let seed = seeded("erasure_private_profile_pins").await;
    let db = seed.client.database(&seed.database);
    let resolved = seed
        .profiles
        .read_trigger_profiles(&seed.target)
        .await
        .unwrap();
    for name in ["agent_profiles", "workspace_profiles", "sandbox_profiles"] {
        db.collection::<Document>(name)
            .update_many(
                doc! {"owner":seed.owner.as_str()},
                doc! {"$set":{"visibility":"private","record.visibility":"private"}},
            )
            .await
            .unwrap();
    }
    let store = crate::store::MongoSandboxStore::open(&seed.client, &seed.database)
        .await
        .unwrap();
    let mut sandbox =
        crate::store::sandbox_mongo_integration_tests::sandbox("sbx_private_profile_visitor");
    sandbox.owner = SubjectId::new("visitor");
    sandbox.profile = SandboxProfileSnapshot::from(&resolved.sandbox);
    store.create_sandbox(sandbox.clone()).await.unwrap();
    erase(&db, &seed.owner).await;
    assert_eq!(
        seed.profiles
            .sandbox_profile(&seed.target.sandbox_profile.id)
            .await
            .unwrap()
            .unwrap()
            .owner
            .as_str(),
        "deleted-account"
    );
    assert_eq!(
        seed.profiles
            .workspace_profile(&seed.target.workspace_profile.id)
            .await
            .unwrap()
            .unwrap()
            .owner
            .as_str(),
        "deleted-account"
    );
    assert_eq!(
        seed.profiles
            .agent_profile(&seed.target.agent_profile.id)
            .await
            .unwrap()
            .unwrap()
            .owner
            .as_str(),
        "deleted-account"
    );
    assert_eq!(
        seed.profiles
            .read_trigger_profiles(&seed.target)
            .await
            .unwrap(),
        resolved
    );
    assert_eq!(
        store.get_sandbox(&sandbox.id).await.unwrap().unwrap(),
        sandbox
    );
    db.drop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn catalogue_sandbox_admission_cannot_commit_a_profile_erased_after_resolution() {
    use futures_util::TryStreamExt;
    use layer2_core::sandbox::{
        CreateSandbox, FixedSandboxCredentialPrerequisite, SandboxOrigin, SandboxPolicy,
        SandboxService,
    };
    use layer2_core::{Authorization, Caller, Capability};
    use std::sync::Arc;
    let seed = seeded("erasure_admission_fence").await;
    let source = seed.client.database(&seed.database);
    let (observed, independent, notice, release) =
        super::race_tests::intercepted("update", "sandbox_admissions").await;
    for name in [
        "agent_profiles",
        "workspace_profiles",
        "sandbox_profiles",
        "agent_profile_revisions",
        "workspace_profile_revisions",
        "sandbox_profile_revisions",
    ] {
        let mut cursor = source
            .collection::<Document>(name)
            .find(doc! {})
            .await
            .unwrap();
        while let Some(row) = cursor.try_next().await.unwrap() {
            independent
                .collection::<Document>(name)
                .insert_one(row)
                .await
                .unwrap();
        }
    }
    let profiles = Arc::new(
        crate::store::sandbox_mongo::MongoProfileStore::open(
            independent.client(),
            independent.name(),
        )
        .await
        .unwrap(),
    );
    let store = Arc::new(
        crate::store::MongoSandboxStore::open(observed.client(), observed.name())
            .await
            .unwrap(),
    );
    let service = SandboxService::new(
        store,
        profiles,
        SandboxPolicy::default(),
        Arc::new(FixedSandboxCredentialPrerequisite::Ready),
    );
    let caller = Caller::new(
        SubjectId::new("visitor"),
        Authorization::new([Capability::CreateSandbox]),
    );
    let revision = seed
        .profiles
        .sandbox_profile_revision(&seed.target.sandbox_profile.id, 1)
        .await
        .unwrap()
        .unwrap();
    let request = CreateSandbox {
        profile: seed.target.sandbox_profile.clone(),
        name: Some("late-catalogue-admission".into()),
        confirmed_digest: Some(revision.digest),
        origin: SandboxOrigin::Manual,
    };
    let task = tokio::spawn(async move { service.create(&caller, request).await });
    tokio::time::timeout(std::time::Duration::from_secs(20), notice.notified())
        .await
        .unwrap();
    erase(&independent, &seed.owner).await;
    release.send(()).unwrap();
    assert!(
        task.await.unwrap().is_err(),
        "a resolved catalogue pin must be fenced in the admission transaction"
    );
    assert_eq!(
        independent
            .collection::<Document>("sandboxes")
            .count_documents(doc! {"owner":"visitor"})
            .await
            .unwrap(),
        0
    );
    independent.drop().await.unwrap();
    source.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn saved_event_worker_profiles_retain_foreign_profile_pins_even_without_their_aggregate() {
    use layer2_core::bots::BotProfiles;
    use layer2_core::event_workers::{
        projection, EventWorkerProfile, EventWorkerProfileInput, EventWorkerProfileStore,
    };
    for historical_only in [false, true] {
        let seed = seeded("erasure_event_worker_profile_pins").await;
        let db = seed.client.database(&seed.database);
        let resolved = seed
            .profiles
            .read_trigger_profiles(&seed.target)
            .await
            .unwrap();
        let profiles = BotProfiles {
            event_worker_profile: None,
            resolved_model: "gpt-5".into(),
            agent: resolved.agent.clone(),
            sandbox: resolved.sandbox.clone(),
            workspace: resolved.workspace.clone(),
        };
        let source = EventWorkerProfileInput {
            name: "Visitor worker".into(),
            description: String::new(),
            agent_profile: seed.target.agent_profile.clone(),
            workspace_profile: seed.target.workspace_profile.clone(),
            sandbox_profile: seed.target.sandbox_profile.clone(),
        };
        let owner = SubjectId::new("visitor");
        let snapshot = projection::snapshot(&projection::WorkerProjection {
            name: &source.name,
            owner: &owner,
            provider: "registered",
            profiles: &profiles,
        })
        .unwrap();
        let worker = EventWorkerProfile {
            provider: "registered".into(),
            id: format!("ewp_{}", uuid::Uuid::new_v4().simple()),
            owner,
            revision: 1,
            source,
            profiles,
            digest: format!(
                "sha256:{}",
                layer2_core::profiles::content_digest(&snapshot).unwrap()
            ),
            snapshot,
            created_at: NOW.into(),
            updated_at: NOW.into(),
            retired_at: None,
            creation_fingerprint: "e".repeat(64),
        };
        worker.validate().unwrap();
        let workers = crate::store::event_workers::MongoEventWorkerProfileStore::open(
            &seed.client,
            &seed.database,
        )
        .await
        .unwrap();
        workers.save(None, &worker).await.unwrap();
        assert_eq!(
            workers.get(&worker.id, Some(1)).await.unwrap(),
            Some(worker.clone())
        );
        if historical_only {
            db.collection::<Document>("event_worker_profiles")
                .delete_one(doc! {"_id":&worker.id})
                .await
                .unwrap();
        }
        erase(&db, &seed.owner).await;
        assert_eq!(
            workers.get(&worker.id, Some(1)).await.unwrap(),
            Some(worker.clone())
        );
        if !historical_only {
            assert_eq!(workers.get(&worker.id, None).await.unwrap(), Some(worker));
        }
        assert_eq!(
            seed.profiles
                .read_trigger_profiles(&seed.target)
                .await
                .unwrap(),
            resolved
        );
        assert_eq!(
            seed.profiles
                .sandbox_profile(&seed.target.sandbox_profile.id)
                .await
                .unwrap()
                .unwrap()
                .owner
                .as_str(),
            "deleted-account"
        );
        assert_eq!(
            seed.profiles
                .workspace_profile(&seed.target.workspace_profile.id)
                .await
                .unwrap()
                .unwrap()
                .owner
                .as_str(),
            "deleted-account"
        );
        assert_eq!(
            seed.profiles
                .agent_profile(&seed.target.agent_profile.id)
                .await
                .unwrap()
                .unwrap()
                .owner
                .as_str(),
            "deleted-account"
        );
        db.drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn publisher_tombstone_does_not_hide_later_revision_metadata_corruption() {
    let seed = seeded("erasure_strict_revision_inventory").await;
    let db = seed.client.database(&seed.database);
    let id = TriggerId("trg_erasure_multiple_revisions".into());
    seed.store
        .create_trigger(trigger(id.as_str(), &seed.owner, &seed.target))
        .await
        .unwrap();
    seed.store
        .publish_trigger(&id, &seed.owner, 1, NOW.into())
        .await
        .unwrap();
    let definition = seed.store.trigger(&id, &seed.owner).await.unwrap().unwrap();
    let mut draft = definition.draft;
    draft.presentation.title = "Second revision".into();
    let saved = seed
        .store
        .save_draft(
            &id,
            &seed.owner,
            definition.draft_version,
            draft,
            "2026-11-14T00:01:00Z".into(),
        )
        .await
        .unwrap();
    let public = seed
        .store
        .publish_trigger(
            &id,
            &seed.owner,
            saved.draft_version,
            "2026-11-14T00:01:00Z".into(),
        )
        .await
        .unwrap();
    assert!(seed
        .store
        .trigger_by_public_id(&public.public_id)
        .await
        .unwrap()
        .is_some());
    let revisions = db.collection::<Document>("trigger_definition_revisions");
    let first = revisions
        .find_one(doc! {"trigger_id":id.as_str(),"revision":1_i64})
        .await
        .unwrap()
        .unwrap();
    super::references::redact(
        &db,
        "trigger_definition_revisions",
        &first,
        &std::collections::BTreeSet::from([seed.owner.to_string()]),
    )
    .await
    .unwrap();
    assert!(seed
        .store
        .trigger_by_public_id(&public.public_id)
        .await
        .unwrap()
        .is_none());
    let second = revisions
        .find_one(doc! {"trigger_id":id.as_str(),"revision":2_i64})
        .await
        .unwrap()
        .unwrap();
    revisions
        .update_one(
            doc! {"trigger_id":id.as_str(),"revision":2_i64},
            doc! {"$set":{
                "published_at":"2026-11-15T00:00:00Z","record.published_at":"2026-11-15T00:00:00Z",
            }},
        )
        .await
        .unwrap();
    assert!(matches!(
        seed.store.trigger_by_public_id(&public.public_id).await,
        Err(layer2_core::trigger_domain::TriggerStoreError::Corrupt(_))
    ));
    assert!(matches!(
        seed.store
            .preview_by_public_id(&public.public_id, "Publisher".into())
            .await,
        Err(layer2_core::trigger_domain::TriggerStoreError::Corrupt(_))
    ));
    revisions
        .replace_one(doc! {"trigger_id":id.as_str(),"revision":2_i64}, second)
        .await
        .unwrap();
    assert!(seed
        .store
        .trigger_by_public_id(&public.public_id)
        .await
        .unwrap()
        .is_none());
    db.drop().await.unwrap();
}
