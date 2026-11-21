//! Cross-owner profile continuity and exact retained revision regression tests.
mod admission_race_tests;
mod publisher_tombstone_tests;

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
        expected_policy_revision: None,
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
